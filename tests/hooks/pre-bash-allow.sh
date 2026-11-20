#!/usr/bin/env bash
# Self-check for the read-only `ams` auto-approve in plugin/hooks/pre-bash.sh.
# A stub `ams` on PATH is enough: the allow decision never runs the binary.
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
hook="$root/plugin/hooks/pre-bash.sh"
stub=$(mktemp -d)
trap 'rm -rf "$stub"' EXIT
printf '#!/bin/sh\nexit 1\n' >"$stub/ams"
chmod +x "$stub/ams"
fail=0
check() { # check allow|pass <command>
    out=$(python3 -c 'import json,sys; print(json.dumps({"tool_input": {"command": sys.argv[1]}, "cwd": sys.argv[2], "session_id": "selfcheck"}))' "$2" "$stub" \
        | PATH="$stub:$PATH" bash "$hook" 2>/dev/null || true)
    case "$out" in *'"allow"'*) got=allow ;; *) got=pass ;; esac
    [ "$got" = "$1" ] || { echo "FAIL: expected $1, got $got: $2"; fail=1; }
}
check allow 'ams find Foo'
check allow 'ams describe crates/x.rs'
check allow 'ams cycles crates'
check allow "cd '/projects/my app' && ams tree crates"
check allow 'cd crates && ams refs SyncStats'
check allow 'rtk ams find Foo'
check allow "rtk proxy ams search 'foo bar'"
check allow 'sudo ams find Foo'
check pass 'ams build'
check pass 'ams find x && rm -rf y'
check pass 'ams find x > ~/.bashrc'
check pass 'ams find $(rm -rf /tmp/x)'
check pass 'ams find `id`'
check pass 'cd $(touch /tmp/p) && ams find x'
check pass 'cd a && cd b && ams find x'
check pass 'PATH=/tmp/evil ams find x'
check pass '/tmp/evil/ams find x'
check pass 'proxy ams find x'
[ "$fail" = 0 ] && echo "pre-bash allow: ok"
exit "$fail"
