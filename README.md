# wavecrux-sigrok-bridge

> A WaveCrux decoder plugin that brings the SigRok protocol-decoder ecosystem to
> WaveCrux without contaminating the WaveCrux codebase with GPL code.

## License notice (read this first)

**This entire repository is licensed under the GNU General Public License,
version 3 or any later version (GPLv3+).** See [`LICENSE`](LICENSE) for the
full text.

The reason is straightforward: the bridge subprocess loads
[`libsigrokdecode`](https://sigrok.org/wiki/Libsigrokdecode) and the SigRok
protocol decoders (Python), all of which are GPLv3+. Any work that links those
libraries must be GPLv3+ too. The bridge is deliberately distributed as a
**separate plugin** to keep this license boundary clean — see "Architecture"
below.

WaveCrux itself is *not* GPL. WaveCrux's open-core repo is published under
Apache 2.0 (post-beta), and the closed-source `wavecrux-pro` overlay carries
its own commercial license. Neither WaveCrux repo links any GPL code, and
this plugin is the only piece of the WaveCrux ecosystem that does.

## What this is

The bridge is an **opt-in plugin** for WaveCrux's user-contributed decoder
plugin loader (Phase 4.1 of the WaveCrux project plan). When installed, it
exposes the 130+ protocol decoders maintained by the SigRok community —
1-Wire, JTAG, PWM, DMX512, Modbus, USB low/full speed, CAN, I²S, and
many more — through WaveCrux's standard decoder picker and transaction
table.

The plugin consists of two cooperating binaries:

* **The shim** (`libwavecrux_sigrok_bridge.{so,dylib,dll}`) — a tiny native
  library implementing WaveCrux's C plugin ABI. The shim has **no GPL
  dependencies**: no `libsigrokdecode`, no `libpython`, no SigRok decoders.
  It only knows how to spawn a subprocess and exchange JSON over a pipe.
* **The bridge** (`wavecrux-sigrok-bridge[.exe]`) — a separate executable
  that hosts `libsigrokdecode` and the embedded Python interpreter. The
  WaveCrux process never loads any of those libraries directly; it only
  reads JSON from the bridge's stdout.

This split is the architectural mechanism that keeps WaveCrux's
non-GPL license boundary intact.

## License-isolation invariants

Every architectural decision in this repo preserves these four invariants.
Any change that risks any of them is rejected at code review:

1. **Separate repo.** This repository is never a Git submodule of
   `wavecrux/wavecrux` or `wavecrux/wavecrux-pro`.
2. **Process boundary.** The shim never `dlopen`s, statically links, or
   otherwise loads `libsigrokdecode` or `libpython` into its own address
   space. Communication with libsigrokdecode happens exclusively via
   spawning the bridge subprocess and exchanging JSON over a pipe.
3. **Separate distribution.** The bridge is published as a standalone
   GitHub Release on this repo. It is **not** bundled with the WaveCrux
   installer, **not** auto-downloaded by WaveCrux, and **not** advertised
   from inside the WaveCrux app without explicit user action.
4. **Explicit GPL notice.** The first paragraph of this README and the
   shim's `manifest.json` description carry an unambiguous GPLv3+ notice.

A CI workflow (`.github/workflows/isolation.yaml`) checks invariant 2
mechanically on every push by running `nm`/`ldd`/`otool`/`dumpbin` against
the shim binary and asserting that no `srd_*` or `Py*` symbols and no
`libsigrokdecode`/`libpython` linkage appears.

## Installation

The bridge is desktop-only (macOS, Linux, Windows). WaveCrux's plugin
loader is not available on iOS, Android, or web.

### 1. Download the release archive

Grab the archive matching your OS and architecture from
[GitHub Releases](https://github.com/Ferrite-Engineering/wavecrux-sigrok-bridge/releases):

* `wavecrux-sigrok-bridge-vX.Y.Z-macos-arm64.tar.gz`
* `wavecrux-sigrok-bridge-vX.Y.Z-macos-x86_64.tar.gz`
* `wavecrux-sigrok-bridge-vX.Y.Z-linux-x86_64.tar.gz`
* `wavecrux-sigrok-bridge-vX.Y.Z-windows-x86_64.zip`

Verify against `SHA256SUMS` in the same release.

### 2. Install the shim into WaveCrux's plugin directory

The default per-user plugin directory:

| Platform | Path |
|---|---|
| macOS   | `~/Library/Application Support/wavecrux/decoders/` |
| Linux   | `~/.local/share/wavecrux/decoders/` (or `$XDG_CONFIG_HOME/wavecrux/decoders/`) |
| Windows | `%APPDATA%\WaveCrux\decoders\` |

Copy `libwavecrux_sigrok_bridge.{so,dylib,dll}` into that directory. WaveCrux
scans the directory at startup; the path can be overridden with the
`WAVECRUX_DECODER_PATH` environment variable.

### 3. Make the bridge subprocess discoverable

The shim resolves the bridge subprocess in this order:

1. `WAVECRUX_SIGROK_BRIDGE` environment variable (absolute path to the
   subprocess binary).
2. Sibling binary in the same directory as the shim itself.
3. Anywhere on `PATH`.

The simplest install is to drop the bridge binary next to the shim in the
plugin directory.

### 4. Install the libsigrokdecode runtime

The bridge subprocess links `libsigrokdecode` at runtime and requires
Python 3.10 or later.

| Platform | Setup |
|---|---|
| Linux (Debian/Ubuntu) | `sudo apt install libsigrokdecode4 libsigrokdecode-dev sigrok-cli` |
| Linux (Fedora) | `sudo dnf install libsigrokdecode libsigrokdecode-devel` |
| macOS (Homebrew) | `brew install libsigrokdecode` |
| Windows | **The Windows release archive currently ships the mock backend** (the five reference decoders), because there is no `apt`/Homebrew source for `libsigrokdecode` on Windows yet. To get the full SigRok corpus on Windows today, build from source against a manually-installed `libsigrokdecode` — see [`HOW_TO_BUILD.md`](HOW_TO_BUILD.md). The Linux and macOS release archives ship the real backend (install the runtime above). |

See [`docs/INSTALL.md`](docs/INSTALL.md) for detailed per-OS instructions
and troubleshooting.

### 5. Restart WaveCrux

On next launch the bridge's protocol decoders appear in WaveCrux's decoder
picker as `sigrok.<protocol>` (for example `sigrok.onewire`, `sigrok.jtag`,
`sigrok.pwm`).

## Acceptance: five reference decoders

This repo's CI verifies end-to-end decode of five reference protocols
against committed VCD fixtures:

| Decoder | Fixture | What it exercises |
|---|---|---|
| 1-Wire | `test/fixtures/onewire/onewire_basic.vcd` | RESET, presence, READ ROM, CRC |
| JTAG | `test/fixtures/jtag/jtag_idcode.vcd` | TAP state traversal, IDCODE shift |
| PWM | `test/fixtures/pwm/pwm_steps.vcd` | Two duty steps, frequency |
| DMX512 | `test/fixtures/dmx512/dmx_two_slots.vcd` | BREAK, MAB, start code, slot data |
| Modbus | `test/fixtures/modbus/modbus_read_holding.vcd` | Slave addr, fn 0x03, CRC |

Each fixture has a `.expected_transactions.json` companion that the
integration test diffs against. Once these five pass, the bridge is
acceptance-complete; the remaining 125+ decoders are exposed through the
same path with no additional code.

## Attribution

The SigRok project at <https://sigrok.org> is the source of `libsigrokdecode`
and the protocol decoder corpus this plugin exposes. Without their work this
project would not exist. See [`NOTICES`](NOTICES) for the full attribution
and copyright list.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md). Contributions are accepted under
GPLv3+ with a Developer Certificate of Origin sign-off (`git commit -s`).

## Documentation

* [`HOW_TO_BUILD.md`](HOW_TO_BUILD.md) — build the shim and bridge from
  source (macOS, Linux, Windows), including the real `--features sigrok`
  backend, and install your build into WaveCrux.
* [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — design overview and the
  rationale for every major decision.
* [`docs/IPC_PROTOCOL.md`](docs/IPC_PROTOCOL.md) — the JSON-IPC schema and
  framing format between the shim and the bridge subprocess.
* [`docs/INSTALL.md`](docs/INSTALL.md) — detailed install and troubleshooting.
* [`docs/SECURITY.md`](docs/SECURITY.md) — process-isolation and trust model.
* [`docs/adr/`](docs/adr) — Architecture Decision Records.
