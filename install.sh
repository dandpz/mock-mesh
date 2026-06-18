#!/bin/sh
# mock-mesh installer: downloads the latest `mock-mesh` release binary for this
# platform from GitHub, verifies its checksum, and installs it.
#
#   curl -fsSL https://raw.githubusercontent.com/dandpz/mock-mesh/main/install.sh | sh
#
# Options (environment variables):
#   MOCKMESH_INSTALL_DIR  install directory (default: ~/.local/bin)
#   MOCKMESH_VERSION      version to install, e.g. "0.2.0" (default: latest release)
set -eu

REPO="dandpz/mock-mesh"
INSTALL_DIR="${MOCKMESH_INSTALL_DIR:-$HOME/.local/bin}"

err() {
    echo "error: $*" >&2
    echo "Prebuilt archives: https://github.com/$REPO/releases" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || err "required command not found: $1"
}

need curl
need tar

os=$(uname -s)
arch=$(uname -m)
case "$os" in
    Linux)
        case "$arch" in
            x86_64 | amd64) target="x86_64-unknown-linux-musl" ;;
            aarch64 | arm64) target="aarch64-unknown-linux-gnu" ;;
            *) err "unsupported architecture: $arch" ;;
        esac
        ;;
    Darwin)
        case "$arch" in
            x86_64) target="x86_64-apple-darwin" ;;
            arm64) target="aarch64-apple-darwin" ;;
            *) err "unsupported architecture: $arch" ;;
        esac
        ;;
    *)
        err "unsupported OS: $os (on Windows, download the .zip from the releases page)"
        ;;
esac

if [ -n "${MOCKMESH_VERSION:-}" ]; then
    version="${MOCKMESH_VERSION#v}"
else
    version=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" |
        grep '"tag_name"' | head -n 1 | sed -E 's/.*"v?([^"]+)".*/\1/')
    [ -n "$version" ] || err "could not determine latest version"
fi

name="mock-mesh-${version}-${target}"
base_url="https://github.com/$REPO/releases/download/v${version}"

tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT

echo "Downloading mock-mesh v${version} (${target})..."
curl -fsSL -o "$tmpdir/${name}.tar.gz" "$base_url/${name}.tar.gz" ||
    err "download failed: $base_url/${name}.tar.gz"
curl -fsSL -o "$tmpdir/SHA256SUMS" "$base_url/SHA256SUMS" ||
    err "download failed: $base_url/SHA256SUMS"

(
    cd "$tmpdir"
    grep " ${name}.tar.gz\$" SHA256SUMS >checksum ||
        err "no checksum for ${name}.tar.gz in SHA256SUMS"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum -c checksum >/dev/null
    else
        shasum -a 256 -c checksum >/dev/null
    fi
) || err "checksum verification failed"
echo "Checksum verified."

tar xzf "$tmpdir/${name}.tar.gz" -C "$tmpdir"
mkdir -p "$INSTALL_DIR"
install -m 755 "$tmpdir/$name/mock-mesh" "$INSTALL_DIR/mock-mesh"

echo "Installed mock-mesh v${version} to $INSTALL_DIR/mock-mesh"
case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
        echo
        echo "Note: $INSTALL_DIR is not on your PATH. Add it with:"
        echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
        ;;
esac
