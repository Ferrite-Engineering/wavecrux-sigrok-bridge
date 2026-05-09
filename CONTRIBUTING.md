# Contributing to wavecrux-sigrok-bridge

Thanks for your interest in contributing. This document covers the
practical bits — license, sign-off, and the change submission flow.

## License of contributions

Every contribution to this repository is licensed under **GPLv3 or any
later version**, the same license as the rest of the repo. By submitting a
pull request you assert that you have the right to license your
contribution under those terms.

There is no Contributor License Agreement. The Developer Certificate of
Origin sign-off (below) is sufficient.

## Developer Certificate of Origin (DCO)

Every commit must be signed off, attesting that you wrote the code (or
have the right to submit it) and agree to license it under the project
terms. The DCO text is at <https://developercertificate.org>.

In practice this means appending a `Signed-off-by:` line to every commit
message. The easiest way is `git commit -s`:

```
feat: add SR latch annotation handler

Signed-off-by: Random Developer <random@developer.example.org>
```

CI rejects PRs whose commits are missing the sign-off line.

## Submission flow

1. Open an issue describing the change first if it's a non-trivial design
   decision (new IPC message, new feature flag, new dependency).
2. Fork, branch from `main` (`feature/...`, `fix/...`, etc).
3. Make your changes. Keep commits focused; squash the noisy ones before
   the PR.
4. Run `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`,
   and `cargo test --workspace`. CI runs the same checks.
5. Open a PR against `main` with a clear summary and a test plan.
6. CI must be green before merge. The license-isolation gate is a hard
   gate — see [`CLAUDE.md`](CLAUDE.md) and `.github/workflows/isolation.yaml`.

## Code style

* Rust 2021 edition.
* `cargo fmt` defaults.
* `cargo clippy -- -D warnings` clean.
* Conventional Commits message format.
* Public items have `///` rustdoc with at least a one-line summary.
* New IPC messages require a corresponding entry in
  [`docs/IPC_PROTOCOL.md`](docs/IPC_PROTOCOL.md) and a serde round-trip
  test in `crates/ipc`.
* New shim or bridge behavior requires a corresponding test in the
  matching crate.

## What you cannot do

The four license-isolation invariants in [`CLAUDE.md`](CLAUDE.md) are
non-negotiable. PRs that risk any of them — for example by adding a
`libsigrokdecode` dependency to the shim, by linking the shim and
bridge into a single binary, or by proposing to bundle the bridge into
the WaveCrux installer — will be closed without merge.

If you believe the invariants need to change, open an issue first. Do
not silently relax them in a PR.

## Reporting security issues

See [`docs/SECURITY.md`](docs/SECURITY.md). Do not open a public issue
for a security report.
