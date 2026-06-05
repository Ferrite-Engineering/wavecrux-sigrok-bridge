//! Build script for the WaveCrux SigRok bridge.
//!
//! In the default (mock) build this is a no-op. When the `sigrok`
//! feature is active it runs `bindgen` against
//! `<libsigrokdecode/libsigrokdecode.h>` (plus the GLib symbols the
//! bridge needs) and emits the linker directives to pull in
//! libsigrokdecode + GLib.
//!
//! Library discovery has two strategies, tried in order:
//!
//!   1. `pkg-config` — the normal path on Linux and macOS (Homebrew /
//!      apt ship a `libsigrokdecode.pc`). pkg-config emits the
//!      `rustc-link-lib` / `rustc-link-search` directives itself and
//!      hands us the include paths (including GLib's) for bindgen.
//!   2. `LIBSIGROKDECODE_DIR` env var — the fallback for Windows, where
//!      pkg-config is usually absent. The variable points at a prefix
//!      containing `include/` (headers) and `lib/` (the import lib). We
//!      emit the link directives manually and feed `-I<dir>/include` to
//!      libclang so bindgen can parse the headers without a system-wide
//!      install. See docs/INSTALL.md (Windows section) for details.

fn main() {
    // Re-run if the wiring inputs change. Harmless in the mock build.
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=LIBSIGROKDECODE_DIR");

    #[cfg(feature = "sigrok")]
    sigrok_bindings();
}

#[cfg(feature = "sigrok")]
fn sigrok_bindings() {
    use std::env;
    use std::path::PathBuf;

    let out_path =
        PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR set by cargo")).join("srd_bindings.rs");

    let mut builder = bindgen::Builder::default()
        .header("wrapper.h")
        // libsigrokdecode surface.
        .allowlist_function("srd_.*")
        .allowlist_type("srd_.*|SRD_.*")
        .allowlist_var("SRD_.*")
        // The GLib symbols the bridge calls directly. GSList is walked
        // by hand via its generated struct (data/next), so no g_slist_*
        // functions are needed.
        .allowlist_function("g_variant_.*")
        .allowlist_function("g_hash_table_.*")
        .allowlist_function("g_strdup")
        .allowlist_function("g_free")
        .allowlist_function("g_str_hash")
        .allowlist_function("g_str_equal")
        // Keep the generated surface small and stable.
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .layout_tests(false);

    // Strategy 1: pkg-config (Linux/macOS). probe_library emits the
    // link directives itself and returns the include paths.
    let mut found_via_pkgconfig = false;
    match pkg_config::Config::new()
        .print_system_libs(false)
        .probe("libsigrokdecode")
    {
        Ok(lib) => {
            for inc in &lib.include_paths {
                builder = builder.clang_arg(format!("-I{}", inc.display()));
            }
            found_via_pkgconfig = true;
        }
        Err(e) => {
            println!("cargo:warning=pkg-config could not find libsigrokdecode ({e}); falling back to LIBSIGROKDECODE_DIR");
        }
    }

    // Strategy 2: LIBSIGROKDECODE_DIR (Windows / non-pkg-config hosts).
    if let Ok(dir) = env::var("LIBSIGROKDECODE_DIR") {
        builder = builder.clang_arg(format!("-I{dir}/include"));
        if !found_via_pkgconfig {
            println!("cargo:rustc-link-search=native={dir}/lib");
            println!("cargo:rustc-link-lib=sigrokdecode");
        }
    } else if !found_via_pkgconfig {
        panic!(
            "libsigrokdecode not found: install it so pkg-config can locate it \
             (e.g. `brew install libsigrokdecode` / `apt install libsigrokdecode-dev`), \
             or set LIBSIGROKDECODE_DIR to a prefix containing include/ and lib/."
        );
    }

    let bindings = builder
        .generate()
        .expect("bindgen failed to generate libsigrokdecode bindings");
    bindings
        .write_to_file(&out_path)
        .expect("failed to write srd_bindings.rs");
}
