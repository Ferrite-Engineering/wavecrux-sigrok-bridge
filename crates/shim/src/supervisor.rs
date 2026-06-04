//! Subprocess supervisor — owns the bridge subprocess handle, the
//! request/response correlation table, and the unsolicited-event queue.
//!
//! The supervisor is synchronous: one outstanding request at a time.
//! That is sufficient because the WaveCrux loader is single-threaded
//! per plugin handle (per the C ABI's threading rules) and `Registry`
//! serializes every request through a `Mutex`.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use log::{debug, info, warn};

use wavecrux_sigrok_bridge_ipc::{
    Event, FrameReader, FrameWriter, Message, Request, RequestOp, Response,
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum SupervisorError {
    #[error("could not spawn bridge subprocess: {0}")]
    SpawnFailed(String),

    #[error("bridge subprocess died unexpectedly")]
    SubprocessDied,

    #[error("ipc protocol violation: {0}")]
    Protocol(String),

    #[error("io: {0}")]
    Io(String),
}

impl From<wavecrux_sigrok_bridge_ipc::CodecError> for SupervisorError {
    fn from(e: wavecrux_sigrok_bridge_ipc::CodecError) -> Self {
        match e {
            wavecrux_sigrok_bridge_ipc::CodecError::PeerClosed => Self::SubprocessDied,
            other => Self::Io(other.to_string()),
        }
    }
}

pub(crate) struct Supervisor {
    child: Child,
    writer: FrameWriter<ChildStdin>,
    reader: FrameReader<ChildStdout>,
    next_id: AtomicU64,
    event_queue: VecDeque<Event>,
}

impl Supervisor {
    /// Spawn the bridge subprocess. Resolution order:
    ///
    ///   1. `WAVECRUX_SIGROK_BRIDGE` env var (must be an absolute path).
    ///   2. Sibling binary in the same directory as this shared library
    ///      (resolved via `dladdr`-style lookup; falls back to the
    ///      current-exe's parent dir if that fails).
    ///   3. `wavecrux-sigrok-bridge` on `PATH`.
    pub(crate) fn spawn() -> Result<Self, SupervisorError> {
        let path = resolve_subprocess_path().ok_or_else(|| {
            SupervisorError::SpawnFailed(
                "wavecrux-sigrok-bridge binary not found (set \
                     WAVECRUX_SIGROK_BRIDGE or add it to PATH)"
                    .to_owned(),
            )
        })?;
        info!("wavecrux-sigrok-bridge: spawning {}", path.display());

        let mut child = Command::new(&path)
            .arg("--ipc")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| SupervisorError::SpawnFailed(e.to_string()))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SupervisorError::Io("no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SupervisorError::Io("no stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| SupervisorError::Io("no stderr".into()))?;
        std::thread::spawn(move || drain_stderr(stderr));

        Ok(Self {
            child,
            writer: FrameWriter::new(stdin),
            reader: FrameReader::new(stdout),
            next_id: AtomicU64::new(1),
            event_queue: VecDeque::new(),
        })
    }

    /// Send a request, drive the receive loop until the matching
    /// response arrives, and return it. Any [`Event`]s that arrive in
    /// the meantime are buffered for [`Self::drain_events`].
    pub(crate) fn request(&mut self, op: RequestOp) -> Result<Response, SupervisorError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let req = Request { id, op };
        let body = serde_json::to_vec(&Message::Request(req))
            .map_err(|e| SupervisorError::Protocol(e.to_string()))?;
        self.writer.write_frame(&body)?;

        loop {
            let frame = self.reader.read_frame()?;
            let msg: Message = serde_json::from_slice(&frame)
                .map_err(|e| SupervisorError::Protocol(e.to_string()))?;
            match msg {
                Message::Response(r) if r.id == id => return Ok(r),
                Message::Response(r) => {
                    warn!(
                        "wavecrux-sigrok-bridge: dropping orphan response \
                         id={} (we asked for {id})",
                        r.id
                    );
                }
                Message::Event(e) => self.event_queue.push_back(e),
                Message::Request(_) => {
                    return Err(SupervisorError::Protocol(
                        "subprocess sent a request to the shim".into(),
                    ));
                }
            }
        }
    }

    /// Take any unsolicited events the supervisor has buffered.
    pub(crate) fn drain_events(&mut self) -> Vec<Event> {
        self.event_queue.drain(..).collect()
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        debug!("wavecrux-sigrok-bridge: tearing down subprocess");
        // Attempt a polite shutdown.
        let body = serde_json::to_vec(&Message::Request(Request {
            id: 0,
            op: RequestOp::Shutdown,
        }))
        .unwrap_or_default();
        let _ = self.writer.write_frame(&body);

        // Wait up to 2 s for the child to exit on its own.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn resolve_subprocess_path() -> Option<PathBuf> {
    // 1. env var override.
    if let Ok(p) = std::env::var("WAVECRUX_SIGROK_BRIDGE") {
        let path = PathBuf::from(p);
        if path.is_absolute() && path.exists() {
            return Some(path);
        }
    }

    // 2. Sibling binary in the same directory as this shared library.
    //    shim_library_path() uses dladdr (Unix) / GetModuleHandleEx
    //    (Windows) to get this dylib's own on-disk path — correct even
    //    when the shim is dlopen'd from a foreign process, where
    //    current_exe() would return the host app's path instead.
    if let Some(lib_path) = shim_library_path() {
        if let Some(parent) = lib_path.parent() {
            let candidate = parent.join(subprocess_filename());
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    // 3. PATH search.
    let name = subprocess_filename();
    if let Some(path) = which_on_path(&name) {
        return Some(path);
    }
    None
}

// Anchor used by dladdr / GetModuleHandleEx to identify this dylib.
// Must not be inlined so the optimizer cannot place the code outside
// this compilation unit's address range.
#[inline(never)]
fn shim_anchor() {}

/// Returns the on-disk path of this shared library (.so / .dylib / .dll).
#[cfg(unix)]
fn shim_library_path() -> Option<PathBuf> {
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    // Pass the address of a known symbol in this DSO. dladdr resolves it
    // to the containing library's path via the dynamic linker's link-map —
    // no symbol-name table needed, so strip = "symbols" is safe.
    let ok = unsafe { libc::dladdr(shim_anchor as *const core::ffi::c_void, &mut info) };
    if ok == 0 || info.dli_fname.is_null() {
        return None;
    }
    let s = unsafe { std::ffi::CStr::from_ptr(info.dli_fname) }
        .to_str()
        .ok()?;
    Some(PathBuf::from(s))
}

#[cfg(windows)]
fn shim_library_path() -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    type Bool = i32;
    type Dword = u32;
    type Hmodule = *mut core::ffi::c_void;
    type Lpcvoid = *const core::ffi::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleExW(flags: Dword, module_name: Lpcvoid, module: *mut Hmodule) -> Bool;
        fn GetModuleFileNameW(module: Hmodule, filename: *mut u16, size: Dword) -> Dword;
    }

    // FLAG_FROM_ADDRESS: resolve the module that contains the given address.
    // FLAG_UNCHANGED_REFCOUNT: don't increment the module's reference count.
    const FROM_ADDRESS: Dword = 0x0000_0004;
    const UNCHANGED_REFCOUNT: Dword = 0x0000_0002;

    let mut hmod: Hmodule = std::ptr::null_mut();
    let ok = unsafe {
        GetModuleHandleExW(
            FROM_ADDRESS | UNCHANGED_REFCOUNT,
            shim_anchor as Lpcvoid,
            &mut hmod,
        )
    };
    if ok == 0 {
        return None;
    }
    // MAX_PATH on Windows is 260, but long-path-aware code uses 32 768.
    let mut buf = vec![0u16; 32_768];
    let len = unsafe { GetModuleFileNameW(hmod, buf.as_mut_ptr(), buf.len() as Dword) };
    if len == 0 {
        return None;
    }
    Some(PathBuf::from(OsString::from_wide(&buf[..len as usize])))
}

#[cfg(not(any(unix, windows)))]
fn shim_library_path() -> Option<PathBuf> {
    None
}

fn subprocess_filename() -> String {
    if cfg!(windows) {
        "wavecrux-sigrok-bridge.exe".to_owned()
    } else {
        "wavecrux-sigrok-bridge".to_owned()
    }
}

fn which_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn drain_stderr(stderr: ChildStderr) {
    let reader = BufReader::new(stderr);
    for line in reader.lines().map_while(Result::ok) {
        info!("wavecrux-sigrok-bridge[stderr]: {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subprocess_filename_matches_platform() {
        let name = subprocess_filename();
        if cfg!(windows) {
            assert!(name.ends_with(".exe"));
        } else {
            assert!(!name.contains('.'));
        }
    }
}
