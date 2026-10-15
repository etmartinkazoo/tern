#!/bin/sh
# Install tern on Debian/Ubuntu or Fedora, with everything it needs.
#
#   curl -fsSL https://raw.githubusercontent.com/etmartinkazoo/tern/main/install.sh | sh
#
# Environment:
#   TERN_REF=<branch|tag>   build this ref instead of main
#   TERN_NO_EXTRAS=1        skip notify-send, wl-copy and xdg-open (tern runs without them)
#
# The whole script is inside main() so a partial download can't run half of it.

set -eu

REPO="https://github.com/etmartinkazoo/tern"
MIN_RUST="1.89"

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# version_ge 1.90.0 1.89 -> true
version_ge() {
    [ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -n1)" = "$2" ]
}

sudo_cmd() {
    if [ "$(id -u)" -eq 0 ]; then
        "$@"
    elif have sudo; then
        sudo "$@"
    else
        die "need root to install packages, and sudo isn't available. Run as root or install sudo."
    fi
}

detect_distro() {
    [ -r /etc/os-release ] || die "can't read /etc/os-release to tell which distribution this is."
    # shellcheck disable=SC1091
    . /etc/os-release
    for id in ${ID:-} ${ID_LIKE:-}; do
        case "$id" in
            debian | ubuntu) echo debian; return ;;
            fedora | rhel | centos) echo fedora; return ;;
        esac
    done
    die "unsupported distribution '${ID:-unknown}'. tern's installer handles Debian, Ubuntu and Fedora.
Elsewhere, install Rust ($MIN_RUST+) and a C compiler, then run:
    cargo install --locked --git $REPO"
}

install_packages() {
    distro=$1
    case "$distro" in
        debian)
            pkgs="build-essential pkg-config git curl ca-certificates"
            [ -n "${TERN_NO_EXTRAS:-}" ] || pkgs="$pkgs libnotify-bin wl-clipboard xdg-utils"
            say "Installing packages: $pkgs"
            sudo_cmd env DEBIAN_FRONTEND=noninteractive apt-get update -q
            # shellcheck disable=SC2086
            sudo_cmd env DEBIAN_FRONTEND=noninteractive apt-get install -y -q $pkgs
            ;;
        fedora)
            pkgs="gcc make pkgconf-pkg-config git curl ca-certificates"
            [ -n "${TERN_NO_EXTRAS:-}" ] || pkgs="$pkgs libnotify wl-clipboard xdg-utils"
            say "Installing packages: $pkgs"
            # shellcheck disable=SC2086
            sudo_cmd dnf install -y $pkgs
            ;;
    esac
}

# Distro Rust is often too old (Ubuntu 24.04 ships 1.75), so use rustup unless
# the cargo on PATH is new enough.
ensure_rust() {
    # shellcheck disable=SC1091
    [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

    if have cargo && have rustc; then
        current=$(rustc --version | awk '{print $2}')
        if version_ge "$current" "$MIN_RUST"; then
            say "Using Rust $current"
            return
        fi
        if have rustup; then
            say "Rust $current is older than $MIN_RUST, updating with rustup"
            rustup update stable
            rustup default stable
            return
        fi
        warn "Rust $current is older than $MIN_RUST; installing a newer one with rustup in ~/.cargo"
    else
        say "Installing Rust with rustup in ~/.cargo"
    fi

    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
        sh -s -- -y --profile minimal --default-toolchain stable --no-modify-path
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
}

main() {
    [ "$(uname -s)" = Linux ] || die "this installer is for Linux."

    # ensure_rust puts ~/.cargo/bin on our PATH, so remember the user's.
    user_path=$PATH

    distro=$(detect_distro)
    install_packages "$distro"
    ensure_rust

    ref=${TERN_REF:-main}
    say "Building tern from $REPO ($ref). This takes a few minutes."
    if git ls-remote --exit-code --tags "$REPO" "refs/tags/$ref" >/dev/null 2>&1; then
        kind=--tag
    else
        kind=--branch
    fi
    cargo install --locked --force --git "$REPO" "$kind" "$ref" tern

    bin="${CARGO_HOME:-$HOME/.cargo}/bin/tern"
    [ -x "$bin" ] || die "the build finished but $bin isn't there."

    say "Installed $("$bin" --version 2>/dev/null || echo tern) to $bin"

    on_path() { case ":$user_path:" in *":$1:"*) return 0 ;; esac; return 1; }
    bindir=$(dirname "$bin")
    if ! on_path "$bindir" && ! on_path "${bindir%/}/"; then
        if on_path "$HOME/.local/bin"; then
            mkdir -p "$HOME/.local/bin"
            ln -sf "$bin" "$HOME/.local/bin/tern"
            say "Linked $HOME/.local/bin/tern -> $bin"
        else
            warn "$bindir isn't on your PATH. Add this to your shell's startup file (~/.bashrc, ~/.zshrc):"
            echo "    export PATH=\"$bindir:\$PATH\""
        fi
    fi

    cat <<EOF

Next:
    tern --demo     look around with a fake mailbox
    tern --init     write ~/.config/tern/config.toml
    tern doctor     check every account and server
EOF
}

main "$@"
