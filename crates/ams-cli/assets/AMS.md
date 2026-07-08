# AMS — code navigation via signatures, not file reads

Applies when the project has `.ams/index.db` (check silently). If `ams` is
installed but there's no index and the project is code-heavy — run `ams build`
once at the root (takes seconds). No `ams` binary → ignore this section.

## Mandatory workflow

**Before Read on an unfamiliar code file:**
1. `ams describe <file>` — signatures with exact `@start-end` spans, 10–40× cheaper than Read
2. `Read(offset=start, limit=end-start+1)` — only the span you need, never the whole file

**Before Grep for a symbol definition:** `ams find <name>` (exact spans, cross-language).
**Orienting in a directory:** `ams tree <dir>` — instead of Glob + serial Reads; high `api`+`used-by` = hub file.
**Before changing any exported API:** `ams refs <name>` (call sites) + `ams related <file>` (reverse deps — what breaks; `--depth 2` for the transitive blast radius, rolled up by directory).
**Exact name unknown:** `ams search <words>` — full-text over names/signatures/docstrings, any language.
**Untangling module structure:** `ams cycles [dir]` — dependency cycles over resolved imports.

## Boundaries

Grep stays for strings, log messages, config values, comments — ams indexes
structure, not text. `refs` doesn't see dynamic dispatch/string lookups: empty
result + suspicion → fall back to Grep. Index self-heals on every query —
never stale, no rebuild step.

Languages: TS/TSX, JS/JSX, Rust, Python, Go, PHP, Java, Kotlin, C#, Ruby.
