#!/usr/bin/env bash
# verify_isolation.sh — mechanical check that the shim binary respects
# license-isolation invariant 2 (process boundary, no GPL linkage).
#
# Runs `nm`/`otool`/`ldd`/`dumpbin` against the built shim and asserts
# that no `srd_*` or `Py*` symbols and no `libsigrokdecode`/`libpython`
# linkage appears. Exits non-zero on any violation.
#
# Designed to run both locally and in `.github/workflows/isolation.yaml`.
# CI runs it after every successful build of the shim crate.

set -euo pipefail

if [[ "${1:-}" == "--help" ]]; then
    cat <<EOF
verify_isolation.sh — assert the shim has no GPL linkage / symbols.

Usage:
  $0 [path/to/shim/library]

Defaults to scanning every cdylib produced under target/release.
EOF
    exit 0
fi

# Find the shim binary. On macOS it's a .dylib, on Linux a .so, on
# Windows a .dll.
candidates=()
if [[ $# -ge 1 ]]; then
    candidates+=("$1")
else
    while IFS= read -r -d $'\0' p; do
        candidates+=("$p")
    done < <(find target/release -maxdepth 1 \
        \( -name 'libwavecrux_sigrok_bridge_shim.dylib' \
        -o -name 'libwavecrux_sigrok_bridge_shim.so' \
        -o -name 'wavecrux_sigrok_bridge_shim.dll' \
        \) -print0 2>/dev/null || true)
fi

if [[ ${#candidates[@]} -eq 0 ]]; then
    echo "verify_isolation.sh: no shim binary found." >&2
    echo "Run \`cargo build --release -p wavecrux-sigrok-bridge-shim\` first." >&2
    exit 2
fi

violation=0

for binary in "${candidates[@]}"; do
    echo "==> Inspecting $binary"
    if [[ ! -f "$binary" ]]; then
        echo "  ERROR: $binary does not exist." >&2
        violation=1
        continue
    fi

    # ── Linkage check ────────────────────────────────────────────────
    case "$(uname -s)" in
        Darwin)
            linkage="$(otool -L "$binary" | tail -n +2)"
            ;;
        Linux)
            if command -v ldd >/dev/null; then
                linkage="$(ldd "$binary" 2>/dev/null || true)"
            else
                linkage="$(readelf -d "$binary" 2>/dev/null || true)"
            fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            if command -v dumpbin >/dev/null; then
                linkage="$(dumpbin /dependents "$binary" || true)"
            else
                # Fall back to objdump for MinGW environments.
                linkage="$(objdump -p "$binary" | grep 'DLL Name' || true)"
            fi
            ;;
        *)
            echo "  WARN: unrecognized platform $(uname -s); skipping linkage check"
            linkage=""
            ;;
    esac
    if echo "$linkage" | grep -iE 'sigrokdecode|libpython|python3[0-9]+\.dll' >/dev/null; then
        echo "  FAIL: forbidden GPL library appears in linkage:" >&2
        echo "$linkage" | grep -iE 'sigrokdecode|libpython|python3[0-9]+\.dll' >&2
        violation=1
    else
        echo "  OK: no libsigrokdecode / libpython linkage"
    fi

    # ── Symbol check ─────────────────────────────────────────────────
    # Real Python C API symbols are camelCase: Py_Initialize, PyEval_*,
    # PyImport_*, PyObject_*, PyModule_*, PyArg_*, PyTuple_*. Real
    # libsigrokdecode symbols are snake_case starting with `srd_`.
    # We grep case-sensitively for those exact prefixes.
    symbols=""
    case "$(uname -s)" in
        Darwin|Linux)
            symbols="$(nm -P "$binary" 2>/dev/null | awk '{print $1}' || true)"
            ;;
        MINGW*|MSYS*|CYGWIN*)
            if command -v dumpbin >/dev/null; then
                symbols="$(dumpbin /symbols "$binary" 2>/dev/null || true)"
            fi
            ;;
    esac
    pattern='(^|_)srd_|(^|_)Py_|(^|_)PyInit|(^|_)PyEval|(^|_)PyImport|(^|_)PyObject|(^|_)PyModule|(^|_)PyArg|(^|_)PyTuple'
    if echo "$symbols" | grep -E "$pattern" >/dev/null; then
        echo "  FAIL: forbidden GPL symbols present:" >&2
        echo "$symbols" | grep -E "$pattern" >&2 | head -20
        violation=1
    else
        echo "  OK: no srd_* / Py* symbols"
    fi
done

if [[ $violation -ne 0 ]]; then
    echo
    echo "License-isolation invariant 2 (process boundary) is violated." >&2
    echo "Refer to CLAUDE.md and docs/SECURITY.md before relaxing the gate." >&2
    exit 1
fi

echo
echo "All license-isolation checks passed."
