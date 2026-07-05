#!/bin/sh
# AMS installer: downloads the latest release binary for this platform and
# registers the agent workflow (via `ams init`: interactive checkbox pick when
# a terminal is available — Claude Code, Codex, Gemini, Copilot, Windsurf,
# Cline, Roo, Kilo, OpenCode, OpenClaw, Pi, Antigravity — else auto-detect).
#   curl -fsSL https://raw.githubusercontent.com/StMalina/ams/main/install.sh | sh
# Options via env:
#   AMS_INSTALL_DIR      target directory (default: ~/.local/bin)
#   AMS_VERSION          tag to install, e.g. v0.6.0 (default: latest)
#   AMS_CLAUDE_MD=0      skip `ams init` (register manually later)
#   AMS_AGENTS           non-interactive pick: e.g. claude,codex | all | auto
#   AMS_SKIP_CHECKSUM=1  install even when no .sha256 is published (at your own risk)
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
    # Resolve the latest tag via the /releases/latest redirect (no API rate limit),
    # falling back to the REST API.
    tag=$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
        "https://github.com/$REPO/releases/latest" 2>/dev/null \
        | sed 's|.*/tag/||') || tag=""
    case "$tag" in
        v*) ;;
        *) tag=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
            | grep '"tag_name"' | head -1 | cut -d'"' -f4) ;;
    esac
    [ -n "$tag" ] || { echo "error: could not resolve latest release tag" >&2; exit 1; }
fi

asset="ams-$tag-$arch-$os.tar.gz"
url="https://github.com/$REPO/releases/download/$tag/$asset"

echo "Installing ams $tag ($arch-$os) to $INSTALL_DIR"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

curl -fsSL "$url" -o "$tmp/$asset"

# Checksum: refuse to install unverified binaries unless explicitly bypassed.
if curl -fsSL "$url.sha256" -o "$tmp/$asset.sha256" 2>/dev/null; then
    expected=$(cut -d' ' -f1 <"$tmp/$asset.sha256")
    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$tmp/$asset" | cut -d' ' -f1)
    else
        actual=$(shasum -a 256 "$tmp/$asset" | cut -d' ' -f1)
    fi
    [ "$expected" = "$actual" ] || { echo "error: checksum mismatch for $asset" >&2; exit 1; }
    echo "checksum OK"
elif [ "${AMS_SKIP_CHECKSUM:-0}" = "1" ]; then
    echo "warning: no checksum published for $asset — installing unverified (AMS_SKIP_CHECKSUM=1)" >&2
else
    echo "error: no .sha256 published for $asset; re-run with AMS_SKIP_CHECKSUM=1 to bypass" >&2
    exit 1
fi

# Refuse archives with absolute paths or .. components.
if tar -tzf "$tmp/$asset" | grep -qE '^/|(^|/)\.\.(/|$)'; then
    echo "error: archive contains unsafe paths" >&2
    exit 1
fi

tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$INSTALL_DIR"
install -m 755 "$tmp/ams" "$INSTALL_DIR/ams"

"$INSTALL_DIR/ams" --version
case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) echo "note: $INSTALL_DIR is not in PATH — add: export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac

# --- register the workflow with the user's coding agents (default: on) ------
# `ams init` is idempotent (backup + atomic writes, undo: ams init --uninstall).
# Interactive checkbox pick over /dev/tty when a terminal is attached;
# otherwise it registers for the agents whose config dirs it detects.
if [ "${AMS_CLAUDE_MD:-1}" = "0" ]; then
    echo "registration skipped (AMS_CLAUDE_MD=0); run '$INSTALL_DIR/ams init' later to enable"
elif [ -n "${AMS_AGENTS:-}" ]; then
    "$INSTALL_DIR/ams" init --agents "$AMS_AGENTS" || echo "warning: 'ams init' failed — run it manually" >&2
else
    "$INSTALL_DIR/ams" init || echo "warning: 'ams init' failed — run it manually" >&2
fi

cat <<'EOF'

Done. Indexes build themselves on first use (or at session start with the
plugin), ams checks for updates once a day, and picking Claude Code above
also installed its plugin (guards + skill) when the claude CLI was found.
Agents without a global instructions file (Cursor, Hermes): copy
AGENTS.md.template from the repo into the project's AGENTS.md.
EOF
