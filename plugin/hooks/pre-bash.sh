#!/usr/bin/env bash
# AMS PreToolUse hook for Bash: agents often bypass the Read/Grep tools and
# search with `grep -rn foo` / `rg foo` in the shell — the Read and Grep
# guards never see that. When such a command greps for a bare identifier
# inside an indexed project, intercept once and answer with `ams find`
# output. Repeating the exact same command passes (per-session marker).
#
# Conservative by design: only plain grep/rg invocations whose pattern looks
# like a symbol (camelCase or snake_case identifier) trigger; strings, logs,
# config greps, pipelines from other output — untouched. Fails open (exit 0)
# on any doubt: this hook must never break a shell command.

set -u

[ "${AMS_NO_BASH_GUARD:-0}" = "1" ] && exit 0
command -v ams >/dev/null 2>&1 || exit 0
command -v python3 >/dev/null 2>&1 || exit 0

input=$(cat) || exit 0

eval "$(printf '%s' "$input" | python3 -c '
import json, re, shlex, sys

try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(0)
cmd = str((d.get("tool_input") or {}).get("command") or "")
if not cmd:
    sys.exit(0)

# Read-only ams queries carry no side effects; auto-approve them so plan mode
# (which otherwise denies every Bash call) never blocks the ams workflow.
AMS_READONLY = {"describe", "tree", "find", "refs", "search", "related", "gain"}

def unwrap(tokens):
    """Strip env-var prefixes and command wrappers (rtk/sudo/env/...)."""
    WRAPPERS = {"rtk", "command", "sudo", "env", "nice", "nohup"}
    while tokens:
        while tokens and re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", tokens[0]):
            tokens.pop(0)
        if tokens and tokens[0].rsplit("/", 1)[-1] in WRAPPERS:
            w = tokens.pop(0).rsplit("/", 1)[-1]
            if w == "rtk" and tokens and tokens[0] == "proxy":
                tokens.pop(0)
            continue
        break
    return tokens

# Only a lone ams query (no operators chaining a second command) is allowed —
# `ams find x && rm -rf y` must not slip through.
segments = [s for s in re.split(r"&&|\|\||[;|]", cmd) if s.strip()]
if len(segments) == 1:
    try:
        toks = unwrap(shlex.split(segments[0].strip()))
    except ValueError:
        toks = []
    if len(toks) >= 2 and toks[0].rsplit("/", 1)[-1] == "ams" and toks[1] in AMS_READONLY:
        print("ALLOW=1")
        sys.exit(0)

if cmd.startswith("ams "):
    sys.exit(0)

IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]{2,}\Z")
# grep options that consume the next argument (pattern must not be mistaken).
GREP_OPTARG = {"-e", "-f", "-m", "-A", "-B", "-C", "-d", "-D", "--include", "--exclude", "--exclude-dir"}
RG_OPTARG = {"-e", "-f", "-m", "-A", "-B", "-C", "-g", "-t", "-T", "--type", "--glob", "--max-count"}

def pattern_of(tokens):
    prog = tokens[0].rsplit("/", 1)[-1]
    if prog not in ("grep", "egrep", "fgrep", "rg"):
        return None
    optarg = RG_OPTARG if prog == "rg" else GREP_OPTARG
    explicit = None
    positional = []
    i = 1
    while i < len(tokens):
        t = tokens[i]
        if t == "--":
            positional.extend(tokens[i + 1 :])
            break
        if t.startswith("-") and len(t) > 1:
            if t in optarg and i + 1 < len(tokens):
                if t == "-e":
                    explicit = tokens[i + 1]
                i += 2
                continue
            if t.startswith("--") and "=" in t:
                if t.startswith("--regexp="):
                    explicit = t.split("=", 1)[1]
                i += 1
                continue
            i += 1
            continue
        positional.append(t)
        i += 1
    return explicit if explicit is not None else (positional[0] if positional else None)

# Split on top-level-ish operators; shlex below rejects anything the naive
# split mangled (unbalanced quotes) -> fail open.
pat = None
for seg in re.split(r"&&|\|\||[;|]", cmd):
    try:
        tokens = shlex.split(seg.strip())
    except ValueError:
        sys.exit(0)
    if not tokens:
        continue
    # Unwrap env-var prefixes and command wrappers: `FOO=1 rtk grep X`,
    # `sudo grep X`, `rtk proxy rg X` must still hit the guard.
    tokens = unwrap(tokens)
    if not tokens or tokens[0] == "ams":
        continue
    p = pattern_of(tokens)
    if p and IDENT.fullmatch(p) and (any(c.isupper() for c in p) or "_" in p):
        pat = p
        break
if not pat:
    sys.exit(0)

print("PAT=%s" % shlex.quote(pat))
print("CWD=%s" % shlex.quote(str(d.get("cwd") or "")))
print("SID=%s" % shlex.quote(str(d.get("session_id") or "nosession")))
' 2>/dev/null)" 2>/dev/null || exit 0

# Auto-approve a read-only ams query (unblocks plan mode).
if [ "${ALLOW:-}" = "1" ]; then
    printf '%s\n' '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","permissionDecisionReason":"ams read-only query — no side effects"}}'
    exit 0
fi

[ -n "${PAT:-}" ] || exit 0
[ -n "${CWD:-}" ] || exit 0
[ -d "$CWD" ] || exit 0

# Nearest .ams/index.db walking up from the working directory.
root="$CWD"
while :; do
    [ -f "$root/.ams/index.db" ] && break
    [ "$root" = "/" ] && exit 0
    root=$(dirname "$root")
done

# Block only once per pattern per session (shared marker with the Grep guard:
# one hint per symbol regardless of which tool asked).
marker="${TMPDIR:-/tmp}/.ams-grep-guard-${SID}-$(printf '%s' "$PAT" | cksum | tr ' \t' '--')"
[ -e "$marker" ] && exit 0

out=$(cd "$root" && ams find "$PAT" 2>/dev/null) || exit 0
case "$out" in
    ''|no\ symbols*|no\ matches*)
        # Feedback signal A: ams has no symbol for an identifier-shaped grep. If
        # the token still exists in the code text, this is a confirmed coverage
        # miss (symbol present, ams didn't index it) — record it once per token
        # per session, then let the grep run.
        mmark="${TMPDIR:-/tmp}/.ams-miss-${SID}-$(printf '%s' "$PAT" | cksum | tr ' \t' '--')"
        if [ ! -e "$mmark" ]; then
            if (cd "$root" && { command -v rg >/dev/null 2>&1 && rg -qwF -- "$PAT" \
                    || grep -rqwF -- "$PAT" .; }) 2>/dev/null; then
                (cd "$root" && ams miss --record "$PAT" >/dev/null 2>&1)
                touch "$mmark" 2>/dev/null
            fi
        fi
        exit 0
        ;;
esac

touch "$marker" 2>/dev/null

{
    echo "grep intercepted (once per pattern): \`$PAT\` is an indexed symbol. Definitions — output of \`ams find $PAT\`:"
    echo
    echo "$out"
    echo
    echo "Call sites: \`ams refs $PAT\`. If you really wanted a text search, repeat the exact same command — it will pass now."
} >&2
exit 2
