//! In-memory registry of decoders the bridge subprocess advertises,
//! plus per-instance state for active decode sessions.
//!
//! All strings handed to the WaveCrux loader (`WcDecoderDef.id`,
//! `display_name`, `manifest_json`) are owned by [`Registry`] for the
//! lifetime of the plugin's library load. The C ABI documents this
//! borrowing relationship.

use std::collections::VecDeque;
use std::ffi::{c_char, c_void, CString};
use std::sync::Mutex;

use log::{error, warn};
use serde::{Deserialize, Serialize};

use crate::abi::{OwnedDecoderDef, WcSample, WcTransaction};
use crate::manifest::build_manifest_json;
use crate::supervisor::{Supervisor, SupervisorError};
use wavecrux_sigrok_bridge_ipc::{
    AnnotationEvent, BitValue, ChannelChange, CreateSessionBody, DecoderManifest, ErrCode, Event,
    FeedBody, FinalizeBody, RequestOp, ResponseBody, ResponseStatus, SampleBatch, SampleTransition,
    SessionId, XzPolicy,
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum RegistryError {
    #[error("subprocess unavailable: {0}")]
    SubprocessUnavailable(String),

    #[error("subprocess returned an error: {0}")]
    Subprocess(String),

    #[error("internal error: {0}")]
    Internal(String),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum FeedError {
    #[error("instance is in failed state")]
    Failed,
    #[error("loader needs to retry with {0} slots")]
    NeedsMoreSlots(usize),
    #[error("supervisor error: {0}")]
    Supervisor(SupervisorError),
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<SupervisorError> for FeedError {
    fn from(e: SupervisorError) -> Self {
        FeedError::Supervisor(e)
    }
}

pub(crate) struct Registry {
    supervisor: Option<Supervisor>,
    defs: Vec<OwnedDecoderDef>,
    manifests: Vec<DecoderManifest>,
}

impl Registry {
    pub(crate) fn new() -> Self {
        Self {
            supervisor: None,
            defs: vec![],
            manifests: vec![],
        }
    }

    /// Returns the list of `WcDecoderDef`-friendly entries the loader
    /// should publish. On first call, spawns the subprocess and
    /// queries it; subsequent calls return the cached list.
    pub(crate) fn populate(
        &mut self,
        _provided_slots: usize,
    ) -> Result<&[OwnedDecoderDef], RegistryError> {
        if !self.defs.is_empty() {
            return Ok(&self.defs);
        }
        let supervisor = match Supervisor::spawn() {
            Ok(s) => s,
            Err(SupervisorError::SpawnFailed(reason)) => {
                return Err(RegistryError::SubprocessUnavailable(reason));
            }
            Err(e) => {
                return Err(RegistryError::Subprocess(e.to_string()));
            }
        };
        self.supervisor = Some(supervisor);

        let manifests = self.list_decoders_blocking()?;
        for m in &manifests {
            let json = build_manifest_json(m);
            self.defs.push(OwnedDecoderDef::new(
                m.id.clone(),
                m.display_name.clone(),
                json,
            ));
        }
        self.manifests = manifests;
        Ok(&self.defs)
    }

    fn list_decoders_blocking(&mut self) -> Result<Vec<DecoderManifest>, RegistryError> {
        let sup = self
            .supervisor
            .as_mut()
            .ok_or_else(|| RegistryError::Internal("no supervisor".into()))?;
        let resp = sup
            .request(RequestOp::ListDecoders)
            .map_err(|e| RegistryError::Subprocess(e.to_string()))?;
        match resp.status {
            ResponseStatus::Ok {
                body: Some(ResponseBody::Decoders(list)),
            } => Ok(list),
            ResponseStatus::Ok { body: _ } => Err(RegistryError::Internal(
                "unexpected response body for list_decoders".into(),
            )),
            ResponseStatus::Err { code, message } => Err(RegistryError::Subprocess(format!(
                "list_decoders failed: {code:?}: {message}"
            ))),
        }
    }

    /// Build a new decoder instance.
    ///
    /// The caller has already obtained the `config_json` from the
    /// WaveCrux loader. We parse the WaveCrux config shape, extract the
    /// decoder id and signal-binding map, and ask the subprocess to
    /// open a session.
    pub(crate) fn create_instance(
        &self,
        config_json: &str,
    ) -> Result<Box<Instance>, RegistryError> {
        let cfg: WaveCruxInstanceConfig = serde_json::from_str(config_json)
            .map_err(|e| RegistryError::Internal(format!("bad config json: {e}")))?;

        let manifest = self
            .manifests
            .iter()
            .find(|m| m.id == cfg.decoder_id)
            .ok_or_else(|| RegistryError::Internal(format!("unknown decoder {}", cfg.decoder_id)))?
            .clone();

        // Channel binding: WaveCrux gives us {channel_name: signal_ref}
        // (signal_ref is a string identifier scoped to the loaded VCD).
        // We translate that into per-channel bit-position indices in
        // the sample frames the shim will emit. The loader emits all
        // bound signals in one packed bit array per `WcSample`, in
        // declaration order from the manifest's `signals` list. The
        // shim assigns 0..N indices in that same order.
        let mut channels = std::collections::BTreeMap::new();
        for (idx, c) in manifest.channels.iter().enumerate() {
            // Optional channel left unbound is allowed; required one is
            // not. The WaveCrux loader has already validated the
            // required-channel rule before reaching us, but we double-
            // check defensively.
            if cfg.signal_bindings.contains_key(&c.name) {
                channels.insert(c.name.clone(), idx as u32);
            } else if c.required {
                return Err(RegistryError::Internal(format!(
                    "required channel {} unbound",
                    c.name
                )));
            }
        }

        // We need a mutable borrow on the supervisor; since this method
        // takes &self, the supervisor lives behind a Mutex elsewhere.
        // For now we reconcile that by sharing a single supervisor at
        // crate level — Registry already owns it. The instance keeps a
        // raw pointer back to the registry (via an inner Arc<Mutex<>>).
        // Implemented via SharedRegistry below.
        let shared = SharedRegistry::current();
        let session_id = shared.create_session_blocking(
            cfg.decoder_id.clone(),
            channels,
            cfg.options.clone(),
            cfg.xz_policy,
        )?;

        Ok(Box::new(Instance {
            session_id,
            manifest,
            inbox: Mutex::new(VecDeque::new()),
            options: cfg.options,
            xz_policy: cfg.xz_policy,
            failed: std::sync::atomic::AtomicBool::new(false),
        }))
    }
}

/// WaveCrux's loader emits this exact JSON shape into `config_json` for
/// each instance. The shim parses it before opening a subprocess
/// session.
#[derive(Debug, Deserialize, Serialize)]
struct WaveCruxInstanceConfig {
    /// Decoder id (e.g. `sigrok.onewire`). Matches the manifest id we
    /// returned from registration.
    decoder_id: String,

    /// Map from channel name → WaveCrux signal reference. We don't
    /// actually need the signal refs (the loader resolves them and
    /// hands us pre-bound `WcSample` bit data); we only need the keys
    /// to know which optional channels were bound.
    #[serde(default)]
    signal_bindings: std::collections::BTreeMap<String, String>,

    /// Per-instance option overrides.
    #[serde(default)]
    options: serde_json::Map<String, serde_json::Value>,

    /// X/Z handling policy.
    #[serde(default)]
    xz_policy: XzPolicy,
}

// ── A single decode-in-progress instance ─────────────────────────────

pub(crate) struct Instance {
    session_id: SessionId,
    /// Captured at session create for future diagnostic surfaces; the
    /// runtime path does not consult it because the bridge subprocess
    /// owns all decode state.
    #[allow(dead_code)]
    manifest: DecoderManifest,
    /// Annotations the supervisor has delivered between blocking calls
    /// but the loader hasn't drained yet.
    inbox: Mutex<VecDeque<OwnedTransaction>>,
    /// Captured for round-tripping when a future minor revision adds a
    /// "describe instance" IPC op.
    #[allow(dead_code)]
    options: serde_json::Map<String, serde_json::Value>,
    /// Captured for the same reason as `options`. The bridge currently
    /// honors the policy via the `CreateSessionBody` it received.
    #[allow(dead_code)]
    xz_policy: XzPolicy,
    failed: std::sync::atomic::AtomicBool,
}

impl Instance {
    /// Reinterpret a raw `WcDecoderHandle` as `&Instance`. Safe iff the
    /// handle came from `create_instance` and has not been destroyed.
    pub(crate) unsafe fn borrow<'a>(handle: *mut c_void) -> &'a Instance {
        unsafe { &*(handle as *const Instance) }
    }

    pub(crate) fn into_raw(self: Box<Instance>) -> *mut Instance {
        Box::into_raw(self)
    }

    /// Drop the instance. Safe iff the handle came from
    /// `create_instance` and has not previously been destroyed.
    pub(crate) unsafe fn take_and_drop(handle: *mut c_void) {
        let inst = unsafe { Box::from_raw(handle as *mut Instance) };
        let shared = SharedRegistry::current();
        let _ = shared.destroy_session_blocking(&inst.session_id);
    }

    /// Forward one sample and return any annotations the bridge has
    /// produced since the last call (capped at `provided_slots`).
    pub(crate) fn feed(
        &self,
        sample: &WcSample,
        provided_slots: usize,
    ) -> Result<Vec<OwnedTransaction>, FeedError> {
        if self.failed.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(FeedError::Failed);
        }

        let transition = self.sample_to_transition(sample)?;
        let shared = SharedRegistry::current();
        shared.feed_blocking(&self.session_id, transition)?;
        self.drain_events(&shared);
        self.take_up_to(provided_slots)
    }

    /// Tell the bridge end-of-stream and drain any final annotations.
    pub(crate) fn flush(&self, provided_slots: usize) -> Result<Vec<OwnedTransaction>, FeedError> {
        if self.failed.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(FeedError::Failed);
        }
        let shared = SharedRegistry::current();
        shared.finalize_blocking(&self.session_id)?;
        self.drain_events(&shared);
        self.take_up_to(provided_slots)
    }

    fn sample_to_transition(&self, s: &WcSample) -> Result<SampleTransition, FeedError> {
        // The WaveCrux loader packs `bit_width` two-bit pairs into the
        // buffer, low bit = level, high bit = unknown flag. See
        // wavecrux_decoder.h §value types.
        let bytes = if s.bits_ptr.is_null() || s.bit_width == 0 {
            &[][..]
        } else {
            unsafe {
                std::slice::from_raw_parts(s.bits_ptr, ((s.bit_width as usize) * 2).div_ceil(8))
            }
        };

        let mut set = Vec::with_capacity(s.bit_width as usize);
        for ch in 0..s.bit_width as usize {
            let bit_idx = ch * 2;
            let byte = bit_idx / 8;
            let lo = (bit_idx % 8) as u32;
            let hi = lo + 1;
            if byte >= bytes.len() {
                break;
            }
            let level = (bytes[byte] >> lo) & 1;
            let unknown_byte = (bit_idx + 1) / 8;
            let unknown = if unknown_byte < bytes.len() {
                (bytes[unknown_byte] >> (hi % 8)) & 1
            } else {
                0
            };
            let v = if unknown == 1 {
                BitValue::X
            } else if level == 1 {
                BitValue::One
            } else {
                BitValue::Zero
            };
            set.push(ChannelChange { ch: ch as u32, v });
        }
        Ok(SampleTransition {
            fs: s.timestamp_fs,
            set,
        })
    }

    fn drain_events(&self, shared: &SharedRegistry) {
        let evs = shared.drain_pending_events_for(&self.session_id);
        if evs.is_empty() {
            return;
        }
        let mut inbox = match self.inbox.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        for ev in evs {
            inbox.push_back(OwnedTransaction::from_annotation(&ev));
        }
    }

    fn take_up_to(&self, max: usize) -> Result<Vec<OwnedTransaction>, FeedError> {
        let mut inbox = self
            .inbox
            .lock()
            .map_err(|_| FeedError::Internal("inbox poisoned".into()))?;
        if inbox.len() > max {
            return Err(FeedError::NeedsMoreSlots(inbox.len()));
        }
        Ok(inbox.drain(..).collect())
    }
}

// ── OwnedTransaction: heap-pinned strings for the C view ─────────────

pub(crate) struct OwnedTransaction {
    label: CString,
    fields_json: CString,
    start_fs: u64,
    end_fs: u64,
    is_error: bool,
}

impl OwnedTransaction {
    fn from_annotation(a: &AnnotationEvent) -> Self {
        let fields = serde_json::Value::Object(a.fields.clone());
        let fields_str = serde_json::to_string(&fields).unwrap_or_else(|_| "{}".into());
        Self {
            label: CString::new(a.label.clone()).unwrap_or_else(|_| CString::new("").unwrap()),
            fields_json: CString::new(fields_str).unwrap_or_else(|_| CString::new("{}").unwrap()),
            start_fs: a.start_fs,
            end_fs: a.end_fs,
            is_error: a.is_error,
        }
    }

    pub(crate) fn as_c_view(&self) -> WcTransaction {
        WcTransaction {
            start_fs: self.start_fs,
            end_fs: self.end_fs,
            label: self.label.as_ptr() as *const c_char,
            fields_json: self.fields_json.as_ptr() as *const c_char,
            is_error: if self.is_error { 1 } else { 0 },
            _reserved0: 0,
        }
    }
}

// ── SharedRegistry: holds the supervisor in a process-global slot ────

/// The supervisor is necessarily process-wide because the C ABI gives
/// us no per-app context pointer. We park it behind a `Mutex` accessible
/// to every `Instance`.
struct SharedRegistry {
    inner: &'static Mutex<SharedRegistryState>,
}

struct SharedRegistryState {
    supervisor: Option<Supervisor>,
}

static SHARED: std::sync::OnceLock<Mutex<SharedRegistryState>> = std::sync::OnceLock::new();

impl SharedRegistry {
    fn current() -> Self {
        let inner = SHARED.get_or_init(|| Mutex::new(SharedRegistryState { supervisor: None }));
        SharedRegistry { inner }
    }

    fn ensure(&self) -> Result<(), SupervisorError> {
        let mut g = self
            .inner
            .lock()
            .map_err(|_| SupervisorError::Io("registry poisoned".into()))?;
        if g.supervisor.is_none() {
            g.supervisor = Some(Supervisor::spawn()?);
        }
        Ok(())
    }

    fn create_session_blocking(
        &self,
        decoder_id: String,
        channels: std::collections::BTreeMap<String, u32>,
        options: serde_json::Map<String, serde_json::Value>,
        xz_policy: XzPolicy,
    ) -> Result<SessionId, RegistryError> {
        self.ensure()
            .map_err(|e| RegistryError::SubprocessUnavailable(e.to_string()))?;
        let mut g = self
            .inner
            .lock()
            .map_err(|_| RegistryError::Internal("registry poisoned".into()))?;
        let sup = g
            .supervisor
            .as_mut()
            .ok_or_else(|| RegistryError::Internal("no supervisor".into()))?;
        let resp = sup
            .request(RequestOp::CreateSession(CreateSessionBody {
                decoder_id,
                channels,
                options,
                xz_policy,
            }))
            .map_err(|e| RegistryError::Subprocess(e.to_string()))?;
        match resp.status {
            ResponseStatus::Ok {
                body: Some(ResponseBody::SessionCreated { session }),
            } => Ok(session),
            ResponseStatus::Ok { body: _ } => Err(RegistryError::Internal(
                "unexpected response body for create_session".into(),
            )),
            ResponseStatus::Err { code, message } => {
                if matches!(code, ErrCode::SessionInitFailed) {
                    warn!("wavecrux-sigrok-bridge: session init failed: {message}");
                }
                Err(RegistryError::Subprocess(message))
            }
        }
    }

    fn feed_blocking(
        &self,
        session: &SessionId,
        transition: SampleTransition,
    ) -> Result<(), FeedError> {
        let mut g = self
            .inner
            .lock()
            .map_err(|_| FeedError::Internal("registry poisoned".into()))?;
        let sup = g
            .supervisor
            .as_mut()
            .ok_or_else(|| FeedError::Internal("no supervisor".into()))?;
        let resp = sup.request(RequestOp::Feed(FeedBody {
            session: session.clone(),
            samples: vec![SampleBatch {
                t: vec![transition],
            }],
        }))?;
        match resp.status {
            ResponseStatus::Ok { .. } => Ok(()),
            ResponseStatus::Err { message, .. } => {
                error!("wavecrux-sigrok-bridge: feed failed for {session}: {message}");
                Err(FeedError::Internal(message))
            }
        }
    }

    fn finalize_blocking(&self, session: &SessionId) -> Result<(), FeedError> {
        let mut g = self
            .inner
            .lock()
            .map_err(|_| FeedError::Internal("registry poisoned".into()))?;
        let sup = g
            .supervisor
            .as_mut()
            .ok_or_else(|| FeedError::Internal("no supervisor".into()))?;
        let resp = sup.request(RequestOp::Finalize(FinalizeBody {
            session: session.clone(),
        }))?;
        match resp.status {
            ResponseStatus::Ok { .. } => Ok(()),
            ResponseStatus::Err { message, .. } => Err(FeedError::Internal(message)),
        }
    }

    fn destroy_session_blocking(&self, session: &SessionId) -> Result<(), FeedError> {
        let mut g = self
            .inner
            .lock()
            .map_err(|_| FeedError::Internal("registry poisoned".into()))?;
        let sup = match g.supervisor.as_mut() {
            Some(s) => s,
            None => return Ok(()),
        };
        let _ = sup.request(RequestOp::Destroy(
            wavecrux_sigrok_bridge_ipc::DestroyBody {
                session: session.clone(),
            },
        ));
        Ok(())
    }

    fn drain_pending_events_for(&self, session: &SessionId) -> Vec<AnnotationEvent> {
        let mut g = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return vec![],
        };
        let sup = match g.supervisor.as_mut() {
            Some(s) => s,
            None => return vec![],
        };
        let mut out = vec![];
        for ev in sup.drain_events() {
            if let Event::Annotation(a) = ev {
                if a.session == *session {
                    out.push(a);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_parses_minimal_shape() {
        let json = r#"{"decoder_id":"sigrok.onewire","signal_bindings":{"data":"top.dq"}}"#;
        let cfg: WaveCruxInstanceConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.decoder_id, "sigrok.onewire");
        assert_eq!(cfg.signal_bindings.get("data").unwrap(), "top.dq");
        assert_eq!(cfg.xz_policy, XzPolicy::Glitch);
    }

    #[test]
    fn config_accepts_options_and_xz_policy() {
        let json = r#"{"decoder_id":"sigrok.uart","signal_bindings":{"rx":"top.rx"},"options":{"baudrate":9600},"xz_policy":"coerce_last"}"#;
        let cfg: WaveCruxInstanceConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.options.get("baudrate").unwrap(), 9600);
        assert_eq!(cfg.xz_policy, XzPolicy::CoerceLast);
    }
}
