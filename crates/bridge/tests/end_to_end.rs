//! End-to-end integration test: spawn the real `wavecrux-sigrok-bridge`
//! binary in `--ipc` mode and drive it through every protocol-level
//! operation against the committed mock decoders.
//!
//! This test is the production-grade conformance test for the IPC
//! contract. It runs in default-feature mode (mock backend) on every
//! CI build, and it is the harness the `sigrok` feature will hook into
//! to verify libsigrokdecode-driven runs once that backend lights up.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use wavecrux_sigrok_bridge_ipc::{
    BitValue, ChannelChange, CreateSessionBody, DecoderManifest, DestroyBody, Event, FeedBody,
    FinalizeBody, FrameReader, FrameWriter, Message, Request, RequestOp, Response, ResponseBody,
    ResponseStatus, SampleBatch, SampleTransition, XzPolicy,
};

fn bridge_binary() -> PathBuf {
    // Cargo sets `CARGO_BIN_EXE_<name>` for every binary in the same
    // package as a test target.
    PathBuf::from(env!("CARGO_BIN_EXE_wavecrux-sigrok-bridge"))
}

struct Harness {
    child: std::process::Child,
    writer: FrameWriter<std::process::ChildStdin>,
    reader: FrameReader<std::process::ChildStdout>,
}

impl Harness {
    fn spawn() -> Self {
        let mut child = Command::new(bridge_binary())
            .arg("--ipc")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn bridge subprocess");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        Self {
            child,
            writer: FrameWriter::new(stdin),
            reader: FrameReader::new(stdout),
        }
    }

    fn request(&mut self, id: u64, op: RequestOp) -> Response {
        let body = serde_json::to_vec(&Message::Request(Request { id, op })).expect("encode");
        self.writer.write_frame(&body).expect("write frame");
        loop {
            let frame = self.reader.read_frame().expect("read frame");
            let msg: Message = serde_json::from_slice(&frame).expect("decode");
            match msg {
                Message::Response(r) if r.id == id => return r,
                Message::Response(r) => panic!("orphan response id={}", r.id),
                Message::Event(_) => continue,
                Message::Request(_) => panic!("subprocess sent a request"),
            }
        }
    }

    fn shutdown(mut self) {
        let body = serde_json::to_vec(&Message::Request(Request {
            id: 0,
            op: RequestOp::Shutdown,
        }))
        .expect("encode shutdown");
        let _ = self.writer.write_frame(&body);
        // Drain any final events and the shutdown response.
        let _ = self.reader.read_frame();
        let mut stderr = self
            .child
            .stderr
            .take()
            .expect("stderr captured but not yet read");
        let _ = self.child.wait();
        // Read any stderr output for diagnostic surfacing on test
        // failure (the test runner displays it).
        let mut buf = String::new();
        let _ = stderr.read_to_string(&mut buf);
        if !buf.is_empty() {
            eprintln!("bridge stderr:\n{buf}");
        }
    }
}

fn request_and_drain(h: &mut Harness, id: u64, op: RequestOp) -> (Response, Vec<Event>) {
    let mut events = vec![];
    let body = serde_json::to_vec(&Message::Request(Request { id, op })).expect("encode");
    h.writer.write_frame(&body).expect("write");
    loop {
        let frame = h.reader.read_frame().expect("read");
        let msg: Message = serde_json::from_slice(&frame).expect("decode");
        match msg {
            Message::Response(r) if r.id == id => return (r, events),
            Message::Response(r) => panic!("orphan id={}", r.id),
            Message::Event(e) => events.push(e),
            Message::Request(_) => panic!("subprocess sent a request"),
        }
    }
}

// The mock backend advertises exactly the five reference decoders with
// scripted annotations. These assertions are specific to that backend;
// under `--features sigrok` the binary hosts the real ~130-decoder
// libsigrokdecode corpus instead, so they are gated off and replaced by
// the `sigrok_*` tests at the bottom of this file.
#[cfg(not(feature = "sigrok"))]
#[test]
fn lists_the_five_reference_decoders() {
    let mut h = Harness::spawn();
    let resp = h.request(1, RequestOp::ListDecoders);
    let decoders: Vec<DecoderManifest> = match resp.status {
        ResponseStatus::Ok {
            body: Some(ResponseBody::Decoders(list)),
        } => list,
        other => panic!("unexpected response: {other:?}"),
    };
    let ids: Vec<_> = decoders.iter().map(|d| d.id.clone()).collect();
    assert!(ids.contains(&"sigrok.onewire".to_string()));
    assert!(ids.contains(&"sigrok.jtag".to_string()));
    assert!(ids.contains(&"sigrok.pwm".to_string()));
    assert!(ids.contains(&"sigrok.dmx512".to_string()));
    assert!(ids.contains(&"sigrok.modbus".to_string()));
    h.shutdown();
}

#[test]
fn rejects_unknown_decoder_with_error_response() {
    let mut h = Harness::spawn();
    let mut channels = std::collections::BTreeMap::new();
    channels.insert("data".into(), 0);
    let resp = h.request(
        1,
        RequestOp::CreateSession(CreateSessionBody {
            decoder_id: "sigrok.nope".into(),
            channels,
            options: Default::default(),
            xz_policy: XzPolicy::Glitch,
        }),
    );
    match resp.status {
        ResponseStatus::Err { .. } => {}
        other => panic!("expected error, got {other:?}"),
    }
    h.shutdown();
}

// Mock-specific: the mock validates required channels up front. Real
// libsigrokdecode reports a missing required channel at decode time, not
// at session creation, so this exact shape only holds for the mock.
#[cfg(not(feature = "sigrok"))]
#[test]
fn rejects_required_channel_missing_for_jtag() {
    let mut h = Harness::spawn();
    let resp = h.request(
        1,
        RequestOp::CreateSession(CreateSessionBody {
            decoder_id: "sigrok.jtag".into(),
            channels: Default::default(),
            options: Default::default(),
            xz_policy: XzPolicy::Glitch,
        }),
    );
    match resp.status {
        ResponseStatus::Err { .. } => {}
        other => panic!("expected error, got {other:?}"),
    }
    h.shutdown();
}

// Mock-specific: depends on the `sigrok.onewire` mock decoder (which is
// not a real libsigrokdecode id — upstream has onewire_link /
// onewire_network) and its scripted three-annotation output.
#[cfg(not(feature = "sigrok"))]
#[test]
fn full_session_roundtrip_for_onewire_emits_three_annotations() {
    let mut h = Harness::spawn();
    let mut channels = std::collections::BTreeMap::new();
    channels.insert("data".into(), 0);
    let resp = h.request(
        1,
        RequestOp::CreateSession(CreateSessionBody {
            decoder_id: "sigrok.onewire".into(),
            channels,
            options: Default::default(),
            xz_policy: XzPolicy::Glitch,
        }),
    );
    let session = match resp.status {
        ResponseStatus::Ok {
            body: Some(ResponseBody::SessionCreated { session }),
        } => session,
        other => panic!("create_session failed: {other:?}"),
    };

    // Feed a single transition. The mock backend emits the full
    // scripted annotation set on the first transition it sees.
    let (_, events) = request_and_drain(
        &mut h,
        2,
        RequestOp::Feed(FeedBody {
            session: session.clone(),
            samples: vec![SampleBatch {
                t: vec![SampleTransition {
                    fs: 0,
                    set: vec![ChannelChange {
                        ch: 0,
                        v: BitValue::Zero,
                    }],
                }],
            }],
        }),
    );
    let annotations: Vec<_> = events
        .into_iter()
        .filter_map(|e| match e {
            Event::Annotation(a) => Some(a),
            _ => None,
        })
        .collect();
    assert_eq!(annotations.len(), 3, "expected 3 mock annotations");
    assert!(annotations.iter().all(|a| a.session == session));
    assert_eq!(annotations[0].label, "RESET / Presence");
    assert!(annotations[1].label.contains("READ ROM"));
    assert!(annotations[2].label.starts_with("ROM 0x28"));

    // Finalize then destroy.
    let resp = h.request(
        3,
        RequestOp::Finalize(FinalizeBody {
            session: session.clone(),
        }),
    );
    assert!(matches!(resp.status, ResponseStatus::Ok { .. }));
    let resp = h.request(4, RequestOp::Destroy(DestroyBody { session }));
    assert!(matches!(resp.status, ResponseStatus::Ok { .. }));

    h.shutdown();
}

#[test]
fn shutdown_request_terminates_subprocess() {
    let mut h = Harness::spawn();
    // List once to confirm liveness.
    let _ = h.request(1, RequestOp::ListDecoders);
    let body = serde_json::to_vec(&Message::Request(Request {
        id: 0,
        op: RequestOp::Shutdown,
    }))
    .unwrap();
    h.writer.write_frame(&body).expect("write");
    // Read the shutdown response.
    let frame = h.reader.read_frame().expect("read");
    let msg: Message = serde_json::from_slice(&frame).expect("decode");
    assert!(matches!(msg, Message::Response(_)));
    let status = h.child.wait().expect("wait");
    assert!(status.success(), "subprocess should exit cleanly");
}

// ── sigrok-feature tests ─────────────────────────────────────────────
//
// These run only when the binary is built against real libsigrokdecode.
// They assert the contract the IPC layer must uphold against the live
// decoder corpus, rather than the mock's scripted annotations.

#[cfg(feature = "sigrok")]
#[test]
fn sigrok_lists_the_real_decoder_corpus() {
    let mut h = Harness::spawn();
    let resp = h.request(1, RequestOp::ListDecoders);
    let decoders: Vec<DecoderManifest> = match resp.status {
        ResponseStatus::Ok {
            body: Some(ResponseBody::Decoders(list)),
        } => list,
        other => panic!("unexpected response: {other:?}"),
    };
    // The exact count depends on the installed libsigrokdecode version
    // (Homebrew 0.5.3 ships ~111; a full apt corpus is 130+). Assert a
    // conservative lower bound plus the well-known protocol decoders.
    assert!(
        decoders.len() >= 50,
        "expected the full libsigrokdecode corpus, got {}",
        decoders.len()
    );
    let ids: Vec<_> = decoders.iter().map(|d| d.id.clone()).collect();
    for want in [
        "sigrok.jtag",
        "sigrok.pwm",
        "sigrok.dmx512",
        "sigrok.modbus",
        "sigrok.i2c",
        "sigrok.spi",
    ] {
        assert!(ids.contains(&want.to_string()), "missing decoder {want}");
    }
    // Every manifest id carries the sigrok. prefix and a display name.
    for d in &decoders {
        assert!(d.id.starts_with("sigrok."), "bad id {}", d.id);
        assert!(
            !d.display_name.is_empty(),
            "empty display_name for {}",
            d.id
        );
    }
    h.shutdown();
}

#[cfg(feature = "sigrok")]
#[test]
fn sigrok_full_session_lifecycle_for_pwm() {
    let mut h = Harness::spawn();
    let mut channels = std::collections::BTreeMap::new();
    channels.insert("data".into(), 0u32);
    let resp = h.request(
        1,
        RequestOp::CreateSession(CreateSessionBody {
            decoder_id: "sigrok.pwm".into(),
            channels,
            options: Default::default(),
            xz_policy: XzPolicy::Glitch,
        }),
    );
    let session = match resp.status {
        ResponseStatus::Ok {
            body: Some(ResponseBody::SessionCreated { session }),
        } => session,
        other => panic!("create_session failed: {other:?}"),
    };

    // Feed a clean 50%-duty square wave: toggle channel 0 every 1 µs for
    // a few periods (timestamps in femtoseconds).
    let mut t = vec![];
    for i in 0..16u64 {
        t.push(SampleTransition {
            fs: i * 1_000_000_000,
            set: vec![ChannelChange {
                ch: 0,
                v: if i % 2 == 0 {
                    BitValue::One
                } else {
                    BitValue::Zero
                },
            }],
        });
    }
    let (resp, _events) = request_and_drain(
        &mut h,
        2,
        RequestOp::Feed(FeedBody {
            session: session.clone(),
            samples: vec![SampleBatch { t }],
        }),
    );
    assert!(
        matches!(resp.status, ResponseStatus::Ok { .. }),
        "feed failed: {:?}",
        resp.status
    );

    // Finalize and destroy must both succeed; any emitted annotations are
    // real pwm output and not asserted here (the mock's scripted shape
    // does not apply to the live decoder).
    let resp = h.request(
        3,
        RequestOp::Finalize(FinalizeBody {
            session: session.clone(),
        }),
    );
    assert!(matches!(resp.status, ResponseStatus::Ok { .. }));
    let resp = h.request(4, RequestOp::Destroy(DestroyBody { session }));
    assert!(matches!(resp.status, ResponseStatus::Ok { .. }));

    h.shutdown();
}
