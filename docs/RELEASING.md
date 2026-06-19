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
source available does **not** require WaveCrux open-core or `wavecrux-pro` to be
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

### What to do before v1.0

1. **Enroll in the Apple Developer Program** ($99/year) and obtain a
   *Developer ID Application* certificate.

2. **Export the certificate** as a `.p12` and add these secrets to the
   `wavecrux-sigrok-bridge` GitHub repository:
   - `APPLE_DEVELOPER_ID_P12` — base64-encoded `.p12` file
   - `APPLE_DEVELOPER_ID_P12_PASSWORD` — the `.p12` password
   - `APPLE_ID` — the Apple ID used for notarization
   - `APPLE_APP_SPECIFIC_PASSWORD` — an app-specific password for that Apple ID
   - `APPLE_TEAM_ID` — the 10-character Apple Developer team ID

3. **Replace the ad-hoc codesign step** in `release.yaml` with a proper
   sign + notarize flow:

   ```yaml
   - name: Import Developer ID certificate
     if: matrix.target.os == 'macos'
     uses: apple-actions/import-codesign-certs@v3
     with:
       p12-file-base64: ${{ secrets.APPLE_DEVELOPER_ID_P12 }}
       p12-password: ${{ secrets.APPLE_DEVELOPER_ID_P12_PASSWORD }}

   - name: Sign macOS binaries
     if: matrix.target.os == 'macos'
     run: |
       codesign --sign "Developer ID Application: <Your Name> (<TEAM_ID>)" \
         --options runtime --timestamp --force \
         target/${{ matrix.target.triple }}/release/wavecrux-sigrok-bridge
       codesign --sign "Developer ID Application: <Your Name> (<TEAM_ID>)" \
         --options runtime --timestamp --force \
         target/${{ matrix.target.triple }}/release/libwavecrux_sigrok_bridge.dylib

   - name: Notarize macOS archive
     if: matrix.target.os == 'macos'
     run: |
       # Submit archive to Apple notarization service and staple ticket.
       xcrun notarytool submit "${tag}.tar.gz" \
         --apple-id "${{ secrets.APPLE_ID }}" \
         --password "${{ secrets.APPLE_APP_SPECIFIC_PASSWORD }}" \
         --team-id "${{ secrets.APPLE_TEAM_ID }}" \
         --wait
       # Staple requires the binary itself (archives are not stapleable),
       # so users still need to remove quarantine after download. The
       # notarization ticket is stored by Apple and checked online.
   ```

   > **Note on stapling:** Apple does not allow stapling a notarization
   > ticket to a `.tar.gz` archive — only to `.app` bundles, `.pkg`
   > installers, and `.dmg` images. Distributing as a signed `.pkg` or
   > `.dmg` instead of `.tar.gz` would allow stapling and fully offline
   > quarantine-free installs. For a `.tar.gz` distribution, the
   > notarization ticket is stored by Apple and verified online at first
   > launch; users still need to remove the quarantine flag or
   > right-click → Open the first time.

4. **Test the signed build** on a clean Mac that has never seen the
   bridge before. Confirm `spctl --assess --type exec wavecrux-sigrok-bridge`
   returns `accepted` and that loading the plugin in WaveCrux shows
   all decoders without any manual codesign or quarantine steps.

## Linux and Windows

No code-signing steps are needed for Linux. Windows Authenticode
signing (for SmartScreen compatibility) follows a similar pattern to
Apple notarization — a code-signing certificate from a trusted CA, a
`signtool.exe` step in CI. Add this before v1.0 if WaveCrux ships a
Windows installer that includes the bridge.
