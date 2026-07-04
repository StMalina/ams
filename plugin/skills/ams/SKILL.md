---
name: ams
description: Navigate and understand code without reading full files. Use before Read on an unfamiliar file, before Grep for a symbol definition, when asked "what's in this file/directory", "where is X defined", "who calls X", or before exploring a codebase's structure. Triggers on code navigation, symbol search, directory overview, and impact analysis (who uses this).
---

# ams — AI Module Signatures

`ams` is a Rust CLI that indexes a codebase with tree-sitter into SQLite
(`.ams/index.db`) and prints compact signatures instead of full source. Every
symbol carries an exact line span `@start-end`, so you read source only when
you need the body, and only the lines you need.

Every `ams` command self-heals: it stat-walks the tree and reparses only
changed files before answering, so the index is never stale by construction.

## Setup (check once per session)

```bash
which ams
```

If empty, install from the AMS repo:

```bash
cargo install --path crates/ams-cli   # run from the ams repo root
```

If `ams` exists but the current project has no index:

```bash
ls .ams/index.db 2>/dev/null || ams build
```

`ams build` walks the project root and creates `.ams/index.db`. Re-running it
is cheap and safe (incremental).

## Workflow — replace these habits

### (a) Unfamiliar file → `describe`, not `Read`

```
$ ams describe src/auth.ts
src/auth.ts [25 loc, ts]
  function validateToken(token: string): User | null  @4-8 exported
  class AuthService  @12-21 exported
    async login(creds: Credentials): Promise<Session>  @13-16
      doc: создаёт сессию после проверки токена
  imports: jwt, ./cache
```

One call replaces reading the whole file. It also works on a directory
(prints one description per file).

### (b) "Where is X defined?" → `find`, not `Grep`

```
$ ams find validateToken
src/auth.ts:4-8  [fn] validateToken exported
  function validateToken(token: string): User | null
```

Filter with `--kind fn|method|class|struct|enum|trait|interface|const|type|mod`
and/or `--exported`. Do not `Grep -r "function validateToken"` — `find` is
faster and returns the exact span.

### (c) "Who calls X?" → `refs`

```
$ ams refs validateToken
src/auth.ts: 4, 55, 102
src/router.ts: 12
```

Impact analysis before renaming/changing a signature: check `refs` first.

### (d) Directory overview → `tree`

```
$ ams tree src/
src/auth.ts      820 loc  api:8   used-by:12  jwt,redis
src/router.ts    340 loc  api:3   used-by:2   express
```

One line per file: size, exported-API count, reverse-dependency count,
external deps. Use this instead of `Glob` + a series of `Read` calls to get
oriented in a new directory.

### (e) Dependencies / reverse-dependencies → `related`

```
$ ams related src/auth.ts
src/auth.ts
  deps internal: ./cache, ./session
  deps external: jwt
  used-by: src/router.ts, src/app.ts
```

Use before touching a file's exported API, to see what would break.

### (f) Reading the actual implementation → `Read` with the span

Once `describe`/`find` gives you `@13-16`, read exactly that range instead of
the whole file:

```
Read(file_path="src/auth.ts", offset=13, limit=4)
```

### (g) Learned something non-obvious → `annotate`

When you figure out *why* a function does something (not visible from its
signature), record it so future sessions/agents don't re-derive it:

```bash
ams annotate src/auth.ts:AuthService.login "creates a session only after 2FA check passes"
```

The note is keyed to the symbol body's hash. It survives reindexing and
shows up in `describe` output as `doc: ...`. If the function body later
changes, the note is kept but flagged `[stale]` — verify and re-annotate.

## What ams does NOT replace

Text/regex search over file contents (log lines, string literals, comments,
config values, arbitrary patterns) is still `Grep`/`rg`. `ams` indexes code
*structure* (symbols, signatures, imports, call-name references) — not
arbitrary text. Use `Grep` when you're searching for a string, not a symbol.

## Flags

- `--json` (global): machine-readable output instead of compact text.
- `--kind <fn|method|class|struct|enum|trait|interface|const|type|mod>` on `find`.
- `--exported` on `describe`/`find`: only exported/public symbols.

## Supported languages

TypeScript/TSX, JavaScript/JSX, Rust, Python, Go.
