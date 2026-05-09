//! IPC types and framing for the WaveCrux SigRok bridge.
//!
//! This crate is shared by the shim ([`wavecrux-sigrok-bridge-shim`])
//! and the bridge subprocess ([`wavecrux-sigrok-bridge`]). It owns:
//!
//!   * the wire-format types (request/response/event envelopes and
//!     their bodies);
//!   * the length-prefixed framing codec used by both ends.
//!
//! The crate has zero GPL dependencies. The repo as a whole is GPLv3+;
//! that license applies to this crate's source as much as to anything
//! else in the workspace, but the crates this depends on (`serde`,
//! `serde_json`, `thiserror`, `byteorder`) are MIT/Apache-2.0.
//!
//! # Wire format
//!
//! Each message on the pipe is:
//!
//! ```text
//!   <u32 little-endian byte length> <UTF-8 JSON body of that length>
//! ```
//!
//! Length is the JSON body's length in bytes, not characters. The maximum
//! single-message length is fixed at [`MAX_MESSAGE_BYTES`]; messages
//! exceeding that are rejected by the codec rather than silently
//! truncated.
//!
//! # Why length-prefixed and not newline-delimited
//!
//! Newline-delimited JSON (NDJSON) requires either escaping every
//! embedded newline or sticking to compact JSON. Compact JSON is
//! enforceable for our own emitted messages but the SigRok decoders
//! produce annotation strings that may legitimately contain `\n`. A
//! single missed escape silently desynchronizes the stream. Length
//! prefixing eliminates that class of bug at minimal cost.

pub mod codec;
pub mod schema;

pub use codec::{CodecError, FrameReader, FrameWriter};
pub use schema::*;

/// Maximum bytes in a single IPC message. Larger messages are rejected
/// at the codec layer before any JSON parsing happens.
///
/// 16 MiB is generous: a single `feed` request with thousands of
/// channel transitions across millions of timestamps tops out around
/// 1–2 MiB in practice.
pub const MAX_MESSAGE_BYTES: u32 = 16 * 1024 * 1024;
