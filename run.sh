#!/usr/bin/env bash
# run.sh — launch Finny, the native Linux desktop GUI (eframe/egui).
# Single binary. No HTTP server, no browser. Self-locating so it works from any cwd.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# Keep data inside the project folder by default (portable, survives restart).
# NOTE: this is the canonical DB location — do not point elsewhere or you lose history.
export FINNY_DATA_DIR="${FINNY_DATA_DIR:-$SCRIPT_DIR/data}"
mkdir -p "$FINNY_DATA_DIR"

# On a normal Ubuntu/Linux desktop the system GL + X libs are already on the
# loader path, so nothing below is needed. In a headless/software-GL sandbox we
# fall back to llvmpipe and a locally-fetched libxkbcommon-x11. Both are additive
# and harmless on a real desktop (missing paths are simply ignored by the loader).
if [ -z "${DISPLAY:-}" ] && [ -z "${WAYLAND_DISPLAY:-}" ]; then
    echo "[finny] No DISPLAY/WAYLAND detected — this is a GUI app; start it from a desktop session." >&2
fi
: "${LIBGL_ALWAYS_SOFTWARE:=}"    # leave GPU accel on by default on real desktops
if [ -d /opt/data/home/userlibs ]; then
    export LD_LIBRARY_PATH="/opt/data/home/userlibs:/usr/lib/x86_64-linux-gnu:${LD_LIBRARY_PATH:-}"
fi

# Prefer a prebuilt release binary; build once if absent.
if [ ! -x ./target/release/finny ]; then
    if command -v cargo >/dev/null 2>&1; then
        echo "[run.sh] Building release binary (first run, ~2 min)…"
        cargo build --release
    else
        echo "[run.sh] ERROR: cargo not found and no prebuilt binary." >&2
        exit 127
    fi
fi

echo "[finny] Launching native window (data: $FINNY_DATA_DIR)"
exec ./target/release/finny "$@"
