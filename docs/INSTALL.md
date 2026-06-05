# Installation guide

The bridge is a desktop-only opt-in plugin. It runs on Linux, macOS,
and Windows alongside WaveCrux on the same machine.

## Step-by-step

### 1. Confirm prerequisites

| Requirement | Why |
|---|---|
| WaveCrux ≥ X.Y (Phase 4.1 plugin loader) | The bridge plugs into the WaveCrux user-contributed decoder loader. |
| Python 3.10 or later, accessible via the system | The bridge subprocess embeds Python at runtime. Linux/macOS use the system or Homebrew Python. (The Windows release archive currently ships the mock backend, which embeds no Python — see the Windows note below.) |
| `libsigrokdecode` runtime + the SigRok decoder set | The real-backend subprocess loads these on startup (Linux/macOS release archives). See platform-specific instructions below. |

### 2. Download the release archive

Grab the archive matching your OS+architecture from
[GitHub Releases](https://github.com/wavecrux/wavecrux-sigrok-bridge/releases),
along with the matching `*.sha256` companion. Verify the checksum before
extracting:

```bash
shasum -a 256 -c wavecrux-sigrok-bridge-vX.Y.Z-<os>-<arch>.tar.gz.sha256
```

### 3. Extract and copy the shim into WaveCrux's plugin directory

**Easiest method (all platforms):** open WaveCrux, go to
**Settings → Decoders → Plugins**, and click **Open plugin directory**.
This creates the correct directory for your WaveCrux build and opens it
in the system file manager. Drop both files in there.

If you prefer to locate the directory manually:

| Platform | Path |
|---|---|
| macOS | `~/Library/Application Support/<bundle-id>/wavecrux/decoders/` — the bundle-id varies by WaveCrux variant (e.g. `com.ferriteengineering.wavecruxPro`); use the button above to avoid guessing. |
| Linux | `~/.local/share/wavecrux/decoders/` (or `$XDG_CONFIG_HOME/wavecrux/decoders/`) |
| Windows | `%APPDATA%\WaveCrux\decoders\` |

Copy **both** `libwavecrux_sigrok_bridge.{so,dylib,dll}` **and**
`wavecrux-sigrok-bridge[.exe]` into that directory.

To override the directory, set `WAVECRUX_DECODER_PATH` in your
environment before launching WaveCrux.

### 3a. macOS: remove the download quarantine flag and codesign

macOS quarantines files downloaded from the internet. The release
archive is ad-hoc signed (no Apple Developer account needed), but
Gatekeeper still blocks quarantined binaries at first launch. After
extracting and copying both files, run:

```bash
# Remove the quarantine flag added by your browser / curl / unarchiver
xattr -d com.apple.quarantine \
  "/path/to/decoders/wavecrux-sigrok-bridge" \
  "/path/to/decoders/libwavecrux_sigrok_bridge.dylib" 2>/dev/null || true
```

The release binaries are already ad-hoc signed by CI; no manual
`codesign` call is needed. If you built from source, ad-hoc sign both
binaries yourself before placing them in the plugin directory:

```bash
codesign --sign - --force /path/to/wavecrux-sigrok-bridge
codesign --sign - --force /path/to/libwavecrux_sigrok_bridge.dylib
```

**Why this is required:** WaveCrux is built with the macOS
`CS_EXEC_SET_KILL` entitlement, which means any subprocess it spawns
must carry a valid code signature. An unsigned binary is killed by the
kernel immediately on launch with `SIGKILL (Code Signature Invalid)`.

### 4. Make the bridge subprocess discoverable

The shim resolves the subprocess in this order:

1. `WAVECRUX_SIGROK_BRIDGE` environment variable — absolute path to the
   subprocess binary.
2. Sibling binary in the same directory as the shim itself (recommended
   default — drop both binaries together).
3. `wavecrux-sigrok-bridge` on `PATH`.

The simplest install is to drop both files into the WaveCrux plugin
directory side by side.

### 5. Install the libsigrokdecode runtime

#### Linux (Debian / Ubuntu)

```bash
sudo apt update
sudo apt install libsigrokdecode4 libsigrokdecode-dev sigrok-cli python3
```

The `sigrok-cli` package pulls in the full Python decoder corpus.
Verify with `srd_decoder_load_all` proxy:

```bash
sigrok-cli --list-decoders | head
```

#### Linux (Fedora)

```bash
sudo dnf install libsigrokdecode libsigrokdecode-devel python3
```

#### macOS (Homebrew)

```bash
brew install libsigrokdecode
brew install python  # If you don't already have Python 3.10+.
```

The bridge expects `libsigrokdecode.dylib` to be on the dynamic-loader
path. If you installed via Homebrew on Apple Silicon, that's
`/opt/homebrew/lib`; on Intel it's `/usr/local/lib`. Either is on the
default loader path; if not, add it via `DYLD_LIBRARY_PATH` (note: SIP
strips this when launching from `/Applications`).

#### Windows

There is no apt/Homebrew-style package for `libsigrokdecode` on Windows.

**Current state: the Windows release archive ships the mock backend**
(the five reference decoders), so it has no runtime dependencies — just
extract and copy the files into the plugin directory (steps 2–3 above).
Bundling a real `libsigrokdecode` + Python runtime into the Windows
archive is not done yet (the Linux and macOS archives ship the real
backend). To run the full SigRok corpus on Windows today you build from
source — see [`../HOW_TO_BUILD.md`](../HOW_TO_BUILD.md) — and supply the
runtime yourself as below.

**Installing the runtime manually:** download the sigrok Windows build
from
<https://sigrok.org/wiki/Windows> — the `sigrok-cli` installer (or the
nightly zip) includes `libsigrokdecode` and the decoder corpus. Extract
`libsigrokdecode-*.dll` and its dependency DLLs (GLib, Python) **into the
same directory as `wavecrux-sigrok-bridge.exe`** so the loader finds them
at runtime.

**Building the bridge from source on Windows:** `pkg-config` is usually
absent on Windows, so `build.rs` falls back to the `LIBSIGROKDECODE_DIR`
environment variable. Point it at a prefix containing `include/` (the
headers) and `lib/` (the `sigrokdecode` import library) before building:

```powershell
$env:LIBSIGROKDECODE_DIR = "C:\sigrok\libsigrokdecode"
cargo build -p wavecrux-sigrok-bridge --features sigrok --release
```

`build.rs` adds `-I%LIBSIGROKDECODE_DIR%\include` to the bindgen clang
invocation and emits the corresponding `rustc-link-search` /
`rustc-link-lib=sigrokdecode` directives. Building with the `sigrok`
feature additionally requires LLVM/Clang on `PATH` (bindgen depends on
`libclang`). The default (mock) build needs none of this.

### 6. Restart WaveCrux

On next launch:

* The decoder picker shows `sigrok.onewire`, `sigrok.jtag`, …, etc.
* Settings → Decoders → Plugins shows
  `wavecrux_sigrok_bridge.{so,dylib,dll}` with ABI version `1.0` and
  the count of advertised decoders.
* The shim's manifest description in WaveCrux's UI carries the GPLv3+
  notice — that's invariant 4 from `CLAUDE.md`.

## Smoke test

```bash
wavecrux-sigrok-bridge --list-decoders | jq '.[] | .id'
```

Should print a list of `sigrok.<protocol>` ids — five if you're running
the mock backend (default), 130+ if you've built with the `sigrok`
feature against a real libsigrokdecode.

## Troubleshooting

### macOS: "Load failed — returned register-error rc=1" / bridge killed immediately

The bridge subprocess is being killed by macOS before it can respond.
This is the `SIGKILL (Code Signature Invalid)` / `Taskgated Invalid
Signature` crash. Cause: WaveCrux propagates `CS_EXEC_SET_KILL` to child
processes; unsigned binaries are killed before they can print anything.

Fix — run both commands from the directory containing the bridge files:

```bash
codesign --sign - --force wavecrux-sigrok-bridge
codesign --sign - --force libwavecrux_sigrok_bridge.dylib
```

Then do a full WaveCrux restart (not just "Reload plugins" — the shim
dylib must be reloaded from disk for the new signature to take effect).

If you downloaded the release archive and still see this, also remove
the quarantine flag:

```bash
xattr -d com.apple.quarantine wavecrux-sigrok-bridge libwavecrux_sigrok_bridge.dylib
codesign --sign - --force wavecrux-sigrok-bridge libwavecrux_sigrok_bridge.dylib
```

### "wavecrux-sigrok-bridge binary not found"

The shim couldn't resolve the subprocess. Check, in order:

1. `WAVECRUX_SIGROK_BRIDGE` env var — set to an absolute path.
2. Sibling install — both binaries in the same directory.
3. `which wavecrux-sigrok-bridge` returns a path.

### "ABI mismatch"

The shim and the WaveCrux build are from different ABI MAJOR versions.
Update both to the same release.

### "libsigrokdecode failed to initialize" / empty decoder list

The bridge was built with `--features sigrok` but `srd_init()` or
`srd_decoder_load_all()` failed — usually because the Python decoder
corpus is not on the default search path, or `libsigrokdecode` can't
find its companion Python runtime. Check the subprocess stderr (the
bridge logs the failing `srd_*` return code there). Confirm the runtime
is installed per the platform notes above, then re-run
`wavecrux-sigrok-bridge --list-decoders` directly. Rebuild without
`--features sigrok` to fall back to the mock decoders.

### Subprocess crashes mid-session

The shim marks every active session failed and re-spawns the
subprocess on the next session creation. To diagnose, check
`stderr` from the subprocess — the shim forwards it through its own
logger, which WaveCrux surfaces in the diagnostics panel under
"plugin logs."

### "this feature is not enabled at runtime"

You hit a code path that requires `libsigrokdecode` to be installed
(running with `--features sigrok` against a build without the runtime
present). Install libsigrokdecode per the platform notes above.

## Uninstalling

Remove `libwavecrux_sigrok_bridge.{so,dylib,dll}` from the WaveCrux
plugin directory and remove the subprocess binary from wherever you
placed it (sibling, PATH location, or env-var target). WaveCrux on
next launch will not load the bridge.
