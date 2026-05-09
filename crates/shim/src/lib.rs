//! WaveCrux SigRok bridge shim.
//!
//! This crate compiles to `libwavecrux_sigrok_bridge.{so,dylib,dll}` and
//! is dropped into WaveCrux's per-user plugin directory. From WaveCrux's
//! point of view this is a normal Phase 4.1 native plugin: it exports
//! `wavecrux_decoder_abi_version` and `wavecrux_decoder_register`, then
//! responds to lifecycle callbacks defined in
//! `crates/shim/include/wavecrux_decoder.h`.
//!
//! Internally the shim does **not** decode anything itself. Every
//! operation that touches a protocol decoder is forwarded to a
//! subprocess (the `wavecrux-sigrok-bridge` binary) over a JSON-IPC
//! pipe. This is the architectural mechanism that keeps
//! `libsigrokdecode` and `libpython` (both GPLv3+) outside the WaveCrux
//! process.
//!
//! See the repo-root README and `CLAUDE.md` for the full license-isolation
//! invariants. The CI workflow `.github/workflows/isolation.yaml`
//! mechanically verifies that this binary contains no `srd_*` or `Py*`
//! symbols and no linkage to `libsigrokdecode` or `libpython`.

mod abi;
mod manifest;
mod registry;
mod supervisor;

pub use abi::{wavecrux_decoder_abi_version, wavecrux_decoder_register};

/// Initialize logging once on first plugin entry. The WaveCrux loader
/// invokes the registration entry point exactly once per library load,
/// so this is the natural place to wire up `env_logger`.
fn init_logger_once() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = env_logger::Builder::from_env(
            env_logger::Env::default().filter_or("WAVECRUX_SIGROK_BRIDGE_LOG", "info"),
        )
        .try_init();
    });
}
