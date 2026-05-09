//! WaveCrux SigRok bridge subprocess.
//!
//! Without the `sigrok` feature this binary runs in **mock mode**:
//! it advertises five reference decoders (1-Wire, JTAG, PWM, DMX512,
//! Modbus) and emits scripted annotations driven entirely by a Rust
//! state machine. This keeps the entire test suite buildable and
//! runnable on a workstation without `libsigrokdecode` or `libpython`
//! installed.
//!
//! With the `sigrok` feature it links `libsigrokdecode` (via
//! bindgen-generated bindings) and embeds Python (via PyO3) to drive
//! the real protocol decoder corpus.
//!
//! See `docs/IPC_PROTOCOL.md` for the wire format.

mod ipc_loop;
#[cfg(not(feature = "sigrok"))]
mod mock;
#[cfg(feature = "sigrok")]
mod srd;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "wavecrux-sigrok-bridge",
    version,
    about = "Subprocess companion to the WaveCrux SigRok bridge shim.",
    long_about = "JSON-IPC bridge between the WaveCrux shim plugin and \
                  libsigrokdecode. License: GPLv3+."
)]
struct Cli {
    /// Run in IPC mode (read length-prefixed JSON requests on stdin,
    /// write responses on stdout). This is what the shim invokes.
    #[arg(long)]
    ipc: bool,

    /// Print the list of advertised decoders as JSON to stdout and
    /// exit. Useful for diagnostics and verification.
    #[arg(long)]
    list_decoders: bool,
}

fn main() -> anyhow::Result<()> {
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().filter_or("WAVECRUX_SIGROK_BRIDGE_LOG", "info"),
    )
    .target(env_logger::Target::Stderr)
    .try_init();

    let cli = Cli::parse();

    if cli.list_decoders {
        return print_decoder_list();
    }
    if cli.ipc {
        return ipc_loop::run();
    }

    eprintln!(
        "wavecrux-sigrok-bridge: no mode selected. \
         Pass --ipc (when invoked by the shim) or --list-decoders."
    );
    std::process::exit(2);
}

fn print_decoder_list() -> anyhow::Result<()> {
    let decoders = current_backend().list_decoders()?;
    println!("{}", serde_json::to_string_pretty(&decoders)?);
    Ok(())
}

/// Return the active decoder backend. Mock when the `sigrok` feature is
/// off; libsigrokdecode-backed when it is on.
pub(crate) fn current_backend() -> Box<dyn backend::Backend> {
    #[cfg(feature = "sigrok")]
    {
        Box::new(srd::SrdBackend::new())
    }
    #[cfg(not(feature = "sigrok"))]
    {
        Box::new(mock::MockBackend::new())
    }
}

pub(crate) mod backend {
    use anyhow::Result;
    use wavecrux_sigrok_bridge_ipc::{
        AnnotationEvent, CreateSessionBody, DecoderManifest, FeedBody, SessionId,
    };

    /// Decoder-engine abstraction. Real and mock implementations both
    /// satisfy this trait.
    pub trait Backend: Send {
        /// Enumerate every decoder this backend can host.
        fn list_decoders(&self) -> Result<Vec<DecoderManifest>>;

        /// Open a new session for one decoder. Returns a session id
        /// the IPC layer correlates against subsequent feeds.
        fn create_session(&mut self, body: CreateSessionBody) -> Result<SessionId>;

        /// Forward samples to the named session. Returns any
        /// annotations the decoder emitted while processing this batch.
        fn feed(&mut self, body: FeedBody) -> Result<Vec<AnnotationEvent>>;

        /// Tell the named session that no more samples are coming and
        /// drain any final annotations.
        fn finalize(&mut self, session: &SessionId) -> Result<Vec<AnnotationEvent>>;

        /// Release the session.
        fn destroy(&mut self, session: &SessionId) -> Result<()>;
    }
}
