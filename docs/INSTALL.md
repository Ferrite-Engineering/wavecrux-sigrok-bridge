# Installation guide

The bridge is a desktop-only opt-in plugin. It runs on Linux, macOS,
and Windows alongside WaveCrux on the same machine.

## Step-by-step

### 1. Confirm prerequisites

| Requirement | Why |
|---|---|
| WaveCrux ≥ X.Y (Phase 4.1 plugin loader) | The bridge plugs into the WaveCrux user-contributed decoder loader. |
| Python 3.10 or later, accessible via the system | The bridge subprocess embeds Python at runtime. Linux/macOS use the system or Homebrew Python; the Windows release archive bundles the official embeddable distribution. |
| `libsigrokdecode` runtime + the SigRok decoder set | The subprocess loads these on startup. See platform-specific instructions below. |

### 2. Download the release archive

Grab the archive matching your OS+architecture from
[GitHub Releases](https://github.com/wavecrux/wavecrux-sigrok-bridge/releases),
along with the matching `*.sha256` companion. Verify the checksum before
extracting:

```bash
shasum -a 256 -c wavecrux-sigrok-bridge-vX.Y.Z-<os>-<arch>.tar.gz.sha256
```

### 3. Extract and copy the shim into WaveCrux's plugin directory

The default per-user plugin directories WaveCrux scans:

| Platform | Path |
|---|---|
| macOS | `~/Library/Application Support/wavecrux/decoders/` |
| Linux | `~/.local/share/wavecrux/decoders/` (or `$XDG_CONFIG_HOME/wavecrux/decoders/`) |
| Windows | `%APPDATA%\WaveCrux\decoders\` |

Copy `libwavecrux_sigrok_bridge.{so,dylib,dll}` into that directory.

To override the directory, set `WAVECRUX_DECODER_PATH` in your
environment before launching WaveCrux.

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

The release archive bundles Windows builds of `libsigrokdecode`, the
SigRok decoder corpus, and the official Python embeddable
distribution. No separate install is needed.

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

### "wavecrux-sigrok-bridge binary not found"

The shim couldn't resolve the subprocess. Check, in order:

1. `WAVECRUX_SIGROK_BRIDGE` env var — set to an absolute path.
2. Sibling install — both binaries in the same directory.
3. `which wavecrux-sigrok-bridge` returns a path.

### "ABI mismatch"

The shim and the WaveCrux build are from different ABI MAJOR versions.
Update both to the same release.

### "real libsigrokdecode backend not implemented yet"

The bridge was built with `--features sigrok` against a backend whose
implementation is not yet complete (see `crates/bridge/src/srd.rs`).
Rebuild without `--features sigrok` to use the mock decoders, or wait
for the libsigrokdecode hookup to ship.

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
