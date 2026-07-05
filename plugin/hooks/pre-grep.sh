#!/usr/bin/env bash
# AMS PreToolUse hook for Grep: when the agent greps for a bare identifier
# (a symbol name, not a string/log/config value) inside an indexed project,
# intercept once and answer with `ams find` output — exact definitions with
# @start-end spans instead of a pile of matching lines. Repeating the same
# Grep passes (per-session marker), so text searches are never blocked twice.
#
# Mechanics: stdout at exit 0 is NOT shown to the model; the only advisory
# channel is blocking (exit 2 + stderr). Every guard fails open (exit 0).

set -u

[ "${AMS_NO_GREP_GUARD:-0}" = "1" ] && exit 0
command -v ams >/dev/null 2>&1 || exit 0
command -v python3 >/dev/null 2>&1 || exit 0

input=$(cat) || exit 0

eval "$(printf '%s' "$input" | python3 -c '
import json, re, shlex, sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(0)
ti = d.get("tool_input") or {}
pat = str(ti.get("pattern") or "")
# Bare identifier with a hint of being a symbol (camelCase or snake_case);
# plain lowercase words ("error", "config") are usually text searches — skip.
if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{2,}", pat):
    sys.exit(0)
if not (any(c.isupper() for c in pat) or "_" in pat):
    sys.exit(0)
print("PAT=%s" % shlex.quote(pat))
print("DIR=%s" % shlex.quote(str(ti.get("path") or d.get("cwd") or "")))
print("SID=%s" % shlex.quote(str(d.get("session_id") or "nosession")))
' 2>/dev/null)" 2>/dev/null || exit 0

[ -n "${PAT:-}" ] || exit 0
[ -n "${DIR:-}" ] || exit 0
[ -d "$DIR" ] || DIR=$(dirname "$DIR")
[ -d "$DIR" ] || exit 0

# Nearest .ams/index.db walking up from the search dir.
root="$DIR"
while :; do
    [ -f "$root/.ams/index.db" ] && break
    [ "$root" = "/" ] && exit 0
    root=$(dirname "$root")
done

# Block only once per pattern per session.
marker="${TMPDIR:-/tmp}/.ams-grep-guard-${SID}-$(printf '%s' "$PAT" | cksum | tr ' \t' '--')"
[ -e "$marker" ] && exit 0

out=$(cd "$root" && ams find "$PAT" 2>/dev/null) || exit 0
# No indexed definitions -> probably a genuine text search; stay silent.
case "$out" in
    ''|no\ symbols*|no\ matches*) exit 0 ;;
esac

touch "$marker" 2>/dev/null

{
    echo "Grep intercepted (once per pattern): \`$PAT\` is an indexed symbol. Definitions — output of \`ams find $PAT\`:"
    echo
    echo "$out"
    echo
    echo "Call sites: \`ams refs $PAT\`. Read a definition with Read(offset=start, limit=end-start+1) from a span above. If you really wanted a text search, repeat the exact same Grep — it will pass now."
} >&2
exit 2
