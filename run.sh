#!/usr/bin/env bash
# run.sh — launch Finny, the native Linux desktop GUI (eframe/egui).
# Single binary, no HTTP server, no browser. Self-locating so it works from any cwd.
#
# On first run this script RESOLVES DEPENDENCIES automatically:
#   * the Rust toolchain (via rustup, if `cargo` is missing)
#   * a C compiler + pkg-config (needed to build bundled SQLite / ring)
#   * runtime graphics libs for the native window (OpenGL + X11/Wayland + xkbcommon)
# It detects the package manager (apt / dnf / pacman) and ONLY installs packages
# that are actually missing. It ALWAYS asks before running a privileged (sudo)
# install, and it NEVER touches your data (./data/finny.db).
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# Keep data inside the project folder by default (portable, survives restart).
# NOTE: this is the canonical DB location — do not point elsewhere or you lose history.
export FINNY_DATA_DIR="${FINNY_DATA_DIR:-$SCRIPT_DIR/data}"
mkdir -p "$FINNY_DATA_DIR"

log()  { printf '\033[1;36m[finny]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[finny]\033[0m %s\n' "$*" >&2; }
err()  { printf '\033[1;31m[finny]\033[0m %s\n' "$*" >&2; }

# Ask a yes/no question on the controlling terminal. Default = yes.
# Returns 0 for yes, 1 for no. Non-interactive (no tty) => refuse (safe default).
confirm() {
    local prompt="$1"
    if [ -r /dev/tty ]; then
        printf '\033[1;33m[finny]\033[0m %s [Y/n] ' "$prompt" >/dev/tty
        local ans
        read -r ans </dev/tty || ans=""
        case "$ans" in [Nn]*) return 1 ;; *) return 0 ;; esac
    else
        warn "non-interactive shell: cannot prompt; skipping '$prompt' (set FINNY_ASSUME_YES=1 to allow)."
        [ "${FINNY_ASSUME_YES:-0}" = "1" ]
    fi
}

# Run a command as root if not already root, using sudo (prompting for pw).
as_root() {
    if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo "$@"; fi
}

# Detect the system package manager (empty string if unknown).
detect_pkg_mgr() {
    if   command -v apt-get >/dev/null 2>&1; then echo apt
    elif command -v dnf     >/dev/null 2>&1; then echo dnf
    elif command -v pacman  >/dev/null 2>&1; then echo pacman
    else echo ""; fi
}

# Install packages via the detected manager (caller already confirmed).
pkg_install() {
    local mgr; mgr="$(detect_pkg_mgr)"
    [ -z "$mgr" ] && { warn "no known package manager found; please install manually: $*"; return 1; }
    log "installing via $mgr: $*"
    case "$mgr" in
        apt)    as_root apt-get update -y >/dev/null 2>&1 || true
                as_root apt-get install -y --no-install-recommends "$@" ;;
        dnf)    as_root dnf install -y "$@" ;;
        pacman) as_root pacman -Sy --noconfirm "$@" ;;
    esac
}

# Is a shared library (soname substring) available to the dynamic linker?
# NOTE: this is deliberately NOT `ldconfig -p | grep -q` — under `set -o pipefail`
# that pipeline returns a *false negative* when grep -q closes the pipe early and
# ldconfig is killed by SIGPIPE, which would make us wrongly "miss" present libs
# and prompt to reinstall them. Instead we snapshot ldconfig once and grep a
# here-string (no pipe, no SIGPIPE, pipefail-safe).
_LD_LOADED=0
_LD_CACHE=""
have_lib() {
    if [ "$_LD_LOADED" = "0" ]; then
        _LD_CACHE="$(ldconfig -p 2>/dev/null || true)"
        _LD_LOADED=1
    fi
    grep -q -- "$1" <<<"$_LD_CACHE"
}

ensure_rust() {
    if command -v cargo >/dev/null 2>&1 && command -v rustc >/dev/null 2>&1; then
        return 0
    fi
    # Maybe rustup is installed but not on PATH.
    for c in "$HOME/.cargo/bin" /opt/data/home/.cargo/bin; do
        [ -x "$c/cargo" ] && { export PATH="$c:$PATH"; export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"; export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"; return 0; }
    done
    warn "Rust toolchain (cargo/rustc) not found — Finny is a Rust app and needs it to build."
    if confirm "Install Rust via rustup (https://rustup.rs)?"; then
        log "installing rustup (stable toolchain)…"
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal
        # shellcheck disable=SC1091
        . "$HOME/.cargo/env" 2>/dev/null || export PATH="$HOME/.cargo/bin:$PATH"
    else
        err "Rust is required. Install it (e.g. 'curl --proto =https --tlsv1.2 -sSf https://sh.rustup.rs | sh') and re-run."
        exit 127
    fi
    command -v cargo >/dev/null 2>&1 || { err "cargo still not on PATH after install."; exit 127; }
}

ensure_build_deps() {
    local need=()
    command -v cc  >/dev/null 2>&1 || command -v gcc >/dev/null 2>&1 || need+=("cc")
    command -v pkg-config >/dev/null 2>&1 || need+=("pkg-config")
    [ ${#need[@]} -eq 0 ] && return 0
    local mgr; mgr="$(detect_pkg_mgr)"
    local pkgs=()
    case "$mgr" in
        apt)    pkgs=(build-essential pkg-config) ;;
        dnf)    pkgs=(gcc pkg-config) ;;
        pacman) pkgs=(base-devel pkg-config) ;;
        *)      warn "need a C compiler + pkg-config to build; install them manually."; return 0 ;;
    esac
    if confirm "Build dependencies missing (${need[*]}). Install ${pkgs[*]}?"; then
        pkg_install "${pkgs[@]}" || warn "could not install build deps; the build may fail."
    fi
}

ensure_runtime_libs() {
    # soname -> human purpose. We check the runtime .so.N (not the -dev symlink).
    declare -A LIBS=(
        ["libGL.so.1"]="OpenGL (mesa)"
        ["libxkbcommon.so.0"]="libxkbcommon"
        ["libxkbcommon-x11.so.0"]="libxkbcommon-x11"
        ["libX11.so.6"]="libX11"
        ["libxcb.so.1"]="libxcb"
        ["libXcursor.so.1"]="libXcursor"
        ["libXi.so.6"]="libXi"
        ["libXrandr.so.2"]="libXrandr"
        ["libXinerama.so.1"]="libXinerama"
        ["libXrender.so.1"]="libXrender"
    )
    local missing_names=()
    for soname in "${!LIBS[@]}"; do
        have_lib "$soname" || missing_names+=("$soname")
    done
    # Wayland client libs are usually present; only flag if a Wayland session and missing.
    if [ -n "${WAYLAND_DISPLAY:-}" ] && ! have_lib "libwayland-client.so.0"; then
        missing_names+=("libwayland-client.so.0")
    fi
    [ ${#missing_names[@]} -eq 0 ] && return 0

    local mgr; mgr="$(detect_pkg_mgr)"
    local pkgs=()
    case "$mgr" in
        apt)    pkgs=(libgl1 libxkbcommon0 libxkbcommon-x11-0 libx11-6 libxcb1 libxcursor1 libxi6 libxrandr2 libxinerama1 libxrender1 libwayland-client0) ;;
        dnf)    pkgs=(mesa-libGL libxkbcommon libxkbcommon-x11 libX11 libxcb libXcursor libXi libXrandr libXinerama libXrender libwayland-client) ;;
        pacman) pkgs=(mesa libxkbcommon libx11 libxcb libxcursor libxi libxrandr libxinerama libxrender wayland) ;;
        *)      warn "missing runtime libs: ${missing_names[*]}; install the GL + X11 + xkbcommon packages for your distro."; return 0 ;;
    esac
    warn "Missing runtime graphics libraries: ${missing_names[*]}"
    if confirm "Install the packages needed to show the native window (${pkgs[*]})?"; then
        pkg_install "${pkgs[@]}" || warn "could not install runtime libs; the window may fail to open."
    fi
}

# ---- resolve everything, then build & launch ----
ensure_rust
ensure_build_deps
ensure_runtime_libs

# Headless / software-GL fallback (harmless on a real desktop).
if [ -z "${DISPLAY:-}" ] && [ -z "${WAYLAND_DISPLAY:-}" ]; then
    warn "No DISPLAY/WAYLAND detected — this is a GUI app; start it from a desktop session."
fi
if [ -d /opt/data/home/userlibs ]; then
    export LD_LIBRARY_PATH="/opt/data/home/userlibs:/usr/lib/x86_64-linux-gnu:${LD_LIBRARY_PATH:-}"
fi

# Prefer a prebuilt release binary; build once if absent.
if [ ! -x ./target/release/finny ]; then
    log "Building release binary (first run, ~1–2 min)…"
    cargo build --release
fi

log "Launching native window (data: $FINNY_DATA_DIR)"
exec ./target/release/finny "$@"
