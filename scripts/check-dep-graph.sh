#!/usr/bin/env bash
# ==============================================================================
# Radivine v0.3.0-rc.1 — Architecture Dependency-Graph Boundary Verification (`X7`)
# ==============================================================================
# Verifies that workspace crates and binaries strictly respect the approved
# dependency boundaries (`ADR-027`, `ADR-028`, `ADR-029`, `ADR-032`, `ADR-033`):
#   1. `radivine-wheel` depends only on `smallvec` at runtime.
#   2. `radivine-convert` is codec-free, SQLite-free, and GUI/X11-free.
#   3. `radivine-jobs` is a lightweight wire/client crate (no SQLite, no codecs, no GUI).
#   4. `radivine` (wheel daemon) never links `radivine-convert-image`, `radivine-webp`,
#      `libwebp-sys`, `jpeg-encoder`, `image`, or `rusqlite` (`libsqlite3-sys`).
#   5. `radivine-control` never links `radivine-convert-image`, `radivine-webp`,
#      `libwebp-sys`, `jpeg-encoder`, or `rusqlite` (`libsqlite3-sys`).
#   6. `radivine-worker` is headless and never links `eframe`, `egui`, `winit`,
#      `x11rb`, `tiny-skia`, `resvg`, or `cosmic-text`.
# ==============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

check_forbidden_deps() {
    local pkg="$1"
    shift
    local tree_out
    tree_out="$(cargo tree -p "$pkg" -e normal --prefix none)"
    for forbidden in "$@"; do
        if echo "$tree_out" | grep -Eq "^${forbidden} v"; then
            echo "ERROR: Architecture boundary violation in '$pkg': forbidden runtime dependency '$forbidden' found!" >&2
            echo "$tree_out" | grep -E "^${forbidden} v" >&2
            exit 1
        fi
    done
    echo "  [OK] $pkg: forbidden dependencies absent ($*)"
}

echo "Verifying Radivine v0.3.0-rc.1 crate dependency boundaries..."

# 1. radivine-wheel: pure Rust, only smallvec allowed at runtime
WHEEL_DEPS="$(cargo tree -p radivine-wheel -e normal --prefix none | grep -v '^radivine-wheel v' || true)"
if [[ "$WHEEL_DEPS" != "smallvec v"* ]] || [[ "$(echo "$WHEEL_DEPS" | wc -l)" -ne 1 ]]; then
    echo "ERROR: radivine-wheel must depend ONLY on smallvec at runtime, found:" >&2
    echo "$WHEEL_DEPS" >&2
    exit 1
fi
echo "  [OK] radivine-wheel: depends solely on ($WHEEL_DEPS)"

# 2. radivine-convert: codec-free, SQLite-free, GUI/X11-free
check_forbidden_deps "radivine-convert" \
    "image" "png" "zune-jpeg" "image-webp" "jpeg-encoder" "libwebp-sys" \
    "radivine-webp" "radivine-convert-image" "rusqlite" "libsqlite3-sys" \
    "eframe" "egui" "winit" "x11rb"

# 3. radivine-jobs: wire/client crate only (no SQLite, no codecs, no GUI)
check_forbidden_deps "radivine-jobs" \
    "rusqlite" "libsqlite3-sys" "image" "libwebp-sys" "jpeg-encoder" \
    "radivine-webp" "radivine-convert-image" "eframe" "egui" "winit" "x11rb"

# 4. radivine (wheel daemon): never links codecs, image, or SQLite
check_forbidden_deps "radivine" \
    "radivine-convert-image" "radivine-webp" "libwebp-sys" "jpeg-encoder" \
    "image" "rusqlite" "libsqlite3-sys"

# 5. radivine-control (Control Center + Advanced Convert): never links conversion codecs or SQLite
check_forbidden_deps "radivine-control" \
    "radivine-convert-image" "radivine-webp" "libwebp-sys" "jpeg-encoder" \
    "rusqlite" "libsqlite3-sys"

# 6. radivine-worker: headless background worker, never links GUI/X11/rendering crates
check_forbidden_deps "radivine-worker" \
    "eframe" "egui" "winit" "x11rb" "tiny-skia" "resvg" "cosmic-text"

echo "All dependency-graph boundary checks PASSED."
