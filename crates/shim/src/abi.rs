//! C-ABI surface that WaveCrux's loader binds against.
//!
//! Every symbol in this module mirrors a declaration in
//! `include/wavecrux_decoder.h`. Panics inside any callback are caught
//! and converted to an error return code — a panic must never unwind
//! across the FFI boundary.

#![allow(non_camel_case_types)]
#![allow(clippy::missing_safety_doc)]

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::OnceLock;

use log::{error, warn};

use crate::registry::{Registry, RegistryError};

// ── ABI constants (must match wavecrux_decoder.h) ────────────────────

const WAVECRUX_DECODER_ABI_MAJOR: u32 = 1;
// Bumped to 1 for the optional self-identification entry points
// (`wavecrux_decoder_plugin_name` / `_description`). The host only gates
// on MAJOR, so this remains backward compatible.
const WAVECRUX_DECODER_ABI_MINOR: u32 = 1;

const WC_DECODER_OK: i32 = 0;
const WC_DECODER_ERR: i32 = 1;
const WC_DECODER_NEED_MORE_SLOTS: i32 = 2;

// ── struct layouts mirroring the C header ────────────────────────────

#[repr(C)]
pub struct WcSample {
    pub timestamp_fs: u64,
    pub bits_ptr: *const u8,
    pub bit_width: u32,
    pub _reserved0: u32,
}

#[repr(C)]
pub struct WcTransaction {
    pub start_fs: u64,
    pub end_fs: u64,
    pub label: *const c_char,
    pub fields_json: *const c_char,
    pub is_error: u32,
    pub _reserved0: u32,
}

pub type WcDecoderHandle = *mut c_void;

pub type WcDecoderCreateFn = extern "C" fn(*const c_char) -> WcDecoderHandle;
pub type WcDecoderFeedFn =
    extern "C" fn(WcDecoderHandle, *const WcSample, *mut WcTransaction, *mut usize) -> c_int;
pub type WcDecoderFlushFn = extern "C" fn(WcDecoderHandle, *mut WcTransaction, *mut usize) -> c_int;
pub type WcDecoderDestroyFn = extern "C" fn(WcDecoderHandle);

#[repr(C)]
pub struct WcDecoderDef {
    pub id: *const c_char,
    pub display_name: *const c_char,
    pub manifest_json: *const c_char,
    pub create: WcDecoderCreateFn,
    pub feed: WcDecoderFeedFn,
    pub flush: WcDecoderFlushFn,
    pub destroy: WcDecoderDestroyFn,
    pub _reserved0: *mut c_void,
    pub _reserved1: u64,
}

// ── Registry: lazily built on the first registration call ────────────

static REGISTRY: OnceLock<std::sync::Mutex<Registry>> = OnceLock::new();

fn registry() -> &'static std::sync::Mutex<Registry> {
    REGISTRY.get_or_init(|| std::sync::Mutex::new(Registry::new()))
}

// ── Public C entry points ────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn wavecrux_decoder_abi_version() -> u32 {
    (WAVECRUX_DECODER_ABI_MAJOR << 16) | WAVECRUX_DECODER_ABI_MINOR
}

#[no_mangle]
pub unsafe extern "C" fn wavecrux_decoder_register(
    out_defs: *mut WcDecoderDef,
    inout_count: *mut usize,
) -> c_int {
    crate::init_logger_once();

    let result = catch_unwind(AssertUnwindSafe(|| {
        if inout_count.is_null() {
            return WC_DECODER_ERR;
        }
        let provided_slots = unsafe { *inout_count };

        let mut registry = match registry().lock() {
            Ok(r) => r,
            Err(_) => return WC_DECODER_ERR,
        };

        match registry.populate(provided_slots) {
            Ok(defs) => {
                let n = defs.len();
                if n > provided_slots {
                    unsafe { *inout_count = n };
                    return WC_DECODER_NEED_MORE_SLOTS;
                }
                if !out_defs.is_null() {
                    for (i, def) in defs.iter().enumerate() {
                        unsafe {
                            std::ptr::write(out_defs.add(i), def.as_c_def());
                        }
                    }
                }
                unsafe { *inout_count = n };
                WC_DECODER_OK
            }
            Err(RegistryError::SubprocessUnavailable(reason)) => {
                warn!(
                    "wavecrux-sigrok-bridge: subprocess unavailable, \
                     plugin will not contribute decoders ({reason})"
                );
                unsafe { *inout_count = 0 };
                WC_DECODER_OK
            }
            Err(e) => {
                error!("wavecrux-sigrok-bridge: registration failed: {e}");
                WC_DECODER_ERR
            }
        }
    }));

    match result {
        Ok(code) => code,
        Err(_) => {
            error!("wavecrux-sigrok-bridge: panic in registration");
            WC_DECODER_ERR
        }
    }
}

/// Optional ABI 1.1 entry point: the plugin's own user-facing name.
/// WaveCrux shows this as the Settings → Decoders plugin-card title
/// rather than borrowing the first decoder's name (which, against a real
/// libsigrokdecode corpus, would be the alphabetically-first decoder —
/// "Audio Codec '97"). Returns a borrowed-for-lifetime static string.
#[no_mangle]
pub extern "C" fn wavecrux_decoder_plugin_name() -> *const c_char {
    // Static, NUL-terminated, valid for the library's whole lifetime.
    b"WaveCrux SigRok Bridge\0".as_ptr() as *const c_char
}

/// Optional ABI 1.1 entry point: a one-line plugin description, shown
/// beneath the name. Carries the GPLv3+ notice (repo invariant 4) so it
/// is visible wherever the plugin is listed.
#[no_mangle]
pub extern "C" fn wavecrux_decoder_plugin_description() -> *const c_char {
    b"libsigrokdecode protocol decoders, via subprocess bridge (GPLv3+).\0".as_ptr()
        as *const c_char
}

/// Helper for the registry: a self-contained, leaked-on-purpose
/// snapshot of one registered decoder. Strings live in `Box<CString>`s
/// owned by the registry; pointers handed back to the loader are
/// borrowed for the lifetime of the plugin.
pub(crate) struct OwnedDecoderDef {
    id: CString,
    display_name: CString,
    manifest_json: CString,
}

impl OwnedDecoderDef {
    pub(crate) fn new(id: String, display_name: String, manifest_json: String) -> Self {
        Self {
            id: CString::new(id).expect("decoder id must not contain nul"),
            display_name: CString::new(display_name).expect("display name must not contain nul"),
            manifest_json: CString::new(manifest_json).expect("manifest json must not contain nul"),
        }
    }

    pub(crate) fn as_c_def(&self) -> WcDecoderDef {
        WcDecoderDef {
            id: self.id.as_ptr(),
            display_name: self.display_name.as_ptr(),
            manifest_json: self.manifest_json.as_ptr(),
            create: callbacks::create,
            feed: callbacks::feed,
            flush: callbacks::flush,
            destroy: callbacks::destroy,
            _reserved0: std::ptr::null_mut(),
            _reserved1: 0,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn id_str(&self) -> &str {
        self.id.to_str().unwrap_or("")
    }
}

// ── Per-instance lifecycle callbacks ─────────────────────────────────

mod callbacks {
    use super::*;

    pub(super) extern "C" fn create(config_json: *const c_char) -> WcDecoderHandle {
        let result = catch_unwind(AssertUnwindSafe(|| {
            let json = if config_json.is_null() {
                "{}".to_owned()
            } else {
                unsafe { CStr::from_ptr(config_json) }
                    .to_string_lossy()
                    .into_owned()
            };

            let registry = registry().lock().ok()?;
            registry.create_instance(&json).ok()
        }));
        match result {
            Ok(Some(handle)) => handle.into_raw() as WcDecoderHandle,
            _ => std::ptr::null_mut(),
        }
    }

    pub(super) extern "C" fn feed(
        handle: WcDecoderHandle,
        sample: *const WcSample,
        out: *mut WcTransaction,
        inout_count: *mut usize,
    ) -> c_int {
        if handle.is_null() || sample.is_null() || inout_count.is_null() {
            return WC_DECODER_ERR;
        }
        let sample_ref = unsafe { &*sample };
        let result = catch_unwind(AssertUnwindSafe(|| {
            let inst = unsafe { crate::registry::Instance::borrow(handle) };
            let provided = unsafe { *inout_count };
            match inst.feed(sample_ref, provided) {
                Ok(written) => {
                    unsafe { write_transactions(out, &written) };
                    unsafe { *inout_count = written.len() };
                    WC_DECODER_OK
                }
                Err(crate::registry::FeedError::NeedsMoreSlots(n)) => {
                    unsafe { *inout_count = n };
                    WC_DECODER_NEED_MORE_SLOTS
                }
                Err(_) => WC_DECODER_ERR,
            }
        }));
        result.unwrap_or(WC_DECODER_ERR)
    }

    pub(super) extern "C" fn flush(
        handle: WcDecoderHandle,
        out: *mut WcTransaction,
        inout_count: *mut usize,
    ) -> c_int {
        if handle.is_null() || inout_count.is_null() {
            return WC_DECODER_ERR;
        }
        let result = catch_unwind(AssertUnwindSafe(|| {
            let inst = unsafe { crate::registry::Instance::borrow(handle) };
            let provided = unsafe { *inout_count };
            match inst.flush(provided) {
                Ok(written) => {
                    unsafe { write_transactions(out, &written) };
                    unsafe { *inout_count = written.len() };
                    WC_DECODER_OK
                }
                Err(crate::registry::FeedError::NeedsMoreSlots(n)) => {
                    unsafe { *inout_count = n };
                    WC_DECODER_NEED_MORE_SLOTS
                }
                Err(_) => WC_DECODER_ERR,
            }
        }));
        result.unwrap_or(WC_DECODER_ERR)
    }

    pub(super) extern "C" fn destroy(handle: WcDecoderHandle) {
        if handle.is_null() {
            return;
        }
        let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
            crate::registry::Instance::take_and_drop(handle);
        }));
    }

    /// Copy `txns` into the loader's slot array. The loader has already
    /// guaranteed `out` points to at least `txns.len()` slots.
    unsafe fn write_transactions(
        out: *mut WcTransaction,
        txns: &[crate::registry::OwnedTransaction],
    ) {
        for (i, t) in txns.iter().enumerate() {
            unsafe { std::ptr::write(out.add(i), t.as_c_view()) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abi_version_is_one_one() {
        assert_eq!(wavecrux_decoder_abi_version(), 0x0001_0001);
    }

    #[test]
    fn plugin_name_and_description_are_valid_cstrings() {
        // SAFETY: both entry points return static NUL-terminated strings
        // valid for the process lifetime.
        let name = unsafe { CStr::from_ptr(wavecrux_decoder_plugin_name()) }
            .to_str()
            .unwrap();
        assert_eq!(name, "WaveCrux SigRok Bridge");

        let desc = unsafe { CStr::from_ptr(wavecrux_decoder_plugin_description()) }
            .to_str()
            .unwrap();
        assert!(
            desc.contains("GPLv3+"),
            "description must carry the GPL notice"
        );
    }
}
