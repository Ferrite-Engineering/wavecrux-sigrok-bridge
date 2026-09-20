# Release checklist

How to cut a release of `wavecrux-sigrok-bridge`.

## Pre-flight

- [ ] All CI checks pass on `main` (`ci.yaml` + `isolation.yaml`).
- [ ] `Cargo.toml` workspace version bumped to the release version.
- [ ] `CHANGELOG.md` (or GitHub release notes draft) written.
- [ ] Five reference decoder tests pass (`cargo test --workspace`).
- [ ] **Repo is public** — required before the *first* public binary Release.
      See "Repository visibility & GPL source availability" below. Do not push a
      release tag while the repo is private if the binaries are meant to be
      publicly downloadable.

## Repository visibility & GPL source availability

This repository is GPLv3+. GPL's source-availability obligation triggers when
you **distribute the binary**, and it runs to **everyone who can obtain that
binary**. Because the bridge is fully isolated in this repository — its own
license, its own process boundary, no GPL code in WaveCrux open-core or Pro —
that obligation is **contained entirely to this repo**. Making this repo's
source available does **not** require the WaveCrux open core or the Pro overlay to be
source-available, and it does not affect their beta closed-source posture. This
isolation is the whole point of the separate-repo + subprocess design.

Practical consequences for cutting a release:

- **During development (current state):** the repo may stay **private**. No
  binaries have been distributed, so no source obligation is live. (GitHub
  Releases on a private repo are visible only to accounts with repo access, so a
  private repo cannot leak a public binary.) The repo is private today only
  because the bridge is not yet ready for public download — not for any
  licensing reason.

- **At the first public binary Release:** flip the repo to **public**, then push
  the release tag. Because the release archives and the corresponding source
  live in the *same* repo at the *same* URL, going public satisfies GPLv3 §6(a)/(d)
  automatically — the source accompanies the binary from the same place. No
  written-offer (§6(b)) bookkeeping is needed.

- **Never split binary and source visibility.** Do not publish a public binary
  while the repo is private — that distributes a GPL binary without accompanying
  source. Either keep both private (a controlled-tester handoff, where you
  provide source privately to those same testers) or make both public. The
  default plan is the latter.

> **CI note:** the binary-publishing workflow (`release.yaml`) already exists and
> fires on a `v*` tag push — building, archiving, checksumming, and uploading the
> per-platform archives to a GitHub Release. No additional CI is required to
> *produce or publish* binaries. The only outstanding pre-v1.0 CI upgrade is the
> macOS Developer ID signing/notarization described below.

## Cut the release

Push a version tag — CI does the rest:

```bash
git tag v0.1.0
git push origin v0.1.0
```

`release.yaml` fires on the tag push, builds all four platform targets
(linux x86_64, macOS x86_64, macOS arm64, windows x86_64), ad-hoc signs
the macOS binaries, assembles archives, generates per-archive SHA256
sidecars, and uploads everything to GitHub Releases.

## macOS code-signing status

| Stage | What ships | User experience |
|---|---|---|
| **Today (beta)** | Ad-hoc signature (`codesign --sign -`) | Works for developer installs. Users must manually `xattr -d com.apple.quarantine` after downloading. |
| **v1.0 public release** | Developer ID Application cert + Apple notarization | Zero-friction install; Gatekeeper approves without any user action. |

### Why this matters

WaveCrux is built with the macOS `CS_EXEC_SET_KILL` entitlement, which
propagates to all child processes the app spawns — including the bridge
subprocess. An unsigned binary is killed immediately by the kernel
(`SIGKILL`, `Taskgated Invalid Signature`) before it can print a single
byte. An ad-hoc signature satisfies the kernel check; a Developer ID
signature + notarization additionally satisfies Gatekeeper for downloaded
archives.

### Developer ID signing and notarization — IMPLEMENTED

`release.yaml` signs and notarizes macOS builds. Three steps run on a
macOS runner, in order:

1. **Set up macOS signing** imports the *Developer ID Application*
   identity from secrets into a throwaway keychain. It **no-ops when
   `MACOS_CERT_P12_BASE64` is absent**, so a fork or a secretless run
   still produces working ad-hoc-signed binaries rather than failing.
2. **Codesign macOS binaries** signs both the executable and the
   `.dylib` with `--options runtime --timestamp` when the identity is
   present, and falls back to `codesign --sign -` when it is not.
   Hardened runtime is not optional here: notarization rejects
   binaries without it.
3. **Notarize macOS binaries** submits them and waits.

The secrets are the **same four the four product repos use**, so one
certificate serves the whole suite:

- `MACOS_CERT_P12_BASE64` — base64 of the `.p12` (single line)
- `MACOS_CERT_PASSWORD` — the `.p12` password
- `NOTARY_APPLE_ID` — Apple ID used for notarization
- `NOTARY_PASSWORD` — an **app-specific** password for that Apple ID

There is no team-id secret: `7957R7M965` (Ferrite Engineering LLC) is
set in the workflow, matching `sign_notarize_macos.sh` in the product
repos.

#### Why a submission-only zip, and not the published archive

`notarytool` accepts `.zip`, `.pkg` and `.dmg` — **not `.tar.gz`**. An
earlier draft of this document proposed `notarytool submit
"${tag}.tar.gz"`, which cannot work; it would have failed on the first
tagged release.

What notarization registers with Apple is each binary's
**code-directory hash**, not the container it arrived in. So the
workflow zips the two signed binaries into a throwaway archive purely
to submit them. The binaries are then notarized whichever archive
carries them, and the published `.tar.gz`, its `.sha256`, `README.md`
and `docs/INSTALL.md` all stay exactly as they are.

> **Note on stapling:** a ticket can only be stapled to a `.app`
> bundle, a `.pkg`, or a `.dmg` — never to a loose executable or to a
> zip of one. Gatekeeper resolves these binaries online instead. That
> is a deliberate trade: shipping a `.pkg` or `.dmg` would allow
> offline stapling, at the cost of changing the artifact every
> downstream document names. Revisit if offline installs are asked
> for.

#### Verifying a release

**Still to be done once, on a clean Mac that has never seen the bridge**
— the signing path above is wired but has not yet run against a tag.
Download the published `.tar.gz`, unpack it, and confirm:

- `codesign -dv --verbose=4 wavecrux-sigrok-bridge` names
  `Developer ID Application: Ferrite Engineering LLC (7957R7M965)` and
  reports `flags=0x10000(runtime)`.
- `spctl --assess --type exec wavecrux-sigrok-bridge` returns
  `accepted`.
- Loading the plugin in WaveCrux shows all decoders with no manual
  `codesign` or `xattr -d com.apple.quarantine` step.

The middle one is the check that distinguishes *signed* from
*notarized*: a Developer ID signature alone passes `codesign --verify`
and still fails `spctl` on a quarantined download.

## Linux and Windows

No code-signing steps are needed for Linux. Windows Authenticode
signing (for SmartScreen compatibility) follows a similar pattern to
Apple notarization — a code-signing certificate from a trusted CA, a
`signtool.exe` step in CI. Add this before v1.0 if WaveCrux ships a
Windows installer that includes the bridge.

## Windows full-backend bundle (pre-v1.0)

**Current state (beta):** the Windows release archive ships the **mock
backend** only — the five reference decoders, pure Rust, no runtime
dependencies (`sigrok: false` in `release.yaml`). The Linux and macOS
archives ship the real `libsigrokdecode` backend; Windows does not. This
is a *packaging* gap, not a capability gap — libsigrokdecode and its Python
decoder corpus run on Windows (sigrok ships a Windows build), but there is
no `apt`/Homebrew-style package to install the runtime, so the real backend
cannot lean on a system package the way Linux/macOS do.

To reach decoder parity with Linux/macOS on Windows, one of these has to
land before v1.0:

1. **Self-contained bundle (preferred).** Build the Windows archive with
   `--features sigrok` and bundle the runtime *into* the archive:
   `libsigrokdecode.dll`, its GLib dependency DLLs, an embedded Python
   interpreter, and the Python decoder corpus — all placed next to
   `wavecrux-sigrok-bridge.exe` so the loader resolves them with no user
   setup. This is real packaging work (DLL dependency closure + Python
   embedding) and is the main reason Windows is still on the mock backend.
   - GPL note: bundling these DLLs is fine — they ship inside *this*
     GPLv3+ repo's release, with source in the same repo (see "Repository
     visibility & GPL source availability" above). No new licensing blocker.
   - Add a `sigrok: true` Windows matrix entry to `release.yaml` plus a
     bundling step once the DLL closure is pinned.

2. **Documented manual-runtime path (fallback).** Keep the mock archive but
   point Windows users at sigrok's own Windows build
   (<https://sigrok.org/wiki/Windows>) to drop `libsigrokdecode-*.dll` + its
   dependency DLLs next to the bridge `.exe`. Already documented in
   `docs/INSTALL.md`; no CI change. This is the beta answer, not the v1.0
   answer.

Track this alongside the Windows Authenticode signing item above — they are
the same "make Windows a first-class distribution target" workstream.

### Pre-v1.0 Windows checklist

- [ ] Decide bundle (#1) vs. keep documented-manual (#2) for v1.0.
- [ ] If bundling: pin the `libsigrokdecode` + GLib + Python DLL closure and
      add the `sigrok: true` Windows build + bundling step to `release.yaml`.
- [ ] Windows Authenticode signing (`signtool.exe` + CA cert) wired into CI.
- [ ] Verify a clean-Windows install exposes the full corpus (or the
      documented mock set) with the decoder count shown in
      `Settings → Decoders → Plugins`.
