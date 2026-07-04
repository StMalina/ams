#!/bin/sh
# AMS installer: downloads the latest release binary for this platform.
#   curl -fsSL https://raw.githubusercontent.com/StMalina/ams/main/install.sh | sh
# Options via env:
#   AMS_INSTALL_DIR  target directory (default: ~/.local/bin)
#   AMS_VERSION      tag to install, e.g. v0.3.0 (default: latest)
set -eu

REPO="StMalina/ams"
INSTALL_DIR="${AMS_INSTALL_DIR:-$HOME/.local/bin}"

case "$(uname -s)" in
    Linux)  os="unknown-linux-musl" ;;
    Darwin) os="apple-darwin" ;;
    *) echo "error: unsupported OS $(uname -s); on Windows download the zip from https://github.com/$REPO/releases" >&2; exit 1 ;;
esac

case "$(uname -m)" in
    x86_64|amd64)  arch="x86_64" ;;
    aarch64|arm64) arch="aarch64" ;;
    *) echo "error: unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac

if [ -n "${AMS_VERSION:-}" ]; then
    tag="$AMS_VERSION"
else
    tag=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
        | grep '"tag_name"' | head -1 | cut -d'"' -f4)
    [ -n "$tag" ] || { echo "error: could not resolve latest release tag" >&2; exit 1; }
fi

asset="ams-$tag-$arch-$os.tar.gz"
url="https://github.com/$REPO/releases/download/$tag/$asset"

echo "Installing ams $tag ($arch-$os) to $INSTALL_DIR"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

curl -fsSL "$url" -o "$tmp/$asset"
tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$INSTALL_DIR"
install -m 755 "$tmp/ams" "$INSTALL_DIR/ams"

"$INSTALL_DIR/ams" --version
case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) echo "note: $INSTALL_DIR is not in PATH — add: export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac
