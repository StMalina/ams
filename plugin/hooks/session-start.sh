#!/usr/bin/env bash
# AMS SessionStart hook: nudge the agent to use ams instead of raw Read/Grep
# for code navigation. Always exits 0 — never blocks session start.

set -u

if ! command -v ams >/dev/null 2>&1; then
    exit 0
fi

if [ -f "$PWD/.ams/index.db" ]; then
    echo "ams index found in this project — prefer 'ams describe/find/refs/tree' over Read/Grep for code navigation (see the ams skill)."
    exit 0
fi

if find "$PWD" -maxdepth 4 \
    \( -name '*.ts' -o -name '*.tsx' -o -name '*.js' -o -name '*.jsx' \
       -o -name '*.rs' -o -name '*.py' -o -name '*.go' -o -name '*.php' \) \
    -not -path '*/node_modules/*' -not -path '*/.git/*' -not -path '*/target/*' \
    -print -quit 2>/dev/null | grep -q .; then
    echo "ams is installed but this project has no .ams/index.db yet — run 'ams build' to enable fast code navigation."
fi

exit 0
