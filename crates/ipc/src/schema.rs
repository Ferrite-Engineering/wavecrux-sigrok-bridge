//! Wire-format types for the IPC protocol.
//!
//! All types are `serde`-serialized as compact JSON. The canonical
//! schema is mirrored in `proto/ipc.schema.json`; if you add a field
//! here, update the schema file too.
//!
//! Three top-level message kinds flow over the pipe, all length-prefixed
//! and tagged by a `kind` discriminator:
//!
//!   * [`Request`] — shim → bridge (synchronous; bridge replies with
//!     [`Response`] tagged by the same `id`).
//!   * [`Response`] — bridge → shim (status + body or error).
//!   * [`Event`] — bridge → shim (unsolicited, for streaming
//!     annotations between blocking calls).
//!
//! The `kind` field is required on the wire to route incoming bytes. The
//! [`Message`] enum is the union type that both ends decode to.

use serde::{Deserialize, Serialize};

/// Top-level message envelope. Either end deserializes incoming frames
/// to this enum and dispatches by tag.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Message {
    Request(Request),
    Response(Response),
    Event(Event),
}

// ── Requests (shim → bridge) ─────────────────────────────────────────

/// A request from the shim to the bridge subprocess. Every request
/// carries a monotonically increasing `id`; the bridge's response
/// echoes the same `id`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub op: RequestOp,
}

/// Discriminated union of the operations the shim can ask the bridge
/// to perform.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", content = "body", rename_all = "snake_case")]
pub enum RequestOp {
    /// Returns the manifest list for every protocol decoder the bridge
    /// can host. Called once per subprocess lifetime.
    ListDecoders,

    /// Create a new decoder session. Returns a session id the shim uses
    /// for subsequent `feed` / `finalize` / `destroy` calls.
    CreateSession(CreateSessionBody),

    /// Feed a batch of samples for one session. Annotations are emitted
    /// asynchronously as [`Event::Annotation`] events.
    Feed(FeedBody),

    /// Flush any pending state at end-of-stream.
    Finalize(FinalizeBody),

    /// Destroy a session. The bridge releases its libsigrokdecode
    /// resources and forgets the session id.
    Destroy(DestroyBody),

    /// Politely shut down the subprocess. The bridge replies with
    /// `ok` then exits.
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateSessionBody {
    /// Decoder id the shim chose, in the form `sigrok.<protocol>`.
    pub decoder_id: String,

    /// Map from the decoder's channel name (as declared in the
    /// manifest) to a zero-based channel index in the upcoming sample
    /// frames. Required channels missing here are an error; optional
    /// channels missing here are simply unused.
    pub channels: std::collections::BTreeMap<String, u32>,

    /// Decoder option overrides. Same shape as the manifest's
    /// `parameters[*].name → value` mapping.
    #[serde(default)]
    pub options: serde_json::Map<String, serde_json::Value>,

    /// Policy for handling indeterminate (X/Z) samples. See the IPC
    /// protocol doc for the full semantics.
    #[serde(default)]
    pub xz_policy: XzPolicy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum XzPolicy {
    /// Default: emit a `glitch` annotation and skip the sample.
    /// libsigrokdecode never sees indeterminate data.
    #[default]
    Glitch,

    /// Coerce any indeterminate bit to its previous determinate value.
    /// Useful for traces with brief X-storm regions during reset.
    CoerceLast,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FeedBody {
    pub session: SessionId,
    /// Per-channel transitions in this batch. The bridge integrates
    /// these into a continuous sample stream for libsigrokdecode.
    pub samples: Vec<SampleBatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FinalizeBody {
    pub session: SessionId,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DestroyBody {
    pub session: SessionId,
}

/// Opaque session identifier. Allocated by the bridge.
pub type SessionId = String;

/// One transition record on the wire. Each batch carries N transitions
/// across one or more channels; transitions on the same timestamp
/// must be merged into a single [`SampleTransition`] with all changed
/// channels named.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SampleBatch {
    /// Transitions in time order. `t` is femtoseconds since the
    /// session origin (matching `WcSample.timestamp_fs` in the C ABI).
    pub t: Vec<SampleTransition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SampleTransition {
    /// Femtoseconds.
    pub fs: u64,

    /// Channels that *changed* at this timestamp. Other channels keep
    /// their previous level. Encoded as a list (not a map) because JSON
    /// object keys are always strings — and we need integer channel
    /// indices to round-trip cleanly.
    pub set: Vec<ChannelChange>,
}

/// One channel transitioning to a new bit value at a [`SampleTransition`]'s
/// timestamp.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelChange {
    pub ch: u32,
    pub v: BitValue,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BitValue {
    Zero,
    One,
    /// Indeterminate (X or Z). The shim collapses both to `x` because
    /// libsigrokdecode does not distinguish them.
    X,
}

// ── Responses (bridge → shim) ────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Response {
    pub id: u64,
    #[serde(flatten)]
    pub status: ResponseStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResponseStatus {
    Ok {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<ResponseBody>,
    },
    Err {
        code: ErrCode,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ResponseBody {
    Decoders(Vec<DecoderManifest>),
    SessionCreated { session: SessionId },
    Empty {},
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrCode {
    /// The decoder id was not found in the bridge's catalog.
    UnknownDecoder,
    /// The session id was not found.
    UnknownSession,
    /// The bridge could not build the requested session — a required
    /// channel was missing, an option failed validation, etc.
    SessionInitFailed,
    /// libsigrokdecode rejected a sample. Carries the libsigrokdecode
    /// error string in `message`.
    DecoderFailed,
    /// The bridge entered an unrecoverable internal state. The shim
    /// should mark every session failed and re-spawn the subprocess.
    Internal,
    /// The request was malformed (parsed but semantically invalid).
    BadRequest,
}

// ── Events (bridge → shim, unsolicited) ──────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", content = "body", rename_all = "snake_case")]
pub enum Event {
    /// One decoded annotation.
    Annotation(AnnotationEvent),

    /// The bridge encountered a non-fatal anomaly. The shim may surface
    /// this into the WaveCrux diagnostics log.
    Log(LogEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnnotationEvent {
    pub session: SessionId,

    /// Femtoseconds — start time on the WaveCrux timeline. This is
    /// computed by the bridge by mapping libsigrokdecode's tick-based
    /// timestamps back through the session's femtosecond-per-tick
    /// scaling.
    pub start_fs: u64,
    pub end_fs: u64,

    /// Annotation class index, as defined by the upstream decoder's
    /// `annotations` tuple. Useful for filtering or color-mapping in
    /// future UI.
    pub ann_class: u32,

    /// Short user-facing label.
    pub label: String,

    /// Optional structured-fields object. The bridge populates this
    /// with the upstream decoder's `annotation_rows` style data when
    /// available; otherwise it is empty.
    #[serde(default)]
    pub fields: serde_json::Map<String, serde_json::Value>,

    /// Whether the upstream decoder flagged this annotation as a
    /// protocol violation. Maps to `WcTransaction.is_error`.
    #[serde(default)]
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogEvent {
    pub session: Option<SessionId>,
    pub level: LogLevel,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

// ── Decoder manifest (returned by ListDecoders) ──────────────────────

/// One entry in the bridge's decoder catalog. The shim translates each
/// of these into a `WcDecoderDef` that the WaveCrux loader registers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DecoderManifest {
    /// Decoder id, in the form `sigrok.<protocol>`. This becomes the
    /// `WcDecoderDef.id` and the user-visible identifier in WaveCrux.
    pub id: String,

    /// Short user-facing name (e.g. "1-Wire", "JTAG").
    pub display_name: String,

    /// One-paragraph description, suitable for tooltip / picker.
    pub description: String,

    /// Channels the decoder needs. Required channels must be bound;
    /// optional channels may be left unbound.
    pub channels: Vec<DecoderChannel>,

    /// Configurable options the decoder exposes.
    pub options: Vec<DecoderOption>,

    /// Annotation classes the decoder can emit. Index in this list is
    /// the `ann_class` reported on each [`AnnotationEvent`].
    pub annotations: Vec<DecoderAnnotationClass>,

    /// Free-form tags from libsigrokdecode (`tags` field on
    /// `srd_decoder`). Used by the shim only to seed WaveCrux's
    /// `DecoderCategory` mapping.
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DecoderChannel {
    /// Decoder-internal short name (e.g. `tck`, `data`).
    pub name: String,
    /// User-facing description.
    pub description: String,
    /// Whether the channel is required.
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DecoderOption {
    pub name: String,
    pub description: String,
    pub kind: OptionKind,
    pub default: serde_json::Value,
    /// Discrete-choice options enumerate their values here. Empty for
    /// free-form numeric / string options.
    #[serde(default)]
    pub choices: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OptionKind {
    String,
    Int,
    Float,
    Bool,
    Enum,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DecoderAnnotationClass {
    /// Stable short id (matches the upstream decoder).
    pub id: String,
    /// User-facing description.
    pub description: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn round_trip(msg: &Message) {
        let s = serde_json::to_string(msg).unwrap();
        let back: Message = serde_json::from_str(&s).unwrap();
        assert_eq!(*msg, back);
    }

    #[test]
    fn round_trips_list_decoders_request() {
        round_trip(&Message::Request(Request {
            id: 1,
            op: RequestOp::ListDecoders,
        }));
    }

    #[test]
    fn round_trips_create_session_request() {
        let mut channels = BTreeMap::new();
        channels.insert("tck".into(), 0);
        channels.insert("tms".into(), 1);
        let req = Request {
            id: 7,
            op: RequestOp::CreateSession(CreateSessionBody {
                decoder_id: "sigrok.jtag".into(),
                channels,
                options: serde_json::Map::new(),
                xz_policy: XzPolicy::CoerceLast,
            }),
        };
        round_trip(&Message::Request(req));
    }

    #[test]
    fn round_trips_feed_request_with_mixed_values() {
        let req = Request {
            id: 9,
            op: RequestOp::Feed(FeedBody {
                session: "s1".into(),
                samples: vec![SampleBatch {
                    t: vec![
                        SampleTransition {
                            fs: 0,
                            set: vec![
                                ChannelChange {
                                    ch: 0,
                                    v: BitValue::One,
                                },
                                ChannelChange {
                                    ch: 1,
                                    v: BitValue::Zero,
                                },
                            ],
                        },
                        SampleTransition {
                            fs: 100_000,
                            set: vec![ChannelChange {
                                ch: 0,
                                v: BitValue::X,
                            }],
                        },
                    ],
                }],
            }),
        };
        round_trip(&Message::Request(req));
    }

    #[test]
    fn round_trips_response_ok_with_decoder_list() {
        let resp = Response {
            id: 1,
            status: ResponseStatus::Ok {
                body: Some(ResponseBody::Decoders(vec![DecoderManifest {
                    id: "sigrok.onewire".into(),
                    display_name: "1-Wire".into(),
                    description: "Maxim 1-Wire bus".into(),
                    channels: vec![DecoderChannel {
                        name: "data".into(),
                        description: "Data line".into(),
                        required: true,
                    }],
                    options: vec![],
                    annotations: vec![DecoderAnnotationClass {
                        id: "rom".into(),
                        description: "ROM commands".into(),
                    }],
                    tags: vec!["embedded".into()],
                }])),
            },
        };
        round_trip(&Message::Response(resp));
    }

    #[test]
    fn round_trips_response_err() {
        let resp = Response {
            id: 9,
            status: ResponseStatus::Err {
                code: ErrCode::UnknownDecoder,
                message: "no such decoder: sigrok.foo".into(),
            },
        };
        round_trip(&Message::Response(resp));
    }

    #[test]
    fn round_trips_annotation_event() {
        let mut fields = serde_json::Map::new();
        fields.insert("opcode".into(), json!("0x06"));
        fields.insert("ir_value".into(), json!(0x06));
        let evt = Event::Annotation(AnnotationEvent {
            session: "s1".into(),
            start_fs: 1_000_000,
            end_fs: 2_000_000,
            ann_class: 2,
            label: "IR=0x06 SAMPLE/PRELOAD".into(),
            fields,
            is_error: false,
        });
        round_trip(&Message::Event(evt));
    }

    #[test]
    fn xz_policy_defaults_to_glitch_when_omitted() {
        let s = r#"{"kind":"request","id":1,"op":"create_session","body":{"decoder_id":"sigrok.onewire","channels":{"data":0}}}"#;
        let msg: Message = serde_json::from_str(s).unwrap();
        match msg {
            Message::Request(Request {
                op: RequestOp::CreateSession(body),
                ..
            }) => assert_eq!(body.xz_policy, XzPolicy::Glitch),
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn external_compatibility_fixed_layout_for_request() {
        // Snapshot of the wire format. If this changes, IPC_PROTOCOL.md
        // and the schema file must change in the same commit.
        let req = Request {
            id: 42,
            op: RequestOp::Shutdown,
        };
        let s = serde_json::to_string(&Message::Request(req)).unwrap();
        assert_eq!(s, r#"{"kind":"request","id":42,"op":"shutdown"}"#);
    }
}
