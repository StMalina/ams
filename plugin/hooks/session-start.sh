#!/usr/bin/env bash
# AMS SessionStart hook: nudge the agent to use ams instead of raw Read/Grep
# for code navigation. Always exits 0 — never blocks session start.

set -u

if ! command -v ams >/dev/null 2>&1; then
    exit 0
fi

if [ -f "$PWD/.ams/index.db" ]; then
    cat <<'EOF'
ams index active in this project. Code-navigation workflow (mandatory, see the ams skill):
1. Orient: `ams tree <dir>` — one line per file (loc, api, used-by), instead of Glob + serial Reads.
2. Inspect: `ams describe <file>` BEFORE Read on any unfamiliar code file — signatures with @start-end spans, 10-40x cheaper than Read.
3. Read only the span: Read(offset=start, limit=end-start+1). Never a whole file when you have its spans.
Before changing an exported API: `ams refs <name>` (call sites) + `ams related <file>` (what breaks).
Symbol definitions: `ams find <name>`, fuzzy/by-meaning: `ams search <words>`. Grep stays for strings/comments/config only.
EOF
    exit 0
fi

if [ "${AMS_NO_AUTO_BUILD:-0}" = "1" ]; then
    exit 0
fi

if find "$PWD" -maxdepth 4 \
    \( -name '*.ts' -o -name '*.tsx' -o -name '*.js' -o -name '*.jsx' \
       -o -name '*.rs' -o -name '*.py' -o -name '*.go' -o -name '*.php' \
       -o -name '*.java' -o -name '*.kt' -o -name '*.cs' -o -name '*.rb' \) \
    -not -path '*/node_modules/*' -not -path '*/.git/*' -not -path '*/target/*' \
    -print -quit 2>/dev/null | grep -q .; then
    # Code project without an index: build it in the background so the first
    # ams query (or guard) already has data. Never blocks session start.
    (cd "$PWD" && nohup ams build >/dev/null 2>&1 &)
    echo "ams: building the code index in the background (.ams/index.db). Use ams describe/find/tree/refs for navigation once ready — queries self-heal."
fi

exit 0
