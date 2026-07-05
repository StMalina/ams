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

## Measured token savings

Numbers from real repositories (not benchmarks-in-a-vacuum):

| Repository | Files | Index time | Signature vs source |
|---|---|---|---|
| Node/Express legacy (330 js) | 330 | 0.57 s | **33×** smaller |
| Python bot (199 py) | 199 | 0.40 s | **18×** smaller |
| Mixed Rust + React (147 rs/ts/tsx) | 147 | 0.34 s | **12×** smaller |

A concrete navigation task — "where are sessions revoked?" on the mixed
repo: the `ams find` + `ams refs` path cost **372 bytes** of output; the
default path (grep, then read the 40 KB route file) costs ~100× more, and
a realistic grep-then-windowed-read still costs ~10× more. As a bonus, `ams`
also returned the frontend counterpart (`revokeSessions` in `client.ts`)
that a `--include='*.rs'` grep silently missed.

Where the wins actually come from:

1. **`used-by` (reverse dependencies) — information agents normally don't
   have at all.** Resolving who imports a file across relative paths,
   index re-exports, and CommonJS/ESM mixes is so expensive with grep that
   agents usually skip impact analysis entirely. `ams tree` shows it as a
   column; `ams related` lists the exact files.
2. **Exact spans kill re-reads.** Grep gives you the start of a function,
   never the end — so agents read `offset, limit=80` and guess. `@243-370`
   means exactly one targeted read.
3. **Savings concentrate in the recon phase.** Orienting in an unfamiliar
   repo drops from 5–15 full-file reads to one `tree` + a few `describe`
   calls: 50–80% fewer navigation tokens. During the editing phase savings
   are smaller — you still read the bodies you change.

## Install

Prebuilt binary (Linux x64/arm64, macOS x64/arm64 — static musl on Linux):

```bash
curl -fsSL https://raw.githubusercontent.com/StMalina/ams/main/install.sh | sh
```

Windows: download the `.zip` from
[Releases](https://github.com/StMalina/ams/releases). From source:

```bash
git clone https://github.com/StMalina/ams
cd ams
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
| `ams build [path]` | — | full index (init + reindex) at `<path>/.ams/index.db`; `--force` reparses all |
| `ams describe <file\|dir>...` | `Read` of a whole file | signatures with `@start-end` spans |
| `ams find <name>` | `Grep` for `fn X\|class X` | definitions: file, span, signature |
| `ams search <words>` | not really possible | full-text over names/signatures/docs, ranked (FTS5) |
| `ams refs <name>` | `Grep` for a name, with noise | usages: file, line |
| `ams tree [dir]` | `Glob` + a series of `Read`s | one line per file; auto directory rollup on big projects; `--hubs`, `--depth N` |
| `ams related <file>` | manually reading imports | deps + reverse deps (used-by) |
| `ams annotate <file>:<Symbol.path> "doc"` | — | attach an LLM note to a symbol |
| `ams gain` | — | accumulated token savings: output printed vs source covered |

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
src/auth.ts     ts   820 loc  api:8   used-by:12  jwt,redis
src/router.ts   ts   340 loc  api:3   used-by:2   express
```

High `api` + high `used-by` marks the hub files of a project.

### `ams search` — find by meaning

Docstrings and doc comments (`///`, `/** */`, `"""..."""`, `//`) are
extracted into the index (shown as `doc*:` in `describe`) and searchable
together with symbol names and signatures. CamelCase/snake_case names are
word-split, terms are prefix-matched, any human language works:

```
$ ams search password
src/Security/Hasher/PasswordHash.php:7-13  [class] PasswordHash exported
$ ams search Проверяет
src/AccessRights/RoleManager.php:20-23  [method] RoleManager.canManage exported
  doc*: Проверяет, может ли одна роль управлять другой ролью
```

This closes the loop with the skill recommendation "give every new function
a one-line docstring": agents document code as they write it, and the index
makes it findable by meaning for free.

### `ams find`

```
$ ams find validateToken
src/auth.ts:4-8  [fn] validateToken exported
  function validateToken(token: string): User | null
```

### `ams refs`

```
$ ams refs validateToken
src/auth.ts: calls 55, 102
src/router.ts: value 12
```

`calls` are direct call sites; `value` means the identifier is passed
around as a value — handler registrations (`route("/x", delete(handler))`),
callbacks, exports. When a name is too common (`get`, `run`: 20+ files) the
output collapses to per-file counts — narrow it with `--in <dir>`. Dynamic
dispatch and string-based lookups are not indexed; fall back to text grep
for those.

### `ams gain` — measure it yourself

Every query logs two numbers into the index: how many bytes ams printed and
the total size of the source files the answer covered (what full reads would
have cost). `ams gain` shows the running totals per command:

```
$ ams gain
cmd         calls     output       source   ratio
describe        2    39.4 KB      68.4 KB      2x
tree            1     3.4 KB       1.7 MB    525x
refs            1     1.7 KB     880.2 KB    526x
total           6    49.8 KB       3.6 MB     75x
```

`source` is an upper bound (an agent wouldn't read every covered file), but
the asymmetry is the point: navigation answers cost KBs, the files they
summarize cost MBs.

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

**Claude Code plugin** — install from this repo (it doubles as a plugin
marketplace):

```
/plugin marketplace add StMalina/ams
/plugin install ams@ams
```

`plugin/` contains a manifest
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

TypeScript, TSX, JavaScript, JSX (ESM and CommonJS), Rust, Python, Go, PHP,
Java, Kotlin, C#, Ruby. Minified/generated files (>2 MB or 2500+ char lines)
are skipped automatically.

Reverse-dependency (`used-by`) resolution: JS/TS relative imports, Python
modules, Rust `crate::` paths, PHP `require` paths + PSR-4 namespaces from
composer.json, Go packages via go.mod, Java/Kotlin imports under common
source roots (`src/main/java`, ...). Everything else (external packages)
shows as an external dep.

## Architecture

- `crates/ams-core` — tree-sitter parsers (one module per language),
  the SQLite index and its staleness/query logic, and the shared data
  model (`Symbol`, `FileSig`, `Span`, `Annotation`).
- `crates/ams-cli` — the `clap`-based CLI and text/JSON output
  formatters.
- `.ams/index.db` — per-project SQLite database created by `ams build`,
  holding files, symbols, imports, identifier-level refs, and annotations.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
