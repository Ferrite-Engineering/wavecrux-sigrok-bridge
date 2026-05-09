# wavecrux-sigrok-bridge

A WaveCrux decoder plugin that bridges the SigRok protocol-decoder ecosystem
to WaveCrux through a process-isolated subprocess.

## License (binding)

**Entire repository is GPLv3-or-later.** See [`LICENSE`](LICENSE).

Reason: the bridge subprocess links `libsigrokdecode` (GPLv3+) and embeds
`libpython` (PSF-2.0; compatible with GPLv3+ but the GPL applies once
combined with libsigrokdecode). Anything in this repo, including the
shim that does not directly link those libraries, is published under
GPLv3+ for clarity and audit consistency.

WaveCrux itself is not GPL. WaveCrux's open-core repo is Apache 2.0
post-beta, and `wavecrux-pro` carries a separate commercial license. The
non-GPL boundary is preserved by the four license-isolation invariants
below — they are non-negotiable.

## License-isolation invariants

These four invariants are the architectural reason the WaveCrux project
can offer the SigRok decoder set without GPL contamination of WaveCrux
core. Any change that risks any of them is a code-review-blocking defect:

1. **Separate repo.** This repository is never a Git submodule of
   `wavecrux/wavecrux` or `wavecrux/wavecrux-pro`.
2. **Process boundary.** The shim never `dlopen`s, statically links, or
   otherwise loads `libsigrokdecode` or `libpython` into the WaveCrux
   process. Communication is exclusively spawn-subprocess + JSON-over-pipe.
3. **Separate distribution.** The bridge is published as a standalone
   GitHub Release on this repo. It is never bundled with the WaveCrux
   installer, auto-downloaded by WaveCrux, or advertised from inside the
   WaveCrux app without explicit user action.
4. **Explicit GPL notice.** The first paragraph of `README.md` and the
   shim's `manifest.json` description carry a clear GPLv3+ notice.

CI enforces invariant 2 mechanically. See
`.github/workflows/isolation.yaml`.

If you find yourself wanting to relax any of these to "make WaveCrux
integration smoother," that is the signal to stop and surface the question
to a human reviewer rather than proceed.

## What this repo is

Two cooperating Rust binaries:

* `crates/shim` — Rust `cdylib`. Compiled to
  `libwavecrux_sigrok_bridge.{so,dylib,dll}`. Implements the C ABI
  declared in WaveCrux's `wavecrux/include/wavecrux_decoder.h`. **Zero
  GPL dependencies.** Spawns the bridge subprocess and translates
  WaveCrux's `feed`/`flush`/`destroy` callbacks into JSON-IPC requests.
* `crates/bridge` — Rust binary. Compiled to
  `wavecrux-sigrok-bridge[.exe]`. Hosts `libsigrokdecode` (via
  bindgen-generated bindings) and embeds Python (via `pyo3` with
  `auto-initialize` disabled). Reads JSON-IPC requests from stdin,
  writes responses on stdout, structured logs on stderr.
* `crates/ipc` — shared Rust crate defining the wire-format types
  (length-prefixed JSON messages) and the framing codec. Used by both
  shim and bridge. **Zero GPL dependencies** (only `serde`, `serde_json`,
  both MIT/Apache 2.0; the IPC crate's *use* is GPL because the repo
  is GPL, but its dependencies are clean).

## Tech stack

* **Language:** Rust everywhere. Single toolchain, single workspace,
  single CI matrix. Rationale documented in
  [`docs/adr/0001-language-choice.md`](docs/adr/0001-language-choice.md).
* **Wire format:** length-prefixed JSON (`u32` LE byte length followed
  by UTF-8 JSON body). No newline framing. Spec in
  [`docs/IPC_PROTOCOL.md`](docs/IPC_PROTOCOL.md).
* **Python embedding:** PyO3 with `auto-initialize = false`. The Python
  runtime is not bundled into the binary — the bridge links the system
  Python via `python3-config --embed` on Linux/macOS, the official
  embeddable distribution on Windows.
* **libsigrokdecode binding:** `bindgen` against `<libsigrokdecode/libsigrokdecode.h>`.
* **JSON:** `serde` + `serde_json` (MIT/Apache 2.0 — fine to use here).

## Build & run

```bash
# All crates, default features (mock decoders only — no libsigrokdecode required)
cargo build --workspace
cargo test --workspace

# The bridge subprocess with real libsigrokdecode hookup
cargo build -p wavecrux-sigrok-bridge --features sigrok

# The shim only (always feature-free)
cargo build -p wavecrux-sigrok-bridge-shim --release

# Run the isolation gate locally (macOS / Linux)
./tool/verify_isolation.sh
```

The `sigrok` feature is the only thing that pulls in the GPL chain. Without
it, the bridge runs in `--mock-decoders` mode where it returns canned
manifests for the five reference decoders and replays a deterministic
script against fixture inputs. This lets the entire test suite run on a
machine without `libsigrokdecode` installed and lets us write fast
integration tests that don't require a sigrok runtime.

CI builds *both* configurations and runs both test suites.

## Repo layout

```
wavecrux-sigrok-bridge/
├── LICENSE                          # GPLv3+ full text
├── README.md                        # User-facing intro + GPL notice
├── CLAUDE.md                        # this file
├── CONTRIBUTING.md                  # DCO sign-off
├── NOTICES                          # SigRok / libsigrokdecode / Python attributions
├── Cargo.toml                       # workspace root
├── rust-toolchain.toml              # pinned stable
├── crates/
│   ├── ipc/                         # shared IPC types + framing codec
│   ├── shim/                        # cdylib — the WaveCrux plugin
│   └── bridge/                      # bin — hosts libsigrokdecode + Python
├── proto/
│   └── ipc.schema.json              # canonical schema for the IPC types
├── docs/
│   ├── ARCHITECTURE.md
│   ├── IPC_PROTOCOL.md
│   ├── INSTALL.md
│   ├── SECURITY.md
│   └── adr/
│       └── 0001-language-choice.md
├── test/
│   └── fixtures/                    # five reference VCDs + .expected_transactions.json
├── tool/
│   ├── generate_fixtures/           # Rust tool emitting deterministic fixtures
│   └── verify_isolation.sh          # nm/ldd/otool/dumpbin checks
└── .github/workflows/
    ├── ci.yaml
    ├── isolation.yaml
    └── release.yaml
```

## Coding conventions

* Rust 2021 edition. `cargo fmt` clean, `cargo clippy -- -D warnings` clean.
* Conventional Commits (`feat:`, `fix:`, `refactor:`, `docs:`, `test:`,
  `chore:`).
* DCO sign-off mandatory: `git commit -s`.
* No `--no-verify`. No `--no-gpg-sign`. No skipping CI.
* All public items have `///` rustdoc.
* Tests live alongside source as `#[cfg(test)] mod tests`. Integration
  tests for the IPC + shim contract live in `crates/*/tests/`. End-to-end
  fixture tests live in `crates/bridge/tests/` and require the `sigrok`
  feature (or the bundled mock).

## Cross-repo touchpoints

The bridge is consumed by WaveCrux through the existing Phase 4.1 plugin
loader (`wavecrux/lib/services/decoders/ffi/`). The bridge does not require
any code changes in WaveCrux to operate.

The WaveCrux repos do gain three additions when this bridge ships its
first release:

1. `wavecrux/verification/VERIFICATION_GUIDE.md` — a sub-section under
   the Phase 4.1 plugin loader covering the bridge install walkthrough
   and the five reference-decoder verification scenarios.
2. `wavecrux/verification/VERIFICATION_CHECKLIST.md` — matching bullet
   group.
3. `wavecrux/test/services/decoders/ffi/sigrok_bridge_smoke_test.dart` —
   a smoke test gated on the bridge being installed; skipped on CI
   machines without it.

The corresponding checkbox in `wavecrux-pro/docs/PROJECT_PLAN.md` § Phase
4.3 P0 is flipped to `[x]` in the same change set that publishes the
first bridge release.

## What this repo is not

* Not a feature of WaveCrux. The bridge is a downloadable plugin.
* Not redistributed by WaveCrux's installer or auto-update mechanism.
* Not advertised from inside the WaveCrux app without explicit user
  action.
* Not a replacement for WaveCrux's own protocol decoders. WaveCrux ships
  curated, tested SPI/I²C/UART/AXI4-Lite/APB/AHB-Lite/Wishbone decoders
  in open core and AXI4 full / USB / PCIe TLP / JTAG / MDIO / Ethernet
  decoders in the Pro overlay. The bridge is for the long tail.
