# Prompt: Implement the Real libsigrokdecode Backend

This document is a self-contained Claude Code prompt. Feed it verbatim
to a new Claude Code session in the `wavecrux-sigrok-bridge` repository
root to drive the full implementation.

---

## Goal

Replace the stub in `crates/bridge/src/srd.rs` with a working
implementation that connects `libsigrokdecode` (via bindgen-generated
C bindings) and Python (via PyO3) to the existing IPC loop. When built
with `cargo build --features sigrok`, the subprocess must advertise and
run the full libsigrokdecode decoder corpus (130+ decoders) instead of
the five hardcoded mock decoders.

The mock backend (`crates/bridge/src/mock.rs`) must continue to compile
and pass tests when the `sigrok` feature is absent. Every existing
`cargo test --workspace` test must continue to pass.

---

## Repository Layout

```
wavecrux-sigrok-bridge/
├── Cargo.toml                   # workspace root
├── crates/
│   ├── ipc/src/schema.rs        # all IPC wire types (READ THIS FIRST)
│   ├── bridge/
│   │   ├── Cargo.toml           # bridge crate — deps + feature flags
│   │   ├── src/
│   │   │   ├── main.rs          # CLI + backend dispatch (READ THIS)
│   │   │   ├── backend.rs       # Backend trait (in main.rs as inline mod)
│   │   │   ├── ipc_loop.rs      # request→backend→response dispatch
│   │   │   ├── mock.rs          # working reference implementation
│   │   │   └── srd.rs           # STUB — replace this entirely
│   │   └── tests/
│   │       └── end_to_end.rs    # integration tests using mock fixtures
│   └── shim/                    # GPL-free WaveCrux plugin (do NOT touch)
├── docs/INSTALL.md              # deployment instructions
└── tool/generate_fixtures/      # fixture generator (mock-mode only)
```

**Read these files before writing any code:**
1. `crates/ipc/src/schema.rs` — every type you must produce or consume
2. `crates/bridge/src/mock.rs` — the complete behavioral reference
3. `crates/bridge/src/main.rs` — the `Backend` trait and how `srd.rs` plugs in
4. `crates/bridge/Cargo.toml` — existing deps; `pyo3` is already declared optional

---

## What the Backend Trait Requires

Defined inline in `crates/bridge/src/main.rs`:

```rust
pub trait Backend: Send {
    fn list_decoders(&self) -> Result<Vec<DecoderManifest>>;
    fn create_session(&mut self, body: CreateSessionBody) -> Result<SessionId>;
    fn feed(&mut self, body: FeedBody) -> Result<Vec<AnnotationEvent>>;
    fn finalize(&mut self, session: &SessionId) -> Result<Vec<AnnotationEvent>>;
    fn destroy(&mut self, session: &SessionId) -> Result<()>;
}
```

The IPC loop in `ipc_loop.rs` calls these methods synchronously and
routes the results to the shim. **Annotations are returned by value**
from `feed` and `finalize` — there is no callback-to-channel dance at
the IPC boundary. Internally the implementation may use a callback; the
output just needs to be collected before returning.

---

## What to Implement

### 1. `crates/bridge/build.rs`  *(create new file)*

Run `bindgen` against `<libsigrokdecode/libsigrokdecode.h>` only when
the `sigrok` feature is active. The output goes to
`$OUT_DIR/srd_bindings.rs`.

```rust
// build.rs skeleton — flesh out as needed
fn main() {
    #[cfg(feature = "sigrok")]
    {
        // 1. Use pkg-config to find libsigrokdecode:
        //      pkg_config::probe_library("libsigrokdecode")
        //    or fall back to manual LIBSIGROKDECODE_DIR env var.
        // 2. Run bindgen:
        //      bindgen::Builder::default()
        //        .header("wrapper.h")  // #include <libsigrokdecode/libsigrokdecode.h>
        //        .allowlist_function("srd_.*")
        //        .allowlist_type("srd_.*|SRD_.*")
        //        .allowlist_var("SRD_.*")
        //        .generate()
        //        .write_to_file(out_path)
        // 3. Emit cargo:rustc-link-lib=sigrokdecode
    }
}
```

Add `bindgen` and `pkg-config` to `[build-dependencies]` in
`crates/bridge/Cargo.toml` (feature-gate them if possible, or gate with
`#[cfg(feature = "sigrok")]` inside `build.rs`).

### 2. `crates/bridge/src/srd.rs`  *(replace stub)*

Implement `SrdBackend` as a complete replacement for the stub.  The
file already has the GPLv3+ license notice; keep it.

**Key design points:**

#### Initialization

libsigrokdecode must be initialized once per process before any other
call. `srd_init(NULL)` uses the default decoder search path (the
installed Python decoder corpus). Call it in `SrdBackend::new()` and
call `srd_exit()` in `SrdBackend`'s `Drop`.

PyO3 note: the feature flag in `Cargo.toml` currently has
`pyo3 = { features = ["auto-initialize"] }`. For libsigrokdecode
use change this to `auto-initialize = false` — `srd_init` initializes
Python; letting PyO3 do it first causes a double-init crash.

#### `list_decoders`

```
srd_decoder_load_all()   → fills the global decoder list
srd_decoder_list()       → GSList* of srd_decoder*
```

For each `srd_decoder*` build a `DecoderManifest`:
- `id` = `"sigrok."` + `decoder->id`
- `display_name` = `decoder->longname` (or `decoder->name` as fallback)
- `description` = `decoder->desc`
- `channels` = from `decoder->channels` (required) + `decoder->optional_channels`
- `options` = from `decoder->options` (a `GSList<srd_decoder_option*>`)
- `annotations` = from `decoder->annotations` (a `GSList<char**>` — each entry is `[short_name, long_name]`)
- `tags` = from `decoder->tags` (a `GSList<char*>`)

`OptionKind` mapping from `srd_decoder_option.def_val` GVariant type:
| GVariant type | `OptionKind` |
|---|---|
| `G_VARIANT_TYPE_STRING` | `Enum` if choices non-empty, else `String` |
| `G_VARIANT_TYPE_INT64` | `Int` |
| `G_VARIANT_TYPE_DOUBLE` | `Float` |
| `G_VARIANT_TYPE_BOOLEAN` | `Bool` |

#### `create_session`

```
srd_session_new(&session)
srd_inst_new(session, decoder_id_without_sigrok_prefix, options_hashtable)
srd_inst_channel_set_all(session, instance, channel_map)
srd_session_start(session)
```

Store `(srd_session*, srd_decoder_inst*, annotation_accumulator)` keyed
by the generated `SessionId`.

Register an annotation callback **before** `srd_session_start`:
```c
srd_pd_output_callback_add(session, SRD_OUTPUT_ANN, annotation_cb, ctx_ptr);
```

The callback signature:
```c
void annotation_cb(srd_proto_data *pdata, void *cb_data);
```

`pdata->data` is a `srd_proto_data_annotation*` when type is
`SRD_OUTPUT_ANN`. Pull out `ann_class` and `ann_text[0]` (the longest
annotation string). Store into the accumulator so `feed`/`finalize` can
drain it.

#### `feed` — sample conversion

The IPC sends **transitions** (sparse, femtosecond-timestamped).
libsigrokdecode expects **dense sample arrays** (one byte-row per
sample, N bytes per row where N = ceil(channels / 8)).

Conversion strategy:
1. Track the last known bit level for every channel (a `Vec<u8>` of
   length = number of declared channels).
2. Sort all transitions in this `FeedBody` by `fs`.
3. For each pair of consecutive transitions at `t0` and `t1`: generate
   one sample representing the level *at* `t0`, run it through
   `srd_session_send`. Use a fixed scale of **1 sample = 1 ns = 1e6 fs**
   so sample_number = `fs / 1_000_000`. This gives 1 GHz resolution
   which is sufficient for any protocol libsigrokdecode supports.
4. Pack bits LSB-first, one channel per bit: channel 0 in bit 0 of byte
   0, channel 7 in bit 7 of byte 0, channel 8 in bit 0 of byte 1, etc.
   (This is `sigrok`'s native bit order.)

`srd_session_send` signature:
```c
int srd_session_send(srd_session *sess,
                     uint64_t start_samplenum,
                     uint64_t end_samplenum,
                     const uint8_t *inbuf,
                     uint64_t inbuflen,   /* bytes */
                     uint64_t unitsize);  /* bytes per sample */
```

After each `srd_session_send`, drain the annotation accumulator and
return collected `AnnotationEvent`s. Convert annotation sample numbers
back to femtoseconds: `fs = sample_num * 1_000_000`.

#### `finalize`

Call `srd_session_terminate_reset(session)` to flush any pending state,
then drain the accumulator.

#### `destroy`

Call `srd_session_destroy(session)` and remove the entry from the map.

#### Thread safety

libsigrokdecode is **not thread-safe**. All calls go on the same thread
(the IPC loop runs single-threaded; `Backend` takes `&mut self` for all
mutating calls). This is sufficient — no extra locking needed.

The annotation callback runs synchronously on the calling thread inside
`srd_session_send`, so pushing into a `Vec` or `VecDeque` is safe
without a mutex (provided the callback closure is not shared across
threads).

### 3. `crates/bridge/Cargo.toml`  *(update)*

- Change `pyo3` feature to `auto-initialize = false`:
  ```toml
  pyo3 = { version = "0.22", features = [], optional = true }
  ```
- Add `bindgen` and `pkg-config` to `[build-dependencies]`, gated:
  ```toml
  [build-dependencies]
  # Only compiled when the sigrok feature is active (build.rs gates it).
  bindgen = "0.69"
  pkg-config = "0.3"
  ```

---

## GLib Type Handling

libsigrokdecode uses GLib types (`GSList`, `GHashTable`, `GVariant`).
Options for Rust interop:

**Preferred — raw pointer traversal with `glib-sys`:**
Add `glib-sys = { version = "0.20", optional = true }` under
`[target.'cfg(feature = "sigrok")'.dependencies]` and use its typed
wrappers for iteration. This avoids writing your own `unsafe` GLib glue.

**Alternative — manual unsafe:**
Walk `GSList` with `(*list).data` / `(*list).next` casts. Write a small
helper:
```rust
unsafe fn gslist_iter<T>(mut list: *const GLib_GSList) -> Vec<*const T> {
    let mut out = vec![];
    while !list.is_null() {
        out.push((*list).data as *const T);
        list = (*list).next;
    }
    out
}
```

`GVariant` values from `srd_decoder_option.def_val` can be inspected
with `g_variant_get_type_string` and extracted with `g_variant_get_int64`,
`g_variant_get_string`, etc. Either bind these from GLib or add
`glib-sys` for the convenience wrappers.

---

## Acceptance Criteria

1. **`cargo build --workspace`** (no features) passes without warnings.
2. **`cargo build --workspace --features sigrok`** passes on a machine
   with `libsigrokdecode` and Python 3.10+ installed (e.g., after
   `brew install libsigrokdecode` on macOS).
3. **`cargo test --workspace`** (no features) — all tests pass.
4. **`cargo test --workspace --features sigrok`** — all tests pass.
5. **`./target/release/wavecrux-sigrok-bridge --list-decoders`** (sigrok
   build) prints ≥ 130 decoder manifests as JSON, including at minimum:
   `sigrok.onewire`, `sigrok.jtag`, `sigrok.pwm`, `sigrok.dmx512`,
   `sigrok.modbus`.
6. **Smoke test against the five reference fixtures** in
   `test/fixtures/{onewire,jtag,pwm,dmx512,modbus}/` — load each VCD,
   run through the bridge (sigrok build), and confirm that
   `.expected_transactions.json` companions still match or that the
   real libsigrokdecode output is a strict superset (more annotation
   detail is fine; missing annotations are not).
7. The existing **mock backend** (`cargo build` with no features) must
   remain completely unchanged in behavior — the five reference
   fixtures must still round-trip identically.

---

## macOS: Ad-hoc Codesign After Build

On macOS, the WaveCrux app propagates `CS_EXEC_SET_KILL` to child
processes. Any binary placed in the WaveCrux plugin directory must carry
a valid code signature or the kernel kills it immediately. After
installing the newly built binaries:

```bash
codesign --sign - --force /path/to/decoders/wavecrux-sigrok-bridge
codesign --sign - --force /path/to/decoders/libwavecrux_sigrok_bridge.dylib
```

The CI release workflow already does this automatically for release
archives. For local development builds, run these two commands after
each `cp` to the plugin directory.

---

## Constraints

- **GPL boundary**: `srd.rs` is in the bridge subprocess, which is
  GPLv3+. You may freely use GLib, libsigrokdecode, and Python C APIs
  here. Do NOT touch anything in `crates/shim/` — the shim is GPL-free
  and must stay that way.
- **No changes to `crates/ipc/`** — the wire protocol is frozen. Map
  libsigrokdecode data into the existing `DecoderManifest`,
  `AnnotationEvent`, etc. types.
- **No changes to `crates/bridge/src/mock.rs`** — the mock backend is
  the correctness reference and must remain identical for the no-features
  build.
- **No changes to `crates/bridge/src/ipc_loop.rs`** — the dispatch
  layer is complete. Your implementation only needs to satisfy the
  `Backend` trait.
- The `srd.rs` file must remain GPLv3+ licensed (the license notice is
  already in the stub — keep it).
- Do not add any dependency to `crates/shim/` that could introduce GPL
  symbols. The CI `isolation.yaml` workflow will reject it.

---

## Useful References

- libsigrokdecode C API header (after `brew install libsigrokdecode`):
  `/opt/homebrew/include/libsigrokdecode/libsigrokdecode.h`
- sigrok project C API docs: https://sigrok.org/api/libsigrokdecode/unstable/
- PyO3 "without Python" / `auto-initialize = false` guide:
  https://pyo3.rs/latest/python-from-rust/calling-existing-code
- The annotation output path: `SRD_OUTPUT_ANN` in the libsigrokdecode
  header, `srd_proto_data_annotation` struct.
