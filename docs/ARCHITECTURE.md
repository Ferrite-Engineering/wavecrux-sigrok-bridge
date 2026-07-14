# Architecture

## Why this repo exists at all

The WaveCrux project plan calls for exposing the 130+ SigRok
community-maintained protocol decoders to WaveCrux users without
contaminating WaveCrux's non-GPL license boundary. `libsigrokdecode`
and the SigRok decoders are GPLv3+; WaveCrux's open-core repo is
Apache 2.0 (post-beta) and the closed-source `wavecrux-pro` overlay
ships under a commercial license. Linking GPL code into either of those
would force them under GPL.

The bridge solves this by separating the GPL world from the WaveCrux
world with a process boundary. The bridge **subprocess** links
`libsigrokdecode` and embeds Python; the WaveCrux process loads only
the **shim**, which in turn spawns the bridge and exchanges JSON over
a pipe. Neither WaveCrux's installer nor any WaveCrux source code
loads `libsigrokdecode`.

## The four invariants (recap)

1. Separate repo (this one) — never a submodule of either WaveCrux repo.
2. Process boundary — no `dlopen`, no static linkage of libsigrokdecode
   or libpython into the WaveCrux process.
3. Separate distribution — published as GitHub Releases on this repo;
   never bundled with the WaveCrux installer.
4. Explicit GPL notice — README, CLAUDE.md, and the manifest description.

These appear verbatim in `README.md` and `CLAUDE.md`. Invariant 2 is
mechanically enforced by `tool/verify_isolation.sh` in the
`.github/workflows/isolation.yaml` workflow.

## Two binaries, one repo

```
┌────────────────────────────────────────────────────────────┐
│ WaveCrux process (Apache 2.0 / commercial)                 │
│                                                            │
│   ┌──────────────────────────────┐                         │
│   │ FfiDecoderLoader             │                         │
│   │ (open-core Phase 4.1 plugin   │                         │
│   │ system, dart:ffi)             │                         │
│   └──────────────┬───────────────┘                         │
│                  │ wavecrux_decoder.h (C ABI)              │
│   ┌──────────────▼───────────────┐                         │
│   │ libwavecrux_sigrok_bridge_shim.*  │  ←── shim (this repo)   │
│   │ • implements C ABI           │      GPLv3+, but        │
│   │ • spawns subprocess          │      contains no        │
│   │ • forwards via JSON-IPC      │      GPL native code    │
│   └──────────────┬───────────────┘                         │
└──────────────────┼─────────────────────────────────────────┘
                   │   length-prefixed JSON
                   │   on stdin/stdout
                   ▼
┌────────────────────────────────────────────────────────────┐
│ wavecrux-sigrok-bridge process (GPLv3+)                    │
│                                                            │
│   ┌──────────────────────────────┐                         │
│   │ ipc_loop                     │                         │
│   └──────────────┬───────────────┘                         │
│   ┌──────────────▼───────────────┐                         │
│   │ Backend (trait)               │                         │
│   │   • mock backend (default)    │                         │
│   │   • srd backend (`sigrok`     │                         │
│   │     feature)                  │                         │
│   │       • libsigrokdecode (C)   │                         │
│   │       • libpython3 via PyO3   │                         │
│   │       • SigRok decoder corpus │                         │
│   │         (Python, GPLv3+)      │                         │
│   └───────────────────────────────┘                         │
└────────────────────────────────────────────────────────────┘
```

## Crate layout

* **`crates/ipc`** — wire-format types and the length-prefixed JSON
  framing codec. Used by both shim and bridge. Permissively licensed
  dependencies only (`serde`, `serde_json`, `thiserror`, `byteorder`).
  The crate's *source* is GPLv3+ because the repo is GPLv3+, but the
  dependency closure is clean.
* **`crates/shim`** — Rust `cdylib`. Compiled to
  `libwavecrux_sigrok_bridge_shim.{so,dylib,dll}`. Implements the C ABI
  declared in `include/wavecrux_decoder.h` (vendored verbatim from the
  WaveCrux open-core repo). **Has no `libsigrokdecode` or `libpython`
  dependencies, transitive or otherwise.** The shim has three jobs:
  spawn the subprocess, forward callbacks as JSON-IPC, translate
  bridge events into `WcTransaction`s.
* **`crates/bridge`** — Rust binary. Compiled to
  `wavecrux-sigrok-bridge[.exe]`. Hosts the active backend behind a
  `Backend` trait. Default features select the **mock backend**, which
  advertises five reference decoders and replays scripted annotations.
  The `sigrok` feature swaps in the libsigrokdecode-backed `SrdBackend`
  (currently a stub — see `src/srd.rs`).
* **`tool/generate_fixtures`** — deterministic VCD + expected-JSON
  emitter. Run before commits that touch fixture-relevant code.

## License-feature gating

The `sigrok` Cargo feature on the `bridge` crate is the **only** path
to pull GPL native code into a built binary. Without it:

* `cargo build` produces a fully working subprocess running on the
  mock backend.
* The full integration test suite passes on any machine with Rust
  installed — no libsigrokdecode, no libpython.
* The shim never sees a difference; the IPC contract is identical.

This means the entire repo can be developed, reviewed, and merged on
machines without `libsigrokdecode` installed, while still guaranteeing
the libsigrokdecode-backed builds will exist and be tested in CI on a
prepared Linux host.

## Threading model

Per `wavecrux_decoder.h`, the WaveCrux loader serializes every callback
on a given handle. A single `Mutex` inside `SharedRegistry` is enough
to serialize requests on the shim side; the supervisor talks to the
subprocess synchronously, request-by-request, and buffers events
arriving in between for the next drain.

The subprocess is single-threaded with respect to libsigrokdecode (the
upstream library is not thread-safe). All decode work happens on the
subprocess's main thread; stdin/stdout I/O is also on the main thread,
which is fine because the protocol is synchronous.

## Why length-prefixed JSON instead of NDJSON

Newline-delimited JSON requires either escaping every embedded newline
in every annotation string or stripping them at emission time. SigRok
annotation labels can legitimately contain newlines (multi-line
descriptions of decoded payloads), and a single missed escape silently
desynchronizes the stream. Length prefixing eliminates that bug class
at the cost of four bytes per message. The trade-off is obvious.

## Why Rust everywhere

Single toolchain, single workspace, single CI matrix. The shim's IPC
parser is the highest-risk surface in the project (it handles bytes
from a subprocess); making it memory-safe by construction matters more
than it would for an internal C tool. The subprocess gets the same
benefit on its IPC side, plus a clean Python embedding via PyO3
(`auto-initialize = false` so we drive interpreter init in lock-step
with `srd_init`). The trade-off is `bindgen` against
`libsigrokdecode.h` at build time, which requires `libclang`. That's
acceptable — `libclang` is a build-time dep, not runtime, and CI
runners have it.

C++ was the obvious alternative (it's libsigrokdecode's host language).
We rejected it because it costs us a second build system to gain
nothing — `libsigrokdecode` is C, Python embedding is C, and
`serde_json` beats every C++ JSON library on usability.

ADR: [`adr/0001-language-choice.md`](adr/0001-language-choice.md).

## What the mock backend is for

The mock advertises the five reference decoders and emits scripted
annotations. It exists for three reasons:

1. **CI without GPL deps.** Most contributors don't have libsigrokdecode
   installed locally, and we don't want to make it a hard requirement
   to develop the IPC protocol or the shim.
2. **Conformance test for the IPC contract.** Mock annotations are
   deterministic and match the committed `expected_transactions.json`
   companions. The end-to-end test suite (`crates/bridge/tests/`) is
   the production-grade verification of the wire protocol.
3. **Fast acceptance signal.** When you're iterating on the shim or the
   IPC protocol, the mock loop is sub-second. Adding libsigrokdecode
   to that loop would burn minutes per cycle.

When the `sigrok` feature is on, the mock is bypassed — the real
libsigrokdecode engine answers every IPC request.

## Subprocess discovery

The shim resolves the subprocess in this order on every spawn:

1. `WAVECRUX_SIGROK_BRIDGE` environment variable, must be an absolute
   path.
2. Sibling-binary lookup: same directory as the shim itself.
3. `wavecrux-sigrok-bridge` on `PATH`.

Order 2 is the typical install: drop both binaries into WaveCrux's
per-user plugin directory and the shim finds the subprocess
automatically. `WAVECRUX_SIGROK_BRIDGE` exists for development overrides
and diagnostics.

## Failure modes

* **Subprocess missing.** `Supervisor::spawn` returns
  `SpawnFailed`. The shim logs a warning and registers zero decoders;
  WaveCrux behaves as if the bridge plugin were absent.
* **ABI mismatch.** The shim reports its own ABI version through
  `wavecrux_decoder_abi_version`. The WaveCrux loader rejects the
  plugin if MAJOR doesn't match. The shim never tries to talk to a
  WaveCrux that wouldn't load it.
* **Subprocess crash mid-session.** The `Supervisor` reports
  `SubprocessDied`. Active sessions are marked failed. The next
  `create_session` call respawns the subprocess.
* **libsigrokdecode missing at runtime.** Only matters when the
  `sigrok` feature is on. The subprocess fails `srd_init`, sends an
  `Err{Internal}` response, and exits. The shim treats this as
  "subprocess unavailable" and shows zero decoders.
* **Indeterminate samples (X/Z).** Default policy is `glitch` —
  emit a `glitch` annotation and skip the sample. `coerce_last`
  policy substitutes the previous determinate value. Documented in
  [`IPC_PROTOCOL.md`](IPC_PROTOCOL.md).

## Performance envelope

The subprocess can handle several million annotations per second over
the JSON pipe — the bottleneck is `serde_json` and the OS pipe, not
libsigrokdecode itself. WaveCrux's transaction-table view is the
practical ceiling at the user-facing end. We have not yet measured
end-to-end latency for an interactive session; the design assumes the
shim's `feed` calls are batched by the WaveCrux loader to amortize JSON
round-trips.
