# ADR 0001: Rust everywhere

* **Status:** Accepted, 2026-05-09.
* **Context:** Initial scoping of `wavecrux-sigrok-bridge`.

## Context

The bridge is two cooperating binaries: a shim that implements
WaveCrux's C plugin ABI, and a subprocess that hosts
`libsigrokdecode` (C, GPLv3+) and embeds Python (C, PSF-2.0). The
shim has no GPL dependencies; the subprocess does. The IPC layer
between them is JSON over a length-prefixed pipe.

Three viable language choices for the subprocess:

* **C** — natural fit for libsigrokdecode and libpython embedding;
  every published example of either uses C. Simplest build pipeline.
* **C++** — same C compatibility, plus modern containers and
  exception-safe RAII. The path Surfer would have taken if Surfer
  had a SigRok bridge.
* **Rust** — memory-safe IPC parser, shared workspace with the shim
  and the IPC crate, single CI matrix.

The shim is essentially fixed at Rust by the task brief — the IPC
parser is the highest-risk surface in the project, and the shim runs
inside the WaveCrux address space, so memory safety matters there
above all else.

## Decision

**Rust everywhere.** Both binaries and the shared IPC crate live in a
single Cargo workspace.

* `crates/ipc` — wire-format types and framing codec.
* `crates/shim` — the Phase 4.1 plugin (`cdylib`).
* `crates/bridge` — the subprocess.

For the bridge subprocess, libsigrokdecode is bound through `bindgen`
against `<libsigrokdecode/libsigrokdecode.h>` (BSD-3-Clause crate,
build-time only — no runtime libclang dep on shipped binaries).
Python embedding goes through PyO3 with `auto-initialize = false`, so
we drive interpreter setup in lock-step with `srd_init`.

## Consequences

### Positive

* Single toolchain (`cargo`), single CI matrix, single workspace.
  Contributors clone, run `cargo test --workspace`, and they have run
  every test in the project.
* The IPC crate is shared between the two binaries with zero
  serialization drift — both ends use the same `Message` enum.
* The shim has memory safety on the IPC parser by construction.
* The subprocess has the same on its IPC side, plus PyO3's
  ergonomic-but-explicit Python embedding model.

### Negative

* `bindgen` requires `libclang` at build time. CI runners have it; a
  contributor on a fresh Linux box might need `apt install libclang`.
  This is a build-time-only dependency — it does not appear in shipped
  binaries.
* PyO3 is still settling on its Python 3.13 / 3.14 support; we pin to
  PyO3 0.22 and a Python ≥ 3.10 requirement, which is the conservative
  compatibility band.

### Rejected alternatives

#### C subprocess

* Pro: matches every libsigrokdecode example exactly. Zero impedance
  for libpython embedding.
* Con: separate build system from the shim. JSON parsing in C is a
  hand-rolled chore (cJSON or a similar choice). The IPC layer becomes
  the highest-risk surface in the project, in C, with no shared types
  with the shim. We rejected this primarily on testability — keeping
  the IPC crate in Rust and shared between the two binaries removes
  an entire class of "did the C json struct match what the Rust serde
  type encoded" bugs.

#### C++ subprocess

* Pro: better data structures than C; libsigrokdecode is C so calling
  it from C++ is trivial.
* Con: a second build system to maintain. Exception handling across
  the JSON-IPC boundary becomes a discipline, not a guarantee. PyO3's
  Rust ergonomics has no peer in C++ (pybind11 is closest and is
  heavier). We rejected this on cost-benefit: C++ buys us no
  capability we don't get from Rust, and costs us tooling integration.

## Revisit if

* PyO3 fails to keep pace with Python 3.x major releases for a long
  enough window that we'd be stuck on an unsupported Python.
* The bridge ever needs to be embedded *into* WaveCrux directly (not
  expected — would violate invariants).
* We discover a libsigrokdecode quirk that PyO3 + bindgen genuinely
  cannot express. Unlikely; libsigrokdecode is straightforward C.
