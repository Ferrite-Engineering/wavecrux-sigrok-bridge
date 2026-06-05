//! Real libsigrokdecode + Python-embedded backend.
//!
//! This module is gated behind the `sigrok` Cargo feature. It links
//! `libsigrokdecode` (via the bindgen-generated bindings in `build.rs`)
//! and drives the installed Python decoder corpus through the
//! [`Backend`] trait, replacing the five hardcoded mock decoders with
//! the full ~130-decoder libsigrokdecode set.
//!
//! License note: every line of code in this file is GPLv3+, like the
//! rest of the bridge subprocess. The libsigrokdecode bindings pull GPL
//! native code into this binary; that is the whole reason the bridge is
//! its own repo, kept at arm's length from the GPL-free shim.
//!
//! ## How transitions become samples
//!
//! The IPC layer sends sparse, femtosecond-timestamped *transitions*.
//! libsigrokdecode wants dense sample rows. We scale time at a fixed
//! 1 sample = 1 ns (= 1e6 fs), i.e. a 1 GHz logic-analyzer, and replay
//! the level held between consecutive transitions as a run of identical
//! sample rows. Annotation sample numbers are mapped back to
//! femtoseconds by the inverse scale. 1 GHz is finer than any protocol
//! libsigrokdecode targets.
//!
//! ## Threading
//!
//! libsigrokdecode is not thread-safe and the IPC loop is
//! single-threaded (`Backend` takes `&mut self`). The annotation
//! callback fires synchronously inside `srd_session_send`, which blocks
//! until the decoder worker has drained the chunk, so pushing into the
//! per-session accumulator needs no extra locking. The raw pointers in
//! [`SrdBackend`] are only ever touched from that one thread, hence the
//! `unsafe impl Send`.

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};

use anyhow::{anyhow, bail, Result};
use serde_json::Value;

use wavecrux_sigrok_bridge_ipc::{
    AnnotationEvent, BitValue, CreateSessionBody, DecoderAnnotationClass, DecoderChannel,
    DecoderManifest, DecoderOption, FeedBody, OptionKind, SessionId,
};

use crate::backend::Backend;

#[allow(
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    dead_code,
    clippy::all
)]
mod ffi {
    include!(concat!(env!("OUT_DIR"), "/srd_bindings.rs"));
}

// libsigrokdecode return convention: 0 == SRD_OK, every error is a
// negative number (see enum srd_error_code in the header). We compare
// against 0 rather than depending on bindgen's enum constant naming.
const SRD_OK: c_int = 0;
// First entry of `enum srd_output_type` — annotation output.
const SRD_OUTPUT_ANN: c_int = 0;
// `enum srd_configkey { SRD_CONF_SAMPLERATE = 10000 }`.
const SRD_CONF_SAMPLERATE: c_int = 10000;

/// Femtoseconds per sample at our fixed 1 GHz replay rate.
const FS_PER_SAMPLE: u64 = 1_000_000;
/// Samples-per-second corresponding to [`FS_PER_SAMPLE`].
const SAMPLERATE_HZ: u64 = 1_000_000_000;
/// Upper bound on samples materialized per `srd_session_send` call, so a
/// long constant stretch between transitions never allocates an
/// unbounded buffer.
const SEND_CHUNK_SAMPLES: u64 = 65_536;

/// One annotation captured by the C callback, in libsigrokdecode's
/// sample-number domain. Converted to femtoseconds on the way out.
struct RawAnnotation {
    start_sample: u64,
    end_sample: u64,
    ann_class: u32,
    text: String,
}

struct SrdSession {
    sess: *mut ffi::srd_session,
    /// Number of distinct channel bit-positions in the sample frame
    /// (max bound channel index + 1).
    num_channels: usize,
    /// Bytes per sample row = ceil(num_channels / 8).
    unitsize: usize,
    /// Current logic level (0/1) per channel bit-position.
    levels: Vec<u8>,
    /// Absolute sample number from which `levels` currently holds.
    /// Samples below this have already been sent.
    cur_sample: u64,
    /// Whether we have observed the first transition (which seeds the
    /// baseline level without emitting samples).
    started: bool,
    /// Heap-stable accumulator the C callback pushes into. Owned here;
    /// freed in `destroy`.
    accum: *mut Vec<RawAnnotation>,
}

pub(crate) struct SrdBackend {
    initialized: bool,
    sessions: HashMap<SessionId, SrdSession>,
    next_session: u64,
}

// All libsigrokdecode access is confined to the single IPC thread; see
// the module docs.
unsafe impl Send for SrdBackend {}

impl SrdBackend {
    pub(crate) fn new() -> Self {
        // `srd_init(NULL)` uses the default decoder search path (the
        // installed Python decoder corpus) and initializes the embedded
        // interpreter. Must happen exactly once per process before any
        // other srd call.
        let rc = unsafe { ffi::srd_init(std::ptr::null()) };
        if rc != SRD_OK {
            log::error!("srd_init failed: rc={rc}");
            return Self {
                initialized: false,
                sessions: HashMap::new(),
                next_session: 1,
            };
        }
        // Load every Python decoder into the global list once.
        let rc = unsafe { ffi::srd_decoder_load_all() };
        if rc != SRD_OK {
            log::error!("srd_decoder_load_all failed: rc={rc}");
            unsafe { ffi::srd_exit() };
            return Self {
                initialized: false,
                sessions: HashMap::new(),
                next_session: 1,
            };
        }
        Self {
            initialized: true,
            sessions: HashMap::new(),
            next_session: 1,
        }
    }

    fn require_init(&self) -> Result<()> {
        if !self.initialized {
            bail!("libsigrokdecode failed to initialize (see stderr log)");
        }
        Ok(())
    }
}

impl Drop for SrdBackend {
    fn drop(&mut self) {
        // Destroy any sessions still open, then shut down the library.
        let ids: Vec<SessionId> = self.sessions.keys().cloned().collect();
        for id in ids {
            let _ = self.destroy(&id);
        }
        if self.initialized {
            unsafe { ffi::srd_exit() };
        }
    }
}

impl Backend for SrdBackend {
    fn list_decoders(&self) -> Result<Vec<DecoderManifest>> {
        self.require_init()?;
        let mut out = Vec::new();
        unsafe {
            let list = ffi::srd_decoder_list();
            for dec_ptr in gslist_iter(list as *const ffi::GSList) {
                let dec = dec_ptr as *const ffi::srd_decoder;
                if dec.is_null() {
                    continue;
                }
                out.push(manifest_from_decoder(&*dec));
            }
        }
        Ok(out)
    }

    fn create_session(&mut self, body: CreateSessionBody) -> Result<SessionId> {
        self.require_init()?;

        let decoder_id = body
            .decoder_id
            .strip_prefix("sigrok.")
            .ok_or_else(|| anyhow!("decoder id must be of the form sigrok.<protocol>"))?
            .to_owned();

        // Frame width: highest bound channel index + 1.
        let num_channels = body
            .channels
            .values()
            .map(|&i| i as usize + 1)
            .max()
            .unwrap_or(0)
            .max(1);
        let unitsize = num_channels.div_ceil(8);

        unsafe {
            let mut sess: *mut ffi::srd_session = std::ptr::null_mut();
            if ffi::srd_session_new(&mut sess) != SRD_OK || sess.is_null() {
                bail!("srd_session_new failed for {}", body.decoder_id);
            }

            // Build the decoder instance with its option overrides.
            let opt_hash = build_options_hash(&body.options);
            let c_id = CString::new(decoder_id.as_str()).unwrap();
            let inst = ffi::srd_inst_new(sess, c_id.as_ptr(), opt_hash);
            if !opt_hash.is_null() {
                ffi::g_hash_table_destroy(opt_hash);
            }
            if inst.is_null() {
                ffi::srd_session_destroy(sess);
                bail!(
                    "srd_inst_new failed for {} (unknown decoder or bad options)",
                    body.decoder_id
                );
            }

            // Bind channels: decoder channel id -> sample-frame bit index.
            // Skip entirely when no channels are bound (passing a NULL
            // table to srd is undefined); libsigrokdecode then reports the
            // missing required channels at decode time.
            let chan_hash = build_channel_hash(&body.channels);
            if !chan_hash.is_null() {
                let rc = ffi::srd_inst_channel_set_all(inst, chan_hash);
                ffi::g_hash_table_destroy(chan_hash);
                if rc != SRD_OK {
                    ffi::srd_session_destroy(sess);
                    bail!("srd_inst_channel_set_all failed (rc={rc}); a required channel is likely unbound");
                }
            }

            // Tell libsigrokdecode our (fixed) sample rate. Many decoders
            // refuse to run without it. The floating GVariant is consumed
            // by srd_session_metadata_set.
            let rate = ffi::g_variant_new_uint64(SAMPLERATE_HZ);
            if ffi::srd_session_metadata_set(sess, SRD_CONF_SAMPLERATE, rate) != SRD_OK {
                ffi::srd_session_destroy(sess);
                bail!("srd_session_metadata_set(SAMPLERATE) failed");
            }

            // Stable accumulator the callback writes into.
            let accum: *mut Vec<RawAnnotation> = Box::into_raw(Box::new(Vec::new()));
            if ffi::srd_pd_output_callback_add(
                sess,
                SRD_OUTPUT_ANN,
                Some(annotation_cb),
                accum as *mut c_void,
            ) != SRD_OK
            {
                drop(Box::from_raw(accum));
                ffi::srd_session_destroy(sess);
                bail!("srd_pd_output_callback_add failed");
            }

            if ffi::srd_session_start(sess) != SRD_OK {
                drop(Box::from_raw(accum));
                ffi::srd_session_destroy(sess);
                bail!("srd_session_start failed for {}", body.decoder_id);
            }

            let id = format!("s{}", self.next_session);
            self.next_session += 1;
            self.sessions.insert(
                id.clone(),
                SrdSession {
                    sess,
                    num_channels,
                    unitsize,
                    levels: vec![0u8; num_channels],
                    cur_sample: 0,
                    started: false,
                    accum,
                },
            );
            Ok(id)
        }
    }

    fn feed(&mut self, body: FeedBody) -> Result<Vec<AnnotationEvent>> {
        // Note: the XZ policy is applied inline in `apply_changes`
        // (coerce-to-last); see its doc comment.
        let session = self
            .sessions
            .get_mut(&body.session)
            .ok_or_else(|| anyhow!("unknown session {}", body.session))?;

        let mut out = Vec::new();
        for batch in &body.samples {
            for tr in &batch.t {
                let sample_num = tr.fs / FS_PER_SAMPLE;
                if !session.started {
                    // First transition seeds the baseline; nothing to emit
                    // before the trace begins.
                    apply_changes(session, &tr.set);
                    session.cur_sample = sample_num;
                    session.started = true;
                } else {
                    if sample_num > session.cur_sample {
                        send_constant(session, session.cur_sample, sample_num)?;
                        session.cur_sample = sample_num;
                    }
                    apply_changes(session, &tr.set);
                }
                drain_into(session, &body.session, &mut out);
            }
        }
        Ok(out)
    }

    fn finalize(&mut self, session_id: &SessionId) -> Result<Vec<AnnotationEvent>> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| anyhow!("unknown session {session_id}"))?;

        let mut out = Vec::new();
        if session.started {
            // Flush the final held level as a single sample so the last
            // edge is observable, then reset to drain pending output.
            let from = session.cur_sample;
            send_constant(session, from, from + 1)?;
            session.cur_sample = from + 1;
            unsafe {
                ffi::srd_session_terminate_reset(session.sess);
            }
            session.started = false;
        }
        drain_into(session, session_id, &mut out);
        Ok(out)
    }

    fn destroy(&mut self, session_id: &SessionId) -> Result<()> {
        if let Some(session) = self.sessions.remove(session_id) {
            unsafe {
                ffi::srd_session_destroy(session.sess);
                drop(Box::from_raw(session.accum));
            }
        }
        Ok(())
    }
}

// ── Sample emission ──────────────────────────────────────────────────

/// Apply a transition's per-channel changes to the running level vector.
/// Indeterminate (X/Z) inputs are coerced to the previously held level —
/// libsigrokdecode has no representation for unknown bits.
fn apply_changes(session: &mut SrdSession, set: &[wavecrux_sigrok_bridge_ipc::ChannelChange]) {
    for change in set {
        let idx = change.ch as usize;
        if idx >= session.num_channels {
            continue;
        }
        match change.v {
            BitValue::Zero => session.levels[idx] = 0,
            BitValue::One => session.levels[idx] = 1,
            BitValue::X => { /* coerce-to-last: leave level unchanged */ }
        }
    }
}

/// Pack the current per-channel levels into one `unitsize`-byte sample
/// row, LSB-first (channel 0 → bit 0 of byte 0, channel 8 → bit 0 of
/// byte 1, …) — libsigrokdecode's native bit order.
fn pack_row(levels: &[u8], unitsize: usize) -> Vec<u8> {
    let mut row = vec![0u8; unitsize];
    for (idx, &lvl) in levels.iter().enumerate() {
        if lvl != 0 {
            row[idx / 8] |= 1 << (idx % 8);
        }
    }
    row
}

/// Send `[from, to)` as a run of identical sample rows carrying the
/// session's current level. Chunked so a long constant stretch never
/// allocates more than [`SEND_CHUNK_SAMPLES`] rows at once.
fn send_constant(session: &mut SrdSession, from: u64, to: u64) -> Result<()> {
    if to <= from {
        return Ok(());
    }
    let row = pack_row(&session.levels, session.unitsize);
    let mut start = from;
    while start < to {
        let n = (to - start).min(SEND_CHUNK_SAMPLES);
        let mut buf = Vec::with_capacity(n as usize * session.unitsize);
        for _ in 0..n {
            buf.extend_from_slice(&row);
        }
        let rc = unsafe {
            ffi::srd_session_send(
                session.sess,
                start,
                start + n,
                buf.as_ptr(),
                buf.len() as u64,
                session.unitsize as u64,
            )
        };
        if rc != SRD_OK {
            bail!("srd_session_send failed (rc={rc})");
        }
        start += n;
    }
    Ok(())
}

/// Move every annotation the callback accumulated into `out`, mapping
/// sample numbers back to femtoseconds.
fn drain_into(session: &SrdSession, session_id: &SessionId, out: &mut Vec<AnnotationEvent>) {
    let acc = unsafe { &mut *session.accum };
    for raw in acc.drain(..) {
        out.push(AnnotationEvent {
            session: session_id.clone(),
            start_fs: raw.start_sample.saturating_mul(FS_PER_SAMPLE),
            end_fs: raw.end_sample.saturating_mul(FS_PER_SAMPLE),
            ann_class: raw.ann_class,
            label: raw.text,
            fields: serde_json::Map::new(),
            is_error: false,
        });
    }
}

/// C callback registered for `SRD_OUTPUT_ANN`. Runs synchronously inside
/// `srd_session_send` on the same (single) thread, so it just pushes
/// into the session accumulator referenced by `cb_data`.
unsafe extern "C" fn annotation_cb(pdata: *mut ffi::srd_proto_data, cb_data: *mut c_void) {
    if pdata.is_null() || cb_data.is_null() {
        return;
    }
    let pd = &*pdata;
    let ann = pd.data as *const ffi::srd_proto_data_annotation;
    if ann.is_null() {
        return;
    }
    let ann = &*ann;
    // ann_text is a NULL-terminated array of strings; [0] is the
    // longest / primary annotation text.
    let label = if ann.ann_text.is_null() {
        String::new()
    } else {
        cstr_to_string(*ann.ann_text)
    };
    let acc = &mut *(cb_data as *mut Vec<RawAnnotation>);
    acc.push(RawAnnotation {
        start_sample: pd.start_sample,
        end_sample: pd.end_sample,
        ann_class: ann.ann_class.max(0) as u32,
        text: label,
    });
}

// ── Manifest construction ────────────────────────────────────────────

unsafe fn manifest_from_decoder(dec: &ffi::srd_decoder) -> DecoderManifest {
    let id = format!("sigrok.{}", cstr_to_string(dec.id));
    let display_name = {
        let long = cstr_to_string(dec.longname);
        if long.is_empty() {
            cstr_to_string(dec.name)
        } else {
            long
        }
    };
    let description = cstr_to_string(dec.desc);

    let mut channels = Vec::new();
    for ch in gslist_iter(dec.channels) {
        channels.push(channel_from(ch as *const ffi::srd_channel, true));
    }
    for ch in gslist_iter(dec.opt_channels) {
        channels.push(channel_from(ch as *const ffi::srd_channel, false));
    }

    let mut options = Vec::new();
    for opt in gslist_iter(dec.options) {
        if let Some(o) = option_from(opt as *const ffi::srd_decoder_option) {
            options.push(o);
        }
    }

    // Each annotation list item's `data` is a `char**`: a NULL-terminated
    // array of [short id, long description]. (The header comment calls it
    // a nested GSList, but the runtime layout is a string array.)
    let mut annotations = Vec::new();
    for ann in gslist_iter(dec.annotations) {
        let arr = ann as *const *const c_char;
        if arr.is_null() {
            continue;
        }
        let a_id = cstr_to_string(*arr);
        // Second slot is the long description (present unless the first
        // slot was already the NULL terminator).
        let a_desc = if (*arr).is_null() {
            String::new()
        } else {
            cstr_to_string(*arr.add(1))
        };
        annotations.push(DecoderAnnotationClass {
            id: a_id,
            description: a_desc,
        });
    }

    DecoderManifest {
        id,
        display_name,
        description,
        channels,
        options,
        annotations,
        // libsigrokdecode 0.5.x has no `tags` field on srd_decoder; the
        // shim seeds its category mapping from the id when tags is empty.
        tags: Vec::new(),
    }
}

unsafe fn channel_from(ch: *const ffi::srd_channel, required: bool) -> DecoderChannel {
    if ch.is_null() {
        return DecoderChannel {
            name: String::new(),
            description: String::new(),
            required,
        };
    }
    let ch = &*ch;
    DecoderChannel {
        name: cstr_to_string(ch.id),
        description: cstr_to_string(ch.desc),
        required,
    }
}

unsafe fn option_from(opt: *const ffi::srd_decoder_option) -> Option<DecoderOption> {
    if opt.is_null() {
        return None;
    }
    let opt = &*opt;
    let name = cstr_to_string(opt.id);
    let description = cstr_to_string(opt.desc);

    let choices: Vec<Value> = gslist_iter(opt.values)
        .map(|v| gvariant_to_json(v as *mut ffi::GVariant))
        .collect();
    let default = gvariant_to_json(opt.def);
    let kind = option_kind(opt.def, !choices.is_empty());

    Some(DecoderOption {
        name,
        description,
        kind,
        default,
        choices,
    })
}

/// Map a default-value GVariant's type to an [`OptionKind`].
unsafe fn option_kind(def: *mut ffi::GVariant, has_choices: bool) -> OptionKind {
    match gvariant_type(def).as_deref() {
        Some("s") => {
            if has_choices {
                OptionKind::Enum
            } else {
                OptionKind::String
            }
        }
        Some("x") | Some("i") | Some("t") | Some("u") | Some("n") | Some("q") | Some("y") => {
            OptionKind::Int
        }
        Some("d") => OptionKind::Float,
        Some("b") => OptionKind::Bool,
        _ => OptionKind::String,
    }
}

// ── GVariant <-> JSON ────────────────────────────────────────────────

unsafe fn gvariant_type(v: *mut ffi::GVariant) -> Option<String> {
    if v.is_null() {
        return None;
    }
    let ts = ffi::g_variant_get_type_string(v);
    if ts.is_null() {
        None
    } else {
        Some(cstr_to_string(ts))
    }
}

unsafe fn gvariant_to_json(v: *mut ffi::GVariant) -> Value {
    match gvariant_type(v).as_deref() {
        Some("s") => {
            let p = ffi::g_variant_get_string(v, std::ptr::null_mut());
            Value::from(cstr_to_string(p))
        }
        Some("x") => Value::from(ffi::g_variant_get_int64(v)),
        Some("i") | Some("n") => Value::from(ffi::g_variant_get_int32(v) as i64),
        Some("t") => Value::from(ffi::g_variant_get_uint64(v)),
        Some("d") => Value::from(ffi::g_variant_get_double(v)),
        Some("b") => Value::from(ffi::g_variant_get_boolean(v) != 0),
        _ => Value::Null,
    }
}

/// Build a `GVariant` from a JSON value, inferring the GVariant type
/// from the JSON type. Returns a floating reference (caller sinks).
unsafe fn json_to_gvariant(v: &Value) -> *mut ffi::GVariant {
    match v {
        Value::String(s) => {
            let c = CString::new(s.as_str()).unwrap_or_default();
            ffi::g_variant_new_string(c.as_ptr())
        }
        Value::Bool(b) => ffi::g_variant_new_boolean(*b as c_int),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                ffi::g_variant_new_int64(i)
            } else if let Some(f) = n.as_f64() {
                ffi::g_variant_new_double(f)
            } else {
                std::ptr::null_mut()
            }
        }
        _ => std::ptr::null_mut(),
    }
}

// ── GHashTable construction for srd_inst_* ───────────────────────────

/// `g_variant_unref` adapter matching the `GDestroyNotify` signature.
unsafe extern "C" fn variant_unref_cb(p: ffi::gpointer) {
    if !p.is_null() {
        ffi::g_variant_unref(p as *mut ffi::GVariant);
    }
}

/// Build the option hash table (`option id` -> `GVariant`) consumed by
/// `srd_inst_new`. Keys are g_strdup'd; values are ref-sunk. The table
/// owns both (freed by `g_hash_table_destroy` via the destroy notifies).
///
/// Always returns a non-NULL table, even when there are no overrides:
/// `srd_inst_new` only runs `srd_inst_option_set` (which populates the
/// instance's `self.options` dict with the decoder's declared defaults)
/// when it is handed a non-NULL table. Passing NULL leaves `self.options`
/// as the class-level tuple of option specs, which blows up the moment a
/// decoder does `self.options['id']`.
unsafe fn build_options_hash(options: &serde_json::Map<String, Value>) -> *mut ffi::GHashTable {
    let ht = ffi::g_hash_table_new_full(
        Some(ffi::g_str_hash),
        Some(ffi::g_str_equal),
        Some(ffi::g_free),
        Some(variant_unref_cb),
    );
    for (key, val) in options {
        let variant = json_to_gvariant(val);
        if variant.is_null() {
            continue;
        }
        let variant = ffi::g_variant_ref_sink(variant);
        let c_key = CString::new(key.as_str()).unwrap_or_default();
        let key_dup = ffi::g_strdup(c_key.as_ptr());
        ffi::g_hash_table_insert(ht, key_dup as ffi::gpointer, variant as ffi::gpointer);
    }
    ht
}

/// Build the channel hash table (`channel id` -> `GVariant(int32 index)`)
/// consumed by `srd_inst_channel_set_all`.
unsafe fn build_channel_hash(
    channels: &std::collections::BTreeMap<String, u32>,
) -> *mut ffi::GHashTable {
    if channels.is_empty() {
        return std::ptr::null_mut();
    }
    let ht = ffi::g_hash_table_new_full(
        Some(ffi::g_str_hash),
        Some(ffi::g_str_equal),
        Some(ffi::g_free),
        Some(variant_unref_cb),
    );
    for (name, &idx) in channels {
        let variant = ffi::g_variant_ref_sink(ffi::g_variant_new_int32(idx as i32));
        let c_key = CString::new(name.as_str()).unwrap_or_default();
        let key_dup = ffi::g_strdup(c_key.as_ptr());
        ffi::g_hash_table_insert(ht, key_dup as ffi::gpointer, variant as ffi::gpointer);
    }
    ht
}

// ── GLib helpers ─────────────────────────────────────────────────────

/// Collect a `GSList`'s `data` pointers into a Vec. Safe for NULL lists
/// (yields empty). Caller knows the element type.
unsafe fn gslist_iter(mut list: *const ffi::GSList) -> std::vec::IntoIter<*mut c_void> {
    let mut out = Vec::new();
    while !list.is_null() {
        out.push((*list).data);
        list = (*list).next;
    }
    out.into_iter()
}

/// Convert a C string to an owned `String`, lossily. NULL → empty.
unsafe fn cstr_to_string(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}
