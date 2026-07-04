# AMS — AI Module Signatures

A single Rust binary that indexes a codebase with tree-sitter into SQLite and
prints compact code signatures instead of file contents — so AI coding
agents stop burning tokens reading whole files just to find their way
around.

## The problem

An agent exploring an unfamiliar repo typically opens a file, reads it end
to end, decides it's the wrong one, and opens the next. Multiply that by a
session and most of the token budget goes to navigation, not the actual
task. Existing tools don't quite fit: `aider`'s repo-map is baked into
`aider` itself, Serena is a full LSP-backed MCP server, and `ctags` gives you
raw tags with no semantics (exports, doc, call sites) and no CLI ergonomics
for an agent. AMS is a lightweight, standalone binary an agent can shell out
to directly.

AMS parses source with tree-sitter, stores symbols/signatures/imports/refs
in SQLite (`.ams/index.db`), and answers queries with compact text: every
symbol carries an exact `@start-end` line span, so the agent follows up with
a targeted `Read(offset, limit)` instead of loading the entire file.

## Install

```bash
git clone <this repo>
cd AMS
cargo install --path crates/ams-cli
ams --version
```

## Quick start

```bash
cd /path/to/your/project
ams build                       # creates .ams/index.db
ams describe src/auth.ts        # signatures instead of full source
ams find validateToken          # where is it defined
ams refs validateToken          # who calls it
```

## Commands

| Command | Replaces | Output |
|---|---|---|
| `ams build [path]` | — | full index (init + reindex) at `<path>/.ams/index.db` |
| `ams describe <file\|dir>...` | `Read` of a whole file | signatures with `@start-end` spans |
| `ams find <name>` | `Grep` for `fn X\|class X` | definitions: file, span, signature |
| `ams refs <name>` | `Grep` for a name, with noise | usages: file, line |
| `ams tree [dir]` | `Glob` + a series of `Read`s | one line per file: loc, api count, used-by, deps |
| `ams related <file>` | manually reading imports | deps + reverse deps (used-by) |
| `ams annotate <file>:<Symbol.path> "doc"` | — | attach an LLM note to a symbol |

Flags: `--kind fn|method|class|struct|enum|trait|interface|const|type|mod`
and `--exported` (on `describe`/`find`), global `--json` for machine-readable
output on any command.

### `ams describe`

```
$ ams describe src/auth.ts
src/auth.ts [25 loc, ts]
  function validateToken(token: string): User | null  @4-8 exported
  class AuthService  @12-21 exported
    async login(creds: Credentials): Promise<Session>  @13-16
      doc: создаёт сессию после проверки токена
  imports: jwt, ./cache
```

### `ams tree`

```
$ ams tree src/
src/ [14 files]
  auth.ts      820 loc  api:8   used-by:12  jwt,redis
  router.ts    340 loc  api:3   used-by:2   express
```

### `ams find`

```
$ ams find validateToken
src/auth.ts:4-8  [fn] validateToken exported
  function validateToken(token: string): User | null
```

### `ams refs`

```
$ ams refs validateToken
src/auth.ts: 4, 55, 102
src/router.ts: 12
```

### `ams related`

```
$ ams related src/auth.ts
src/auth.ts
  deps internal: ./cache, ./session
  deps external: jwt
  used-by: src/router.ts, src/app.ts
```

## Staleness and annotations

Every command self-heals before answering: it stat-walks the project,
rehashes changed files, and reparses only what changed (milliseconds for a
typical edit). There is no separate "reindex" step to remember — the index
is never stale by construction.

Annotations (`ams annotate`) are LLM-written notes attached to a symbol,
keyed by the symbol's path *and* a hash of its body — not by line number.
They live in their own table, so `ams build`/reindexing never wipes them.
When the annotated symbol's body changes, the note is kept but shown with a
`[stale]` marker in `describe` output, signaling it should be re-verified.

## Integration

**Claude Code plugin** — `plugin/` contains a manifest
(`plugin/.claude-plugin/plugin.json`), a skill
(`plugin/skills/ams/SKILL.md`) that teaches the agent the
describe/find/refs/tree/related/annotate workflow in place of raw
Read/Grep, and a `SessionStart` hook (`plugin/hooks/`) that reminds the
agent to use `ams` when an index is present, or suggests `ams build` when
one is missing. The `ams` binary itself is not bundled in the plugin (it's
platform-specific) — install it separately via `cargo install`.

**Other agents (Codex, Gemini CLI, ...)** — copy the workflow from
`AGENTS.md.template` into your project's `AGENTS.md`.

## Supported languages

TypeScript, TSX, JavaScript, JSX, Rust, Python, Go.

## Architecture

- `crates/ams-core` — tree-sitter parsers (one module per language),
  the SQLite index and its staleness/query logic, and the shared data
  model (`Symbol`, `FileSig`, `Span`, `Annotation`).
- `crates/ams-cli` — the `clap`-based CLI and text/JSON output
  formatters.
- `.ams/index.db` — per-project SQLite database created by `ams build`,
  holding files, symbols, imports, identifier-level refs, and annotations.
