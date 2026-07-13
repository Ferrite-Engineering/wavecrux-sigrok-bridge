# Building wavecrux-sigrok-bridge from source

This guide builds the SigRok bridge **from source** and installs it into
WaveCrux, on macOS, Linux, and Windows. It covers two audiences:

* **Beta testers** who want to try the bridge before there are signed
  release archives — there is a copy-paste
  [macOS quick path](#beta-tester-quick-path-macos) at the end.
* **End users and packagers** (post-beta, once this repo is public) who
  want to build the bridge themselves rather than download a release.

If you just want to *install a prebuilt release archive*, you don't need
this file — see [`docs/INSTALL.md`](docs/INSTALL.md) instead. If you want
to *cut* a release, see [`docs/RELEASING.md`](docs/RELEASING.md).

## License note (read this first)

**This entire repository is GPLv3-or-later**, because the bridge links
`libsigrokdecode` and the SigRok protocol decoders (Python), which are
GPLv3+. Building it from source produces GPLv3+ binaries. WaveCrux itself
is *not* GPL — the bridge is a separate, opt-in plugin precisely so that
license boundary stays clean. See [`README.md`](README.md) and
[`LICENSE`](LICENSE) for the full rationale.

## What you'll build

The plugin is **two cooperating binaries** (the same split described in
the [README](README.md#what-this-is)):

| Binary | File | Links GPL code? |
|---|---|---|
| **Shim** | `libwavecrux_sigrok_bridge.{so,dylib,dll}` | **No.** Pure Rust; only spawns the bridge and exchanges JSON over a pipe. |
| **Bridge** | `wavecrux-sigrok-bridge[.exe]` | **Yes.** Hosts `libsigrokdecode` + embedded Python. Runs as a subprocess; WaveCrux never loads it into its own address space. |

### Two build flavors: mock vs. real

The bridge subprocess has a Cargo feature, `sigrok`, that decides which
decoder backend it carries:

| Build | Command | Decoders advertised | GPL runtime deps |
|---|---|---|---|
| **Mock** (default) | `cargo build --release --workspace` | 5 hardcoded reference decoders (1-Wire, JTAG, PWM, DMX512, Modbus) with scripted output | **None** — pure Rust |
| **Real** | `cargo build --release --workspace --features sigrok` | The full installed `libsigrokdecode` corpus (130+ on a full Linux install; ~111 on Homebrew 0.5.3) | `libsigrokdecode` + Python 3.10+ |

The mock build needs nothing but a Rust toolchain and is what the test
suite and CI run on. **To actually get the SigRok decoders you want the
`--features sigrok` build** — that is the focus of this guide.

The shim is identical in both flavors and has no `sigrok` feature; the
`--features sigrok` flag only affects the bridge crate.

---

## 1. Prerequisites (all platforms)

Install the Rust toolchain via [rustup](https://rustup.rs):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # macOS / Linux
# Windows: download and run rustup-init.exe from https://rustup.rs
```

The repo pins the **stable** channel with `rustfmt` and `clippy` (see
[`rust-toolchain.toml`](rust-toolchain.toml)); rustup installs the right
toolchain automatically the first time you run `cargo` in the repo.

Then clone the repo (it has no submodules):

```bash
git clone https://github.com/Ferrite-Engineering/wavecrux-sigrok-bridge.git
cd wavecrux-sigrok-bridge
```

### Sanity check: build the mock first

Before pulling in the GPL toolchain, confirm your Rust setup works by
building the default (mock) flavor and running the tests:

```bash
cargo build --release --workspace
cargo test  --workspace
```

If that succeeds you have a working `libwavecrux_sigrok_bridge.dylib`
(/`.so`/`.dll`) and `wavecrux-sigrok-bridge` binary under
`target/release/`, advertising the five mock decoders. Now add the real
backend.

---

## 2. Install the SigRok build dependencies

The `--features sigrok` build needs, at build time:

* **`libsigrokdecode`** + its development headers,
* **Python 3.10+** (libsigrokdecode embeds it),
* **`pkg-config`** (Linux/macOS — used to locate libsigrokdecode),
* **`libclang`** — the build uses `bindgen` to parse the C headers, and
  bindgen loads `libclang` at build time. Where it comes from depends on
  the platform (Apple's Command Line Tools on macOS, `libclang-dev` on
  Linux, an LLVM install on Windows); the per-OS steps below cover it, so
  you don't install it separately on macOS.

### macOS (Homebrew)

```bash
brew install libsigrokdecode pkg-config
# Homebrew's libsigrokdecode pulls in glib and a python@3.x automatically.
```

That's the whole list. bindgen also needs `libclang`, but Apple's
Command Line Tools already provide it (and if you have Homebrew you
already have the Command Line Tools), so there's normally nothing extra
to install — this is the exact set CI builds with.

**Only if** the build later fails with a "can't find libclang" /
"unable to find libclang" error, install LLVM and point bindgen at it:

```bash
brew install llvm
export LIBCLANG_PATH="$(brew --prefix llvm)/lib"
```

### Linux (Debian / Ubuntu)

```bash
sudo apt update
sudo apt install \
  libsigrokdecode-dev libsigrokdecode4 sigrok-cli \
  python3-dev pkg-config libclang-dev clang
```

`sigrok-cli` pulls in the full Python decoder corpus. `libclang-dev`
provides the `libclang` that bindgen needs.

### Linux (Fedora)

```bash
sudo dnf install \
  libsigrokdecode libsigrokdecode-devel \
  python3-devel pkgconf-pkg-config clang-devel
```

### Windows (MSVC)

Windows has no package manager for `libsigrokdecode`, so you supply it
manually:

1. Install **Rust (MSVC toolchain)** via rustup, plus the **Visual Studio
   Build Tools** (MSVC `cl.exe`).
2. Install **LLVM** (for `libclang`) — e.g. `winget install LLVM.LLVM` —
   and make sure `bin\` is on `PATH`, or set `LIBCLANG_PATH` to the LLVM
   `bin` directory.
3. Download a sigrok Windows build from
   <https://sigrok.org/wiki/Windows> (the `sigrok-cli` installer / nightly
   zip includes `libsigrokdecode` and the decoder corpus).
4. Point the build at it via the `LIBSIGROKDECODE_DIR` environment
   variable. It must contain `include/` (headers) and `lib/` (the
   `sigrokdecode` import library):

   ```powershell
   $env:LIBSIGROKDECODE_DIR = "C:\sigrok\libsigrokdecode"
   ```

   The build's `build.rs` adds `-I%LIBSIGROKDECODE_DIR%\include` to the
   bindgen invocation and emits the matching link directives. (On
   Linux/macOS `pkg-config` does this automatically; `LIBSIGROKDECODE_DIR`
   is the no-pkg-config fallback.)

> **Note on Python versions.** The bridge links `libsigrokdecode`, which
> in turn links a specific Python (e.g. Homebrew's `python@3.14`). The
> build pulls in PyO3 only to link `libpython`; it is pinned to a current
> release that tracks the latest CPython. If a future CPython outpaces
> PyO3, bump the `pyo3` dependency in
> [`crates/bridge/Cargo.toml`](crates/bridge/Cargo.toml) to the newest
> release.

---

## 3. Build with the real SigRok corpus

From the repo root:

```bash
cargo build --release --workspace --features sigrok
```

This produces, under `target/release/`:

* `libwavecrux_sigrok_bridge.{dylib,so,dll}` — the shim
* `wavecrux-sigrok-bridge[.exe]` — the bridge (now carrying real
  libsigrokdecode)

### Verify the build sees the decoders

Ask the bridge to dump its decoder catalog as JSON:

```bash
./target/release/wavecrux-sigrok-bridge --list-decoders | \
  python3 -c "import json,sys; d=json.load(sys.stdin); print(len(d), 'decoders')"
```

You should see 100+ decoders (exact count depends on your installed
`libsigrokdecode` version). If you instead see `5 decoders`, you built
the mock flavor — re-run with `--features sigrok`.

---

## 4. Install into WaveCrux

WaveCrux discovers plugins in a per-user directory. The easiest way to
find it: open WaveCrux → **Settings → Decoders → Plugins → Open plugin
directory** (this creates the directory and reveals it in your file
manager). Or locate it manually:

| Platform | Default plugin directory |
|---|---|
| macOS | `~/Library/Application Support/<bundle-id>/wavecrux/decoders/` — the bundle id varies by build (e.g. `com.ferriteengineering.wavecruxPro` for WaveCrux Pro); use the in-app button to avoid guessing. |
| Linux | `~/.local/share/wavecrux/decoders/` (or `$XDG_CONFIG_HOME/wavecrux/decoders/`) |
| Windows | `%APPDATA%\WaveCrux\decoders\` |

Copy **both** binaries into that directory (the shim resolves the bridge
as a sibling — see [README §3](README.md#3-make-the-bridge-subprocess-discoverable)):

```bash
# macOS example — adjust DEST to the path the "Open plugin directory" button revealed
DEST="$HOME/Library/Application Support/com.ferriteengineering.wavecruxPro/wavecrux/decoders"
mkdir -p "$DEST"
cp target/release/libwavecrux_sigrok_bridge.dylib "$DEST/"
cp target/release/wavecrux-sigrok-bridge          "$DEST/"
```

(On Linux the shim is `.so`; on Windows it's `wavecrux_sigrok_bridge.dll`
plus `wavecrux-sigrok-bridge.exe`, and you must also place
`libsigrokdecode`'s DLLs + the Python runtime alongside the bridge — see
[`docs/INSTALL.md`](docs/INSTALL.md), Windows section.)

### macOS only: code-sign the binaries

WaveCrux is built with the `CS_EXEC_SET_KILL` hardening flag, so macOS
**kills any unsigned subprocess** it spawns. Locally built binaries are
unsigned, so you must ad-hoc sign both files after copying them:

```bash
codesign --sign - --force "$DEST/wavecrux-sigrok-bridge"
codesign --sign - --force "$DEST/libwavecrux_sigrok_bridge.dylib"
```

`--sign -` is an *ad-hoc* signature: no Apple Developer account needed,
cryptographically valid, and enough to pass the kernel's `CS_KILL` check.
Re-run these two commands every time you rebuild and re-copy. (Release
archives from CI are ad-hoc signed automatically; v1.0 will ship a
notarized Developer ID signature — see
[`docs/RELEASING.md`](docs/RELEASING.md#macos-code-signing-status).)

If you downloaded rather than built the binaries, also strip the
download quarantine flag: `xattr -d com.apple.quarantine <file>`.

### Restart WaveCrux

Do a **full quit and relaunch** (not "Reload plugins" — the dylib must be
re-loaded from disk). Then check **Settings → Decoders → Plugins**: the
card should read **WaveCrux SigRok Bridge**, status `Loaded`, `ABI v1.1`,
with the decoder count beneath it. The decoders appear in the decoder
picker as `sigrok.<protocol>` (e.g. `sigrok.i2c`, `sigrok.jtag`).

---

## 5. Verifying your build

| Check | Command |
|---|---|
| Mock tests pass | `cargo test --workspace` |
| Real-backend tests pass (needs the deps from §2) | `cargo test --workspace --features sigrok` |
| Formatting / lint (what CI enforces) | `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` |
| **License isolation** — the shim must carry *no* GPL symbols | `bash tool/verify_isolation.sh` |

The isolation check is the hard gate that keeps the GPL boundary intact:
it runs `nm`/`otool`/`ldd`/`dumpbin` over the shim and fails if any
`srd_*` / `Py*` symbol or `libsigrokdecode`/`libpython` linkage appears.
Run it after any change near the shim.

---

## Troubleshooting build failures

**`pkg-config` / "libsigrokdecode not found" during `cargo build --features sigrok`.**
The dependency isn't installed or isn't discoverable. Confirm
`pkg-config --modversion libsigrokdecode` prints a version on Linux/macOS.
On Windows set `LIBSIGROKDECODE_DIR` (§2). Without the `sigrok` feature
the build never needs any of this.

**bindgen: "unable to find libclang" / "couldn't find libclang".**
bindgen needs `libclang`. Install LLVM (`brew install llvm`,
`apt install libclang-dev`, `dnf install clang-devel`, or LLVM on
Windows) and, if needed, set `LIBCLANG_PATH` to the directory containing
`libclang` (`$(brew --prefix llvm)/lib` on macOS).

**PyO3: "the configured Python interpreter version … is newer than PyO3's maximum supported version".**
Your `libsigrokdecode` links a CPython newer than the pinned PyO3
supports. Bump `pyo3` in
[`crates/bridge/Cargo.toml`](crates/bridge/Cargo.toml) to the latest
release, or point the build at an older Python via `PYO3_PYTHON`.

**macOS: the bridge is killed instantly / "Load failed".**
The binary is unsigned (or you rebuilt and forgot to re-sign). Re-run the
two `codesign --sign - --force` commands from §4 and do a full WaveCrux
restart. This and other *runtime* issues (subprocess not found, ABI
mismatch, libsigrokdecode missing at runtime) are covered in detail in
[`docs/INSTALL.md`](docs/INSTALL.md#troubleshooting).

**`--features sigrok` build is slow the first time.**
It compiles `bindgen`, `pyo3`, and friends. Subsequent builds are
incremental and fast.

---

## Beta tester quick path (macOS)

Zero to working SigRok decoders on an Apple-silicon Mac, copy-paste:

```bash
# 1. Toolchains & SigRok runtime
#    (libclang comes from Apple's Command Line Tools — no llvm needed;
#     if the build later can't find libclang, see the macOS deps section.)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
brew install libsigrokdecode pkg-config

# 2. Clone & build the real backend
git clone https://github.com/Ferrite-Engineering/wavecrux-sigrok-bridge.git
cd wavecrux-sigrok-bridge
cargo build --release --workspace --features sigrok

# 3. Confirm the bridge sees the corpus (expect 100+)
./target/release/wavecrux-sigrok-bridge --list-decoders | \
  python3 -c "import json,sys; print(len(json.load(sys.stdin)), 'decoders')"

# 4. Install into WaveCrux's plugin directory.
#    Get the exact path from WaveCrux: Settings → Decoders → Plugins →
#    "Open plugin directory", then set DEST to it. Example for WaveCrux Pro:
DEST="$HOME/Library/Application Support/com.ferriteengineering.wavecruxPro/wavecrux/decoders"
mkdir -p "$DEST"
cp target/release/libwavecrux_sigrok_bridge.dylib "$DEST/"
cp target/release/wavecrux-sigrok-bridge          "$DEST/"

# 5. Ad-hoc code-sign both (required — macOS kills unsigned subprocesses)
codesign --sign - --force "$DEST/wavecrux-sigrok-bridge"
codesign --sign - --force "$DEST/libwavecrux_sigrok_bridge.dylib"

# 6. Fully quit and relaunch WaveCrux. Settings → Decoders → Plugins should
#    show "WaveCrux SigRok Bridge", Loaded, ABI v1.1, with the decoder count.
```

If anything misbehaves, the
[Troubleshooting](#troubleshooting-build-failures) section above and
[`docs/INSTALL.md`](docs/INSTALL.md#troubleshooting) cover the common
cases. When reporting back, the most useful artifact is the output of
`./target/release/wavecrux-sigrok-bridge --list-decoders` and anything
WaveCrux logs under Settings → Decoders (the shim forwards the bridge's
stderr there).

---

## Related documentation

* [`README.md`](README.md) — what the bridge is, the license-isolation
  invariants, and installing a prebuilt release.
* [`docs/INSTALL.md`](docs/INSTALL.md) — installing release archives and
  runtime troubleshooting (per-OS).
* [`docs/RELEASING.md`](docs/RELEASING.md) — cutting tagged releases and
  the macOS code-signing roadmap.
* [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — design and the rationale
  for the two-binary split.
* [`CONTRIBUTING.md`](CONTRIBUTING.md) — dev workflow, DCO sign-off, and
  the checks CI runs.
