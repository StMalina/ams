---
name: ams
description: Navigate code without reading full files. Use before Read on an unfamiliar file, before Grep for a symbol definition, when asked "what's in this file/directory", "where is X defined", "who calls/uses X", or before changing any exported API. Triggers on code navigation, symbol search, directory overview, and impact analysis.
---

# ams — compact code signatures instead of file reads

Indexed signatures with exact line spans. One `describe` costs 10–40× fewer
tokens than reading the file. The index self-heals on every query — never
stale, no rebuild needed.

Setup once: `which ams` (install: `cargo install --path crates/ams-cli` from
the AMS repo). No `.ams/` in project → run `ams build` once at the root.

## The 3 commands that cover 90% of navigation

**1. Orient in a directory — `ams tree [dir]`** (instead of Glob + Reads)

```
$ ams tree src/
src/auth.ts   ts   820 loc  api:8   used-by:12   jwt,redis
src/util.ts   ts    40 loc  api:2   used-by:31
```

High `api` + high `used-by` = hub file, start there. On big projects the
output auto-rolls-up by directory; `--hubs` shows the top-20 most-imported
files, `--depth 0` forces the flat list.

**2. See inside a file — `ams describe <file>`** (instead of Read)

```
$ ams describe src/auth.ts
src/auth.ts [820 loc, ts, used-by:12]
  function validateToken(token: string): User | null  @42-78 exported
    doc*: Validates the JWT and loads the session
  class AuthService  @97-410 exported
    async login(creds): Promise<Session>  @103-160
      doc: creates session only after 2FA passes
  imports: jwt, redis, ./session
```

`doc*` comes from the source (docstring/doc comment); `doc` is an
out-of-band `ams annotate` note.

**3. Read only the body you need — `Read` with the span**

`@103-160` → `Read(file_path="src/auth.ts", offset=103, limit=58)`.
Never read a whole file when you already have its spans.

## Before changing any exported API

- `ams related <file>` — deps + reverse deps (`used-by`): what breaks if you
  change this file. This is info you can't cheaply get with Grep.
- `ams refs <name>` — usage sites: `calls 12, 45 | value 88`. `value` = passed
  as handler/callback (router registrations etc.). Not indexed: dynamic
  dispatch, string-based lookups — if `refs` is empty but you suspect usage,
  fall back to Grep.
- `ams find <name>` — where a symbol is defined (substring match, exact spans,
  cross-language).
- `ams search <words>` — full-text search over names, signatures, and docs
  when you don't know the exact name: `ams search password reset` finds
  `MemberPasswordGenerator` (camelCase is word-split; prefixes match; works
  in any language, including Cyrillic docstrings).

## Rarely needed, good to know

- **When you write a new function, give it a one-line docstring/doc-comment**
  (the *why/purpose*, not a restatement of the signature). It gets indexed
  for free and makes the code findable by meaning later.
- `ams annotate <file>:<Symbol.path> "note"` — same idea for code you should
  NOT edit (legacy, third-party, foreign modules): persist a non-obvious
  insight out-of-band. Survives reindexing; flagged `[stale]` if the body
  changes.
- `--exported` (describe/find) — public surface only; `--kind fn|class|...` on find.
- `--json` — machine-readable output.
- `ams build --force` — full reparse (only after an ams binary upgrade acts odd).

## Not a Grep replacement

Searching for strings, log messages, config values, comments → still Grep.
ams indexes structure (symbols, spans, imports, references), not text.
Languages: TypeScript/TSX, JavaScript/JSX (ESM+CommonJS), Rust, Python, Go, PHP.
