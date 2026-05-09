//! Mock decoder backend used when the `sigrok` feature is off.
//!
//! The mock advertises the five reference decoders and replays
//! deterministic, scripted annotations against the committed fixtures.
//! It exists so the entire test suite runs without libsigrokdecode or
//! libpython installed and so CI can verify the IPC path end-to-end on
//! every push.
//!
//! The annotations the mock emits are intentionally aligned with the
//! `expected_transactions.json` companions in `test/fixtures/`. Real
//! libsigrokdecode output may differ slightly in label wording; the
//! verification guide flags this and treats the mock as the
//! authoritative shape for IPC contract testing.

use std::collections::HashMap;

use anyhow::{anyhow, Result};

use wavecrux_sigrok_bridge_ipc::{
    AnnotationEvent, BitValue, CreateSessionBody, DecoderAnnotationClass, DecoderChannel,
    DecoderManifest, DecoderOption, FeedBody, OptionKind, SessionId,
};

use crate::backend::Backend;

pub(crate) struct MockBackend {
    sessions: HashMap<SessionId, Session>,
    next_session: u64,
}

struct Session {
    /// Tracked for diagnostics-mode introspection; not read by the
    /// mock decode loop itself (the per-decoder modules below switch
    /// on `state`'s enum variant).
    #[allow(dead_code)]
    decoder_id: String,
    /// Channel binding map. Reserved for future use when the mock
    /// expands to honor specific channel indices.
    #[allow(dead_code)]
    channels: std::collections::BTreeMap<String, u32>,
    last_levels: HashMap<u32, BitValue>,
    state: DecoderState,
}

#[derive(Default)]
enum DecoderState {
    #[default]
    Idle,
    OneWire(onewire::State),
    Jtag(jtag::State),
    Pwm(pwm::State),
    Dmx(dmx::State),
    Modbus(modbus::State),
}

impl MockBackend {
    pub(crate) fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            next_session: 1,
        }
    }
}

impl Backend for MockBackend {
    fn list_decoders(&self) -> Result<Vec<DecoderManifest>> {
        Ok(reference_decoder_manifests())
    }

    fn create_session(&mut self, body: CreateSessionBody) -> Result<SessionId> {
        let manifests = reference_decoder_manifests();
        let manifest = manifests
            .iter()
            .find(|m| m.id == body.decoder_id)
            .ok_or_else(|| anyhow!("unknown decoder {}", body.decoder_id))?;
        for ch in &manifest.channels {
            if ch.required && !body.channels.contains_key(&ch.name) {
                return Err(anyhow!("required channel {} unbound", ch.name));
            }
        }
        let id = format!("s{}", self.next_session);
        self.next_session += 1;
        let state = match body.decoder_id.as_str() {
            "sigrok.onewire" => DecoderState::OneWire(onewire::State::default()),
            "sigrok.jtag" => DecoderState::Jtag(jtag::State::default()),
            "sigrok.pwm" => DecoderState::Pwm(pwm::State::default()),
            "sigrok.dmx512" => DecoderState::Dmx(dmx::State::default()),
            "sigrok.modbus" => DecoderState::Modbus(modbus::State::default()),
            _ => DecoderState::Idle,
        };
        self.sessions.insert(
            id.clone(),
            Session {
                decoder_id: body.decoder_id,
                channels: body.channels,
                last_levels: HashMap::new(),
                state,
            },
        );
        Ok(id)
    }

    fn feed(&mut self, body: FeedBody) -> Result<Vec<AnnotationEvent>> {
        let session = self
            .sessions
            .get_mut(&body.session)
            .ok_or_else(|| anyhow!("unknown session"))?;

        let mut emit = Vec::new();
        for batch in body.samples {
            for transition in batch.t {
                for change in &transition.set {
                    session.last_levels.insert(change.ch, change.v);
                }
                let evs = match &mut session.state {
                    DecoderState::OneWire(st) => {
                        onewire::on_transition(st, transition.fs, &session.last_levels)
                    }
                    DecoderState::Jtag(st) => {
                        jtag::on_transition(st, transition.fs, &session.last_levels)
                    }
                    DecoderState::Pwm(st) => {
                        pwm::on_transition(st, transition.fs, &session.last_levels)
                    }
                    DecoderState::Dmx(st) => {
                        dmx::on_transition(st, transition.fs, &session.last_levels)
                    }
                    DecoderState::Modbus(st) => {
                        modbus::on_transition(st, transition.fs, &session.last_levels)
                    }
                    DecoderState::Idle => vec![],
                };
                emit.extend(evs);
            }
        }
        Ok(emit
            .into_iter()
            .map(|a| AnnotationEvent {
                session: body.session.clone(),
                ..a
            })
            .collect())
    }

    fn finalize(&mut self, session_id: &SessionId) -> Result<Vec<AnnotationEvent>> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| anyhow!("unknown session"))?;
        let evs = match &mut session.state {
            DecoderState::OneWire(st) => onewire::finalize(st),
            DecoderState::Jtag(st) => jtag::finalize(st),
            DecoderState::Pwm(st) => pwm::finalize(st),
            DecoderState::Dmx(st) => dmx::finalize(st),
            DecoderState::Modbus(st) => modbus::finalize(st),
            DecoderState::Idle => vec![],
        };
        Ok(evs
            .into_iter()
            .map(|a| AnnotationEvent {
                session: session_id.clone(),
                ..a
            })
            .collect())
    }

    fn destroy(&mut self, session_id: &SessionId) -> Result<()> {
        self.sessions.remove(session_id);
        Ok(())
    }
}

/// The five reference decoders the bridge advertises. Channel and
/// option layouts mirror libsigrokdecode upstream so swapping the mock
/// backend for the real one is a drop-in replacement at the IPC layer.
pub(crate) fn reference_decoder_manifests() -> Vec<DecoderManifest> {
    vec![
        DecoderManifest {
            id: "sigrok.onewire".into(),
            display_name: "1-Wire".into(),
            description: "Maxim Integrated 1-Wire serial bus.".into(),
            channels: vec![DecoderChannel {
                name: "data".into(),
                description: "1-Wire bidirectional data line.".into(),
                required: true,
            }],
            options: vec![],
            annotations: vec![
                DecoderAnnotationClass {
                    id: "reset".into(),
                    description: "Reset / presence pulse".into(),
                },
                DecoderAnnotationClass {
                    id: "rom".into(),
                    description: "ROM command".into(),
                },
                DecoderAnnotationClass {
                    id: "data".into(),
                    description: "Data byte".into(),
                },
            ],
            tags: vec!["embedded".into(), "serial".into()],
        },
        DecoderManifest {
            id: "sigrok.jtag".into(),
            display_name: "JTAG".into(),
            description: "IEEE 1149.1 JTAG TAP controller.".into(),
            channels: vec![
                DecoderChannel {
                    name: "tck".into(),
                    description: "Test clock".into(),
                    required: true,
                },
                DecoderChannel {
                    name: "tms".into(),
                    description: "Test mode select".into(),
                    required: true,
                },
                DecoderChannel {
                    name: "tdi".into(),
                    description: "Test data in".into(),
                    required: true,
                },
                DecoderChannel {
                    name: "tdo".into(),
                    description: "Test data out".into(),
                    required: true,
                },
            ],
            options: vec![],
            annotations: vec![
                DecoderAnnotationClass {
                    id: "tap_state".into(),
                    description: "TAP controller state".into(),
                },
                DecoderAnnotationClass {
                    id: "ir".into(),
                    description: "IR scan".into(),
                },
                DecoderAnnotationClass {
                    id: "dr".into(),
                    description: "DR scan".into(),
                },
            ],
            tags: vec!["debug".into(), "test".into()],
        },
        DecoderManifest {
            id: "sigrok.pwm".into(),
            display_name: "PWM".into(),
            description: "Pulse-width modulation analyzer.".into(),
            channels: vec![DecoderChannel {
                name: "signal".into(),
                description: "Pulse-width-modulated signal".into(),
                required: true,
            }],
            options: vec![DecoderOption {
                name: "polarity".into(),
                description: "Active level (active-high or active-low)".into(),
                kind: OptionKind::Enum,
                default: serde_json::json!("active-high"),
                choices: vec![
                    serde_json::json!("active-high"),
                    serde_json::json!("active-low"),
                ],
            }],
            annotations: vec![
                DecoderAnnotationClass {
                    id: "duty".into(),
                    description: "Duty cycle".into(),
                },
                DecoderAnnotationClass {
                    id: "frequency".into(),
                    description: "Frequency".into(),
                },
            ],
            tags: vec!["analog".into(), "embedded".into()],
        },
        DecoderManifest {
            id: "sigrok.dmx512".into(),
            display_name: "DMX512".into(),
            description: "USITT DMX512 stage lighting protocol.".into(),
            channels: vec![DecoderChannel {
                name: "data".into(),
                description: "DMX data line".into(),
                required: true,
            }],
            options: vec![],
            annotations: vec![
                DecoderAnnotationClass {
                    id: "break".into(),
                    description: "BREAK".into(),
                },
                DecoderAnnotationClass {
                    id: "mab".into(),
                    description: "Mark after break".into(),
                },
                DecoderAnnotationClass {
                    id: "slot".into(),
                    description: "DMX slot".into(),
                },
            ],
            tags: vec!["lighting".into(), "serial".into()],
        },
        DecoderManifest {
            id: "sigrok.modbus".into(),
            display_name: "Modbus".into(),
            description: "Modbus RTU serial protocol.".into(),
            channels: vec![DecoderChannel {
                name: "rx".into(),
                description: "Receive line".into(),
                required: true,
            }],
            options: vec![DecoderOption {
                name: "baudrate".into(),
                description: "UART baud rate".into(),
                kind: OptionKind::Int,
                default: serde_json::json!(9600),
                choices: vec![],
            }],
            annotations: vec![
                DecoderAnnotationClass {
                    id: "frame".into(),
                    description: "Frame".into(),
                },
                DecoderAnnotationClass {
                    id: "function".into(),
                    description: "Function code".into(),
                },
                DecoderAnnotationClass {
                    id: "crc".into(),
                    description: "CRC".into(),
                },
            ],
            tags: vec!["industrial".into(), "serial".into()],
        },
    ]
}

// ── Per-decoder mock state machines ──────────────────────────────────
//
// These produce annotations driven by edges on the channel(s) the
// fixture replays. They are not fully spec-correct decoders — they
// produce *exactly* the annotations the committed fixtures expect, no
// more. When the `sigrok` feature is on, the real libsigrokdecode
// engine takes over and the mock is unused.

mod onewire {
    use super::*;
    use serde_json::json;

    #[derive(Default)]
    pub(super) struct State {
        emitted: bool,
    }

    pub(super) fn on_transition(
        st: &mut State,
        fs: u64,
        levels: &HashMap<u32, BitValue>,
    ) -> Vec<AnnotationEvent> {
        // The 1-Wire fixture is small enough that we emit the full
        // annotation set on first transition and ignore the rest. This
        // matches `onewire_basic.expected_transactions.json`.
        if st.emitted {
            return vec![];
        }
        if levels.is_empty() {
            return vec![];
        }
        st.emitted = true;
        vec![
            ann(fs, fs + 480_000_000, 0, "RESET / Presence", json!({})),
            ann(
                fs + 480_000_000,
                fs + 540_000_000,
                1,
                "READ ROM (0x33)",
                json!({"opcode":"0x33"}),
            ),
            ann(
                fs + 540_000_000,
                fs + 1_020_000_000,
                2,
                "ROM 0x28 0xFF 0x46 0x9D 0x01 0x16 0x03 0xC2",
                json!({"family":"0x28","crc":"0xC2","ok":true}),
            ),
        ]
    }

    pub(super) fn finalize(_: &mut State) -> Vec<AnnotationEvent> {
        vec![]
    }
}

mod jtag {
    use super::*;
    use serde_json::json;

    #[derive(Default)]
    pub(super) struct State {
        emitted: bool,
    }

    pub(super) fn on_transition(
        st: &mut State,
        fs: u64,
        levels: &HashMap<u32, BitValue>,
    ) -> Vec<AnnotationEvent> {
        if st.emitted || levels.is_empty() {
            return vec![];
        }
        st.emitted = true;
        vec![
            ann(fs, fs + 1_000_000_000, 0, "TEST-LOGIC-RESET", json!({})),
            ann(
                fs + 1_000_000_000,
                fs + 2_000_000_000,
                1,
                "IR=0x09 IDCODE",
                json!({"opcode":"0x09"}),
            ),
            ann(
                fs + 2_000_000_000,
                fs + 3_000_000_000,
                2,
                "DR=0x1234ABCD",
                json!({"value":"0x1234abcd"}),
            ),
        ]
    }

    pub(super) fn finalize(_: &mut State) -> Vec<AnnotationEvent> {
        vec![]
    }
}

mod pwm {
    use super::*;
    use serde_json::json;

    #[derive(Default)]
    pub(super) struct State {
        emitted: bool,
    }

    pub(super) fn on_transition(
        st: &mut State,
        fs: u64,
        levels: &HashMap<u32, BitValue>,
    ) -> Vec<AnnotationEvent> {
        if st.emitted || levels.is_empty() {
            return vec![];
        }
        st.emitted = true;
        vec![
            ann(
                fs,
                fs + 5_000_000_000,
                0,
                "duty=25.0%",
                json!({"duty_pct":25.0,"frequency_hz":1000.0}),
            ),
            ann(
                fs + 5_000_000_000,
                fs + 10_000_000_000,
                0,
                "duty=75.0%",
                json!({"duty_pct":75.0,"frequency_hz":1000.0}),
            ),
        ]
    }

    pub(super) fn finalize(_: &mut State) -> Vec<AnnotationEvent> {
        vec![]
    }
}

mod dmx {
    use super::*;
    use serde_json::json;

    #[derive(Default)]
    pub(super) struct State {
        emitted: bool,
    }

    pub(super) fn on_transition(
        st: &mut State,
        fs: u64,
        levels: &HashMap<u32, BitValue>,
    ) -> Vec<AnnotationEvent> {
        if st.emitted || levels.is_empty() {
            return vec![];
        }
        st.emitted = true;
        vec![
            ann(fs, fs + 88_000_000, 0, "BREAK", json!({})),
            ann(fs + 88_000_000, fs + 96_000_000, 1, "MAB", json!({})),
            ann(
                fs + 96_000_000,
                fs + 140_000_000,
                2,
                "Slot 0 = 0x00 (start code)",
                json!({"slot":0,"value":0}),
            ),
            ann(
                fs + 140_000_000,
                fs + 184_000_000,
                2,
                "Slot 1 = 0xFF",
                json!({"slot":1,"value":255}),
            ),
            ann(
                fs + 184_000_000,
                fs + 228_000_000,
                2,
                "Slot 2 = 0x80",
                json!({"slot":2,"value":128}),
            ),
        ]
    }

    pub(super) fn finalize(_: &mut State) -> Vec<AnnotationEvent> {
        vec![]
    }
}

mod modbus {
    use super::*;
    use serde_json::json;

    #[derive(Default)]
    pub(super) struct State {
        emitted: bool,
    }

    pub(super) fn on_transition(
        st: &mut State,
        fs: u64,
        levels: &HashMap<u32, BitValue>,
    ) -> Vec<AnnotationEvent> {
        if st.emitted || levels.is_empty() {
            return vec![];
        }
        st.emitted = true;
        vec![
            ann(
                fs,
                fs + 4_000_000_000,
                0,
                "Read Holding Registers",
                json!({"slave":1,"function":3,"start":1,"count":2}),
            ),
            ann(fs, fs + 1_000_000_000, 1, "fn=0x03", json!({"function":3})),
            ann(
                fs + 3_500_000_000,
                fs + 4_000_000_000,
                2,
                "CRC ok",
                json!({"ok":true,"crc":"0xAABB"}),
            ),
        ]
    }

    pub(super) fn finalize(_: &mut State) -> Vec<AnnotationEvent> {
        vec![]
    }
}

fn ann(
    start_fs: u64,
    end_fs: u64,
    ann_class: u32,
    label: &str,
    fields: serde_json::Value,
) -> AnnotationEvent {
    let map = match fields {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    AnnotationEvent {
        session: String::new(),
        start_fs,
        end_fs,
        ann_class,
        label: label.to_owned(),
        fields: map,
        is_error: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertises_five_decoders() {
        let m = reference_decoder_manifests();
        assert_eq!(m.len(), 5);
        let ids: Vec<_> = m.iter().map(|d| d.id.clone()).collect();
        assert!(ids.contains(&"sigrok.onewire".to_string()));
        assert!(ids.contains(&"sigrok.jtag".to_string()));
        assert!(ids.contains(&"sigrok.pwm".to_string()));
        assert!(ids.contains(&"sigrok.dmx512".to_string()));
        assert!(ids.contains(&"sigrok.modbus".to_string()));
    }

    #[test]
    fn rejects_unknown_decoder_in_create_session() {
        let mut be = MockBackend::new();
        let body = CreateSessionBody {
            decoder_id: "sigrok.totallyfake".into(),
            channels: Default::default(),
            options: Default::default(),
            xz_policy: Default::default(),
        };
        assert!(be.create_session(body).is_err());
    }

    #[test]
    fn rejects_missing_required_channel() {
        let mut be = MockBackend::new();
        let body = CreateSessionBody {
            decoder_id: "sigrok.onewire".into(),
            channels: Default::default(),
            options: Default::default(),
            xz_policy: Default::default(),
        };
        assert!(be.create_session(body).is_err());
    }

    #[test]
    fn opens_and_feeds_a_minimal_onewire_session() {
        let mut be = MockBackend::new();
        let mut chans = std::collections::BTreeMap::new();
        chans.insert("data".into(), 0);
        let id = be
            .create_session(CreateSessionBody {
                decoder_id: "sigrok.onewire".into(),
                channels: chans,
                options: Default::default(),
                xz_policy: Default::default(),
            })
            .unwrap();
        let evs = be
            .feed(FeedBody {
                session: id.clone(),
                samples: vec![wavecrux_sigrok_bridge_ipc::SampleBatch {
                    t: vec![wavecrux_sigrok_bridge_ipc::SampleTransition {
                        fs: 0,
                        set: vec![wavecrux_sigrok_bridge_ipc::ChannelChange {
                            ch: 0,
                            v: BitValue::Zero,
                        }],
                    }],
                }],
            })
            .unwrap();
        assert!(
            !evs.is_empty(),
            "mock should emit annotations on first edge"
        );
        assert_eq!(evs[0].session, id);
    }
}
