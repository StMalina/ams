#!/usr/bin/env bash
# AMS PreToolUse hook for Read: the first Read of a large, indexed code file
# is intercepted once and answered with `ams describe` output (signatures +
# exact @start-end spans) instead of the full file. The agent then re-issues
# a targeted Read(offset, limit) — or repeats the same Read to get the whole
# file, which passes because a per-session marker is left behind.
#
# Mechanics: plain stdout at exit 0 is NOT shown to the model for PreToolUse;
# the only advisory channel is blocking (exit 2 + stderr). Hence block-once.
# Every guard below fails open (exit 0) — this hook must never break Read.

set -u

[ "${AMS_NO_READ_GUARD:-0}" = "1" ] && exit 0
command -v ams >/dev/null 2>&1 || exit 0
command -v python3 >/dev/null 2>&1 || exit 0

input=$(cat) || exit 0

eval "$(printf '%s' "$input" | python3 -c '
import json, shlex, sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(0)
ti = d.get("tool_input") or {}
print("FILE=%s" % shlex.quote(str(ti.get("file_path") or "")))
spanned = ti.get("offset") is not None or ti.get("limit") is not None
print("SPANNED=%s" % ("1" if spanned else "0"))
print("SID=%s" % shlex.quote(str(d.get("session_id") or "nosession")))
' 2>/dev/null)" 2>/dev/null || exit 0

[ -n "${FILE:-}" ] || exit 0
# A Read that already targets a span is exactly the behavior we want.
[ "${SPANNED:-1}" = "0" ] || exit 0

case "$FILE" in
    /*) ;;
    *) exit 0 ;;
esac
case "$FILE" in
    *.ts|*.tsx|*.js|*.jsx|*.mjs|*.cjs|*.rs|*.py|*.go|*.php|*.java|*.kt|*.kts|*.cs|*.rb) ;;
    *) exit 0 ;;
esac

[ -f "$FILE" ] || exit 0

# Small files: describe does not pay for the extra round-trip.
lines=$(wc -l <"$FILE" 2>/dev/null) || exit 0
[ "${lines:-0}" -ge 150 ] || exit 0

# Nearest .ams/index.db walking up from the file.
root=$(dirname "$FILE")
while :; do
    [ -f "$root/.ams/index.db" ] && break
    [ "$root" = "/" ] && exit 0
    root=$(dirname "$root")
done

# Block only the first Read of this file in this session.
marker="${TMPDIR:-/tmp}/.ams-read-guard-${SID}-$(printf '%s' "$FILE" | cksum | tr ' \t' '--')"
[ -e "$marker" ] && exit 0

rel=${FILE#"$root"/}
out=$(cd "$root" && ams describe "$rel" 2>/dev/null) || exit 0
[ -n "$out" ] || exit 0

touch "$marker" 2>/dev/null

{
    echo "Read intercepted (once per file): this file is indexed by ams ($lines lines). Signatures with exact spans — output of \`ams describe $rel\`:"
    echo
    echo "$out"
    echo
    echo "Read only the span you need: Read(file_path=\"$FILE\", offset=<start>, limit=<end - start + 1>) using an @start-end span above. If you really need the whole file, repeat the exact same Read — it will pass now."
} >&2
exit 2
