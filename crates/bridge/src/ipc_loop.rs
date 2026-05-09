//! IPC loop: read length-prefixed JSON requests on stdin, dispatch to
//! the active backend, write responses (and unsolicited annotation
//! events) on stdout.
//!
//! Stdout carries IPC frames only. Stderr is reserved for log output
//! (consumed by the shim's stderr drain thread).

use anyhow::Result;
use log::{error, warn};

use wavecrux_sigrok_bridge_ipc::{
    AnnotationEvent, ErrCode, Event, FrameReader, FrameWriter, Message, Request, RequestOp,
    Response, ResponseBody, ResponseStatus, SessionId,
};

use crate::current_backend;

pub(crate) fn run() -> Result<()> {
    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();
    let mut reader = FrameReader::new(stdin);
    let mut writer = FrameWriter::new(stdout);

    let mut backend = current_backend();

    loop {
        let frame = match reader.read_frame() {
            Ok(b) => b,
            Err(wavecrux_sigrok_bridge_ipc::CodecError::PeerClosed) => {
                return Ok(());
            }
            Err(e) => {
                error!("ipc: framing error: {e}");
                return Err(e.into());
            }
        };
        let msg: Message = match serde_json::from_slice(&frame) {
            Ok(m) => m,
            Err(e) => {
                warn!("ipc: bad json from shim: {e}");
                continue;
            }
        };
        let req = match msg {
            Message::Request(r) => r,
            other => {
                warn!("ipc: shim sent non-request: {other:?}");
                continue;
            }
        };
        let id = req.id;
        let (resp, events): (Response, Vec<AnnotationEvent>) = handle(&mut backend, req);
        // Emit annotation events *before* the response so the shim,
        // which returns on the matching response, has already buffered
        // every annotation that resulted from this request.
        for ev in events {
            send(&mut writer, &Message::Event(Event::Annotation(ev)))?;
        }
        send(&mut writer, &Message::Response(resp))?;
        if id == 0 {
            // shutdown sentinel — once handled we exit cleanly.
            return Ok(());
        }
    }
}

fn handle(
    backend: &mut Box<dyn crate::backend::Backend>,
    req: Request,
) -> (Response, Vec<AnnotationEvent>) {
    let id = req.id;
    match req.op {
        RequestOp::ListDecoders => match backend.list_decoders() {
            Ok(list) => (
                Response {
                    id,
                    status: ResponseStatus::Ok {
                        body: Some(ResponseBody::Decoders(list)),
                    },
                },
                vec![],
            ),
            Err(e) => err(id, ErrCode::Internal, &e.to_string()),
        },
        RequestOp::CreateSession(body) => match backend.create_session(body) {
            Ok(session) => (
                Response {
                    id,
                    status: ResponseStatus::Ok {
                        body: Some(ResponseBody::SessionCreated { session }),
                    },
                },
                vec![],
            ),
            Err(e) => err(id, ErrCode::SessionInitFailed, &e.to_string()),
        },
        RequestOp::Feed(body) => {
            let session = body.session.clone();
            match backend.feed(body) {
                Ok(events) => (
                    Response {
                        id,
                        status: ResponseStatus::Ok { body: None },
                    },
                    tag_events(&session, events),
                ),
                Err(e) => err(id, ErrCode::DecoderFailed, &e.to_string()),
            }
        }
        RequestOp::Finalize(body) => match backend.finalize(&body.session) {
            Ok(events) => (
                Response {
                    id,
                    status: ResponseStatus::Ok { body: None },
                },
                tag_events(&body.session, events),
            ),
            Err(e) => err(id, ErrCode::DecoderFailed, &e.to_string()),
        },
        RequestOp::Destroy(body) => match backend.destroy(&body.session) {
            Ok(()) => (
                Response {
                    id,
                    status: ResponseStatus::Ok { body: None },
                },
                vec![],
            ),
            Err(e) => err(id, ErrCode::Internal, &e.to_string()),
        },
        RequestOp::Shutdown => (
            Response {
                id: 0,
                status: ResponseStatus::Ok { body: None },
            },
            vec![],
        ),
    }
}

fn err(id: u64, code: ErrCode, message: &str) -> (Response, Vec<AnnotationEvent>) {
    (
        Response {
            id,
            status: ResponseStatus::Err {
                code,
                message: message.to_owned(),
            },
        },
        vec![],
    )
}

fn tag_events(session: &SessionId, events: Vec<AnnotationEvent>) -> Vec<AnnotationEvent> {
    events
        .into_iter()
        .map(|mut e| {
            if e.session.is_empty() {
                e.session = session.clone();
            }
            e
        })
        .collect()
}

fn send<W: std::io::Write>(writer: &mut FrameWriter<W>, msg: &Message) -> Result<()> {
    let body = serde_json::to_vec(msg)?;
    writer.write_frame(&body)?;
    Ok(())
}
