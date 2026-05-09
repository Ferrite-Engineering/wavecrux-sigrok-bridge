//! Real libsigrokdecode + Python-embedded backend. **Stub.**
//!
//! This module is gated behind the `sigrok` Cargo feature. When enabled
//! it should:
//!
//!   1. Initialize libsigrokdecode (`srd_init`) once per process.
//!   2. Discover every Python decoder the runtime exposes
//!      (`srd_decoder_load_all`).
//!   3. For each `srd_decoder*` build a [`DecoderManifest`].
//!   4. Bridge `feed` calls to `srd_session_send`.
//!   5. Receive annotations through a Python callback registered with
//!      `srd_pd_output_callback_add` and ferry them out as
//!      [`AnnotationEvent`]s.
//!
//! Implementing this properly requires:
//!
//!   * `bindgen` against `<libsigrokdecode/libsigrokdecode.h>` in
//!     `build.rs` (only run when the `sigrok` feature is active);
//!   * PyO3 with `auto-initialize = false` driving `srd_init`-bound
//!     interpreter setup;
//!   * a Linux or libsigrokdecode-equipped macOS workstation to compile
//!     against the real library.
//!
//! This file currently compiles to a placeholder that reports an error
//! at runtime when invoked — sufficient to unblock the rest of the
//! workspace while the libsigrokdecode hookup is iterated on a CI
//! machine that has libsigrokdecode and Python development headers
//! installed.
//!
//! License note: every line of code in this file is GPLv3+, like the
//! rest of the repo. The eventual libsigrokdecode bindings will pull
//! GPL native code into the bridge subprocess; that is the whole reason
//! the bridge is its own repo.

use anyhow::{anyhow, Result};

use wavecrux_sigrok_bridge_ipc::{
    AnnotationEvent, CreateSessionBody, DecoderManifest, FeedBody, SessionId,
};

use crate::backend::Backend;

pub(crate) struct SrdBackend;

impl SrdBackend {
    pub(crate) fn new() -> Self {
        SrdBackend
    }
}

impl Backend for SrdBackend {
    fn list_decoders(&self) -> Result<Vec<DecoderManifest>> {
        Err(anyhow!(
            "real libsigrokdecode backend not implemented yet — \
             rebuild with default features for the mock backend, or \
             see crates/bridge/src/srd.rs for the implementation \
             checklist"
        ))
    }

    fn create_session(&mut self, _body: CreateSessionBody) -> Result<SessionId> {
        Err(anyhow!("real libsigrokdecode backend not implemented yet"))
    }

    fn feed(&mut self, _body: FeedBody) -> Result<Vec<AnnotationEvent>> {
        Err(anyhow!("real libsigrokdecode backend not implemented yet"))
    }

    fn finalize(&mut self, _session: &SessionId) -> Result<Vec<AnnotationEvent>> {
        Err(anyhow!("real libsigrokdecode backend not implemented yet"))
    }

    fn destroy(&mut self, _session: &SessionId) -> Result<()> {
        Err(anyhow!("real libsigrokdecode backend not implemented yet"))
    }
}
