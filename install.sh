#!/bin/sh
# AMS installer: downloads the latest release binary for this platform.
#   curl -fsSL https://raw.githubusercontent.com/StMalina/ams/main/install.sh | sh
# Options via env:
#   AMS_INSTALL_DIR  target directory (default: ~/.local/bin)
#   AMS_VERSION      tag to install, e.g. v0.3.0 (default: latest)
#   AMS_CLAUDE_MD=1  append the agent workflow snippet to ~/.claude/CLAUDE.md
#                    (idempotent: guarded by <!-- ams --> markers)
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

# --- optional: register the workflow in the user's global CLAUDE.md ---------
# `curl | sh` has no tty, so no interactive prompt: opt in via AMS_CLAUDE_MD=1.
if [ "${AMS_CLAUDE_MD:-0}" = "1" ]; then
    claude_md="$HOME/.claude/CLAUDE.md"
    mkdir -p "$HOME/.claude"
    if [ -f "$claude_md" ] && grep -q '<!-- ams:start -->' "$claude_md"; then
        echo "CLAUDE.md: ams section already present — skipped"
    else
        cat >>"$claude_md" <<'EOF'

<!-- ams:start -->
## Code navigation (projects with .ams/index.db)
Before Read on an unfamiliar code file: `ams describe <file>` — signatures
with @start-end spans, 10-40x cheaper; then Read only the span.
Symbol definition: `ams find <name>`. Directory: `ams tree <dir>`.
Before changing an exported API: `ams refs <name>` + `ams related <file>`.
Grep only for strings/comments/config. No index yet -> `ams build` once.
<!-- ams:end -->
EOF
        echo "CLAUDE.md: ams workflow section appended to $claude_md"
    fi
fi

cat <<'EOF'

Next steps:
  1. Index a project:        cd <project> && ams build
  2. Claude Code plugin (skill + hooks that make agents actually use ams):
       /plugin marketplace add StMalina/ams
       /plugin install ams@ams
  3. Optional, standing instructions for every project — either re-run with
     AMS_CLAUDE_MD=1, or paste the snippet from the README into
     ~/.claude/CLAUDE.md. Other agents (Codex, ...): see AGENTS.md.template.
EOF
