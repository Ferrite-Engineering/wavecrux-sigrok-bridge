# Security & trust model

## What this plugin runs

When the bridge is installed, two pieces of code run in the user's
session:

1. **The shim** — a Rust `cdylib` loaded into the WaveCrux process
   address space.
2. **The subprocess** — a separate process that loads
   `libsigrokdecode`, embeds Python, and runs Python protocol decoder
   modules.

Both pieces have *user-level* privileges. The bridge does no
sandboxing of its own — it is a development plugin, not a hardened
runtime.

## Trust model

WaveCrux's user-contributed decoder plugin loader (Phase 4.1) treats
every plugin as **fully trusted user code**. From `wavecrux/CLAUDE.md`
and the project plan §4.2.7:

> A plugin is loaded with the user's privileges and is trusted in the
> same way an arbitrary `.so` from a personal toolchain would be.

The user is the trust anchor. WaveCrux's first-load safety
acknowledgment ("I understand plugins run as native code") applies to
the bridge exactly as it does to any other plugin.

## libsigrokdecode and Python

`libsigrokdecode` loads a directory of Python files
(`/usr/share/libsigrokdecode/decoders` on Linux, comparable paths on
macOS/Windows) and executes them in an embedded interpreter. Anything
those Python files do — file I/O, network calls, subprocess spawning,
arbitrary computation — happens at user privilege.

In practice the upstream SigRok decoder corpus is well-behaved and
audited (it's been in production for years). But the bridge does
**not** restrict what those Python modules can do. Treat your
libsigrokdecode install with the same level of trust you give the
package source it came from (your distro's package manager, Homebrew,
or whatever you used).

## Process isolation, what it does and doesn't do

The shim and subprocess are deliberately separated. That separation
provides:

* **License isolation.** GPL libraries never enter the WaveCrux
  process. This is the primary motivation. See `CLAUDE.md` invariants.
* **Crash isolation.** A SIGSEGV inside libsigrokdecode crashes the
  subprocess, not WaveCrux. The shim re-spawns on the next request.

It does **not** provide:

* **Privilege isolation.** The subprocess runs as the same user as
  WaveCrux, with the same filesystem and network access.
* **Resource isolation.** A runaway decoder can consume CPU and memory
  in the subprocess; the shim does not enforce a budget.

If the WaveCrux project later decides to sandbox the bridge — e.g.
seccomp on Linux, App Sandbox on macOS, AppContainer on Windows —
that is additive on top of process isolation. Today there is no
sandbox.

## Network behavior

The bridge does **not** make network calls.

* The shim does not check for updates, phone home, or contact any
  service. Updates are out-of-band — see the GitHub Releases page.
* The subprocess does not initiate network connections of its own.
  libsigrokdecode itself does not need the network. Individual SigRok
  Python decoder modules *could* in principle make network calls, but
  the upstream corpus does not.

If you want to verify this on a paranoid build, run the subprocess
under `strace -e trace=network` (Linux) or Activity Monitor → Network
column (macOS). You should see no connections at startup or during
decode.

## Updates

The bridge does **not** auto-update. Updating means: download the new
release archive, verify the SHA256 against the companion file,
extract, replace the shim and subprocess binaries.

This is intentional. The bridge is GPLv3+ and travels on a different
release cadence from WaveCrux — auto-updating from inside a non-GPL
WaveCrux process would couple distribution channels, which violates
license-isolation invariant 3.

## Reporting a security issue

Email <martin.robert.fink@gmail.com> with `[wavecrux-sigrok-bridge
SECURITY]` in the subject. Do **not** open a public GitHub issue.

Acknowledged within five business days. Disclosure timeline is
negotiated case-by-case but defaults to coordinated disclosure 60 days
after a fix is available.

## Threat model summary

| Threat | Mitigation | Notes |
|---|---|---|
| GPL contamination of WaveCrux | Process boundary, four invariants in `CLAUDE.md` | Mechanically enforced by `verify_isolation.sh` in CI. |
| Crash in libsigrokdecode brings down WaveCrux | Subprocess separation, supervisor restarts on next request | Active sessions are marked failed; user sees an error transaction. |
| Malicious decoder Python module | None | Trust your libsigrokdecode source. |
| Tampered release archive | SHA256 companions in every release | Verify before installing. |
| Unauthorized auto-update | Bridge has no update mechanism | User-driven only. |
