use crate::model::*;
use crate::parser::{parser_for_ext, SUPPORTED_EXTS};
use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "__pycache__",
    "venv",
    ".venv",
    "vendor",
    ".ams",
];

pub struct Index {
    conn: Connection,
    pub root: PathBuf,
}

#[derive(Debug, Default)]
pub struct SyncStats {
    pub parsed: u32,
    pub removed: u32,
    pub total: u32,
}

impl Index {
    /// Create (or reuse) the index under `root/.ams/`.
    pub fn create(root: &Path) -> Result<Self> {
        let root = root
            .canonicalize()
            .with_context(|| format!("path not found: {}", root.display()))?;
        let dir = root.join(".ams");
        std::fs::create_dir_all(&dir)?;
        let conn = Connection::open(dir.join("index.db"))?;
        let idx = Index { conn, root };
        idx.init_schema()?;
        Ok(idx)
    }

    /// Find an existing index by walking up from `start`.
    pub fn open_existing(start: &Path) -> Result<Self> {
        let start = start.canonicalize()?;
        let mut cur = start.as_path();
        loop {
            let candidate = cur.join(".ams/index.db");
            if candidate.exists() {
                let conn = Connection::open(&candidate)?;
                let idx = Index {
                    conn,
                    root: cur.to_path_buf(),
                };
                idx.init_schema()?;
                return Ok(idx);
            }
            match cur.parent() {
                Some(p) => cur = p,
                None => bail!(
                    "no .ams index found from {} upward — run `ams build` in the project root",
                    start.display()
                ),
            }
        }
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS files(
               id INTEGER PRIMARY KEY,
               path TEXT UNIQUE NOT NULL,
               lang TEXT NOT NULL,
               loc INTEGER NOT NULL,
               hash TEXT NOT NULL,
               mtime INTEGER NOT NULL,
               size INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS symbols(
               id INTEGER PRIMARY KEY,
               file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
               parent_id INTEGER REFERENCES symbols(id) ON DELETE CASCADE,
               name TEXT NOT NULL,
               kind TEXT NOT NULL,
               signature TEXT NOT NULL,
               start_line INTEGER NOT NULL,
               end_line INTEGER NOT NULL,
               exported INTEGER NOT NULL,
               body_hash TEXT NOT NULL,
               doc TEXT
             );
             CREATE INDEX IF NOT EXISTS symbols_name ON symbols(name);
             CREATE INDEX IF NOT EXISTS symbols_file ON symbols(file_id);
             CREATE TABLE IF NOT EXISTS imports(
               file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
               target TEXT NOT NULL,
               resolved_file_id INTEGER
             );
             CREATE INDEX IF NOT EXISTS imports_file ON imports(file_id);
             CREATE INDEX IF NOT EXISTS imports_resolved ON imports(resolved_file_id);
             CREATE TABLE IF NOT EXISTS refs(
               file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
               name TEXT NOT NULL,
               line INTEGER NOT NULL,
               kind TEXT NOT NULL DEFAULT 'call'
             );
             CREATE INDEX IF NOT EXISTS refs_name ON refs(name);
             CREATE TABLE IF NOT EXISTS annotations(
               key TEXT PRIMARY KEY,
               body_hash TEXT NOT NULL,
               doc TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS meta(
               key TEXT PRIMARY KEY,
               value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS stats(
               ts INTEGER NOT NULL,
               cmd TEXT NOT NULL,
               output_bytes INTEGER NOT NULL,
               source_bytes INTEGER NOT NULL
             );
             CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts
               USING fts5(name, signature, doc, path);",
        )?;
        // Migrations for older databases.
        let _ = self
            .conn
            .execute("ALTER TABLE refs ADD COLUMN kind TEXT NOT NULL DEFAULT 'call'", []);
        let _ = self.conn.execute("ALTER TABLE symbols ADD COLUMN doc TEXT", []);
        // Parser output changes between versions; stored signatures from an
        // older binary would silently diverge. Wipe files (annotations are
        // hash-anchored and survive) and let sync() reparse everything.
        let version = env!("CARGO_PKG_VERSION");
        let stored: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'parser_version'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if stored.as_deref() != Some(version) {
            self.clear_files()?;
            self.conn.execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES ('parser_version', ?1)",
                params![version],
            )?;
        }
        Ok(())
    }

    /// Drop all indexed data (annotations survive); next sync reparses from scratch.
    pub fn clear_files(&self) -> Result<()> {
        self.conn.execute("DELETE FROM files", [])?;
        self.conn.execute("DELETE FROM symbols_fts", [])?;
        Ok(())
    }

    /// Bring the index in line with the filesystem. Cheap when nothing changed
    /// (stat-only walk); reparses only files whose mtime+size or hash differ.
    pub fn sync(&mut self) -> Result<SyncStats> {
        let mut stats = SyncStats::default();
        let mut seen: HashSet<String> = HashSet::new();

        let known: HashMap<String, (i64, i64, String)> = self
            .conn
            .prepare("SELECT path, mtime, size, hash FROM files")?
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?)))
            })?
            .collect::<rusqlite::Result<_>>()?;

        let walker = ignore::WalkBuilder::new(&self.root)
            .filter_entry(|e| {
                e.file_name()
                    .to_str()
                    .map_or(true, |n| !SKIP_DIRS.contains(&n))
            })
            .build();

        let tx = self.conn.transaction()?;
        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().map_or(false, |t| t.is_file()) {
                continue;
            }
            let path = entry.path();
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if !SUPPORTED_EXTS.contains(&ext) {
                continue;
            }
            let rel = path
                .strip_prefix(&self.root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            seen.insert(rel.clone());
            stats.total += 1;

            let meta = entry.metadata()?;
            let mtime = meta
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let size = meta.len() as i64;

            if let Some((db_mtime, db_size, _)) = known.get(&rel) {
                if *db_mtime == mtime && *db_size == size {
                    continue; // fast path
                }
            }

            let source = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(_) => continue, // non-UTF8 or unreadable
            };
            if is_generated(&source, size) {
                continue; // minified bundles / huge generated files
            }
            let hash = blake3::hash(source.as_bytes()).to_hex().to_string();
            if let Some((_, _, db_hash)) = known.get(&rel) {
                if *db_hash == hash {
                    tx.execute(
                        "UPDATE files SET mtime = ?1, size = ?2 WHERE path = ?3",
                        params![mtime, size, rel],
                    )?;
                    continue;
                }
            }

            let parser = parser_for_ext(ext).unwrap();
            let parsed = parser
                .parse(&source)
                .with_context(|| format!("parse failed: {rel}"))?;
            upsert_file(&tx, &rel, ext, &parsed, &hash, mtime, size)?;
            stats.parsed += 1;
        }

        for path in known.keys() {
            if !seen.contains(path) {
                tx.execute("DELETE FROM files WHERE path = ?1", params![path])?;
                stats.removed += 1;
            }
        }

        if stats.parsed > 0 || stats.removed > 0 {
            resolve_imports(&tx, &self.root)?;
        }
        tx.commit()?;
        Ok(stats)
    }

    /// Convert a user-supplied path (relative to cwd or absolute) into the
    /// index-relative form.
    pub fn rel_path(&self, user_path: &str) -> Result<String> {
        let p = PathBuf::from(user_path);
        let abs = if p.is_absolute() {
            p
        } else {
            std::env::current_dir()?.join(p)
        };
        let abs = normalize(&abs);
        Ok(abs
            .strip_prefix(&self.root)
            .map_err(|_| anyhow!("{} is outside the indexed root {}", user_path, self.root.display()))?
            .to_string_lossy()
            .replace('\\', "/"))
    }

    fn file_id(&self, rel: &str) -> Result<i64> {
        self.conn
            .query_row("SELECT id FROM files WHERE path = ?1", params![rel], |r| {
                r.get(0)
            })
            .optional()?
            .ok_or_else(|| anyhow!("not indexed: {rel} (unsupported language or excluded dir?)"))
    }

    pub fn describe(&self, rel: &str) -> Result<FileDescription> {
        let (file_id, lang, loc): (i64, String, u32) = self
            .conn
            .query_row(
                "SELECT id, lang, loc FROM files WHERE path = ?1",
                params![rel],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .ok_or_else(|| anyhow!("not indexed: {rel}"))?;

        let annotations: HashMap<String, (String, String)> = self
            .conn
            .prepare("SELECT key, body_hash, doc FROM annotations WHERE key LIKE ?1")?
            .query_map(params![format!("{rel}:%")], |r| {
                Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?)))
            })?
            .collect::<rusqlite::Result<_>>()?;

        let symbols = self.load_symbols(file_id, None, rel, "", &annotations)?;

        let imports: Vec<String> = self
            .conn
            .prepare("SELECT target FROM imports WHERE file_id = ?1")?
            .query_map(params![file_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;

        Ok(FileDescription {
            path: rel.to_string(),
            lang,
            loc,
            symbols,
            imports,
            used_by: self.used_by(file_id)?,
        })
    }

    fn load_symbols(
        &self,
        file_id: i64,
        parent_id: Option<i64>,
        rel: &str,
        prefix: &str,
        annotations: &HashMap<String, (String, String)>,
    ) -> Result<Vec<SymbolInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, kind, signature, start_line, end_line, exported, body_hash, doc
             FROM symbols WHERE file_id = ?1 AND parent_id IS ?2 ORDER BY start_line",
        )?;
        #[allow(clippy::type_complexity)]
        let rows: Vec<(i64, String, String, String, u32, u32, bool, String, Option<String>)> = stmt
            .query_map(params![file_id, parent_id], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;

        let mut out = Vec::with_capacity(rows.len());
        for (id, name, kind, sig, start, end, exported, body_hash, docstring) in rows {
            let symbol_path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}.{name}")
            };
            let key = format!("{rel}:{symbol_path}");
            let (doc, doc_stale) = match annotations.get(&key) {
                Some((anno_hash, doc)) => (Some(doc.clone()), *anno_hash != body_hash),
                None => (None, false),
            };
            out.push(SymbolInfo {
                children: self.load_symbols(file_id, Some(id), rel, &symbol_path, annotations)?,
                name,
                kind: SymbolKind::from_str(&kind).unwrap_or(SymbolKind::Function),
                signature: sig,
                start_line: start,
                end_line: end,
                exported,
                docstring,
                doc,
                doc_stale,
            });
        }
        Ok(out)
    }

    fn used_by(&self, file_id: i64) -> Result<Vec<String>> {
        Ok(self
            .conn
            .prepare(
                "SELECT DISTINCT f.path FROM imports i JOIN files f ON f.id = i.file_id
                 WHERE i.resolved_file_id = ?1 ORDER BY f.path",
            )?
            .query_map(params![file_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn find(
        &self,
        pattern: &str,
        kind: Option<SymbolKind>,
        exported_only: bool,
    ) -> Result<Vec<FindHit>> {
        let mut sql = String::from(
            "SELECT f.path, s.name, p.name, s.kind, s.signature, s.start_line, s.end_line, s.exported, s.doc
             FROM symbols s
             JOIN files f ON f.id = s.file_id
             LEFT JOIN symbols p ON p.id = s.parent_id
             WHERE s.name LIKE ?1",
        );
        if let Some(k) = kind {
            sql.push_str(&format!(" AND s.kind = '{}'", k.as_str()));
        }
        if exported_only {
            sql.push_str(" AND s.exported = 1");
        }
        sql.push_str(" ORDER BY s.exported DESC, f.path, s.start_line LIMIT 200");

        Ok(self
            .conn
            .prepare(&sql)?
            .query_map(params![format!("%{pattern}%")], |r| {
                let name: String = r.get(1)?;
                let parent: Option<String> = r.get(2)?;
                let kind_s: String = r.get(3)?;
                Ok(FindHit {
                    path: r.get(0)?,
                    symbol_path: match parent {
                        Some(p) => format!("{p}.{name}"),
                        None => name,
                    },
                    kind: SymbolKind::from_str(&kind_s).unwrap_or(SymbolKind::Function),
                    signature: r.get(4)?,
                    start_line: r.get(5)?,
                    end_line: r.get(6)?,
                    exported: r.get(7)?,
                    doc: r.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Full-text search over symbol names, signatures, and docs (docstrings
    /// plus annotate notes). Terms are AND-ed; ranked by bm25.
    pub fn search(&self, query: &str) -> Result<Vec<FindHit>> {
        // Quote (syntax-safe) and prefix-match each term: `passw` finds `password`.
        let fts_query = query
            .split_whitespace()
            .map(|t| format!("\"{}\"*", t.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" ");
        if fts_query.is_empty() {
            return Ok(vec![]);
        }
        Ok(self
            .conn
            .prepare(
                "SELECT fl.path, s.name, p.name, s.kind, s.signature,
                        s.start_line, s.end_line, s.exported, s.doc
                 FROM symbols_fts f
                 JOIN symbols s ON s.id = f.rowid
                 JOIN files fl ON fl.id = s.file_id
                 LEFT JOIN symbols p ON p.id = s.parent_id
                 WHERE symbols_fts MATCH ?1
                 ORDER BY rank LIMIT 20",
            )?
            .query_map(params![fts_query], |r| {
                let name: String = r.get(1)?;
                let parent: Option<String> = r.get(2)?;
                let kind_s: String = r.get(3)?;
                Ok(FindHit {
                    path: r.get(0)?,
                    symbol_path: match parent {
                        Some(p) => format!("{p}.{name}"),
                        None => name,
                    },
                    kind: SymbolKind::from_str(&kind_s).unwrap_or(SymbolKind::Function),
                    signature: r.get(4)?,
                    start_line: r.get(5)?,
                    end_line: r.get(6)?,
                    exported: r.get(7)?,
                    doc: r.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Record one query's output size vs the source bytes it covered
    /// (the files the agent would otherwise have read). Powers `ams gain`.
    pub fn log_stat(&self, cmd: &str, output_bytes: usize, paths: &[&str]) -> Result<()> {
        let unique: HashSet<&str> = paths.iter().copied().collect();
        let mut source: i64 = 0;
        {
            let mut stmt = self
                .conn
                .prepare_cached("SELECT size FROM files WHERE path = ?1")?;
            for p in unique {
                if let Some(sz) = stmt
                    .query_row(params![p], |r| r.get::<_, i64>(0))
                    .optional()?
                {
                    source += sz;
                }
            }
        }
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn.execute(
            "INSERT INTO stats(ts, cmd, output_bytes, source_bytes) VALUES (?1, ?2, ?3, ?4)",
            params![ts, cmd, output_bytes as i64, source],
        )?;
        Ok(())
    }

    pub fn gain(&self) -> Result<Vec<GainRow>> {
        Ok(self
            .conn
            .prepare(
                "SELECT cmd, COUNT(*), SUM(output_bytes), SUM(source_bytes)
                 FROM stats GROUP BY cmd ORDER BY COUNT(*) DESC",
            )?
            .query_map([], |r| {
                Ok(GainRow {
                    cmd: r.get(0)?,
                    calls: r.get(1)?,
                    output_bytes: r.get(2)?,
                    source_bytes: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn refs(&self, name: &str, in_dir: Option<&str>) -> Result<Vec<RefHit>> {
        let like = match in_dir {
            Some(d) if !d.is_empty() => format!("{}/%", d.trim_end_matches('/')),
            _ => "%".to_string(),
        };
        Ok(self
            .conn
            .prepare(
                "SELECT f.path, r.line, r.kind FROM refs r JOIN files f ON f.id = r.file_id
                 WHERE r.name = ?1 AND f.path LIKE ?2
                 ORDER BY f.path, r.kind, r.line LIMIT 500",
            )?
            .query_map(params![name, like], |r| {
                let kind: String = r.get(2)?;
                Ok(RefHit {
                    path: r.get(0)?,
                    line: r.get(1)?,
                    kind: if kind == "value" {
                        RefKind::Value
                    } else {
                        RefKind::Call
                    },
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn tree(&self, dir_prefix: Option<&str>) -> Result<Vec<TreeEntry>> {
        let like = match dir_prefix {
            Some(d) if !d.is_empty() => format!("{}/%", d.trim_end_matches('/')),
            _ => "%".to_string(),
        };
        let mut stmt = self.conn.prepare(
            "SELECT f.id, f.path, f.lang, f.loc,
               (SELECT COUNT(*) FROM symbols s
                  WHERE s.file_id = f.id AND s.parent_id IS NULL AND s.exported = 1),
               (SELECT COUNT(DISTINCT i.file_id) FROM imports i WHERE i.resolved_file_id = f.id)
             FROM files f WHERE f.path LIKE ?1 ORDER BY f.path",
        )?;
        let rows: Vec<(i64, String, String, u32, u32, u32)> = stmt
            .query_map(params![like], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;

        let mut out = Vec::with_capacity(rows.len());
        for (id, path, lang, loc, api, used_by) in rows {
            let deps: Vec<String> = self
                .conn
                .prepare(
                    "SELECT DISTINCT target FROM imports
                     WHERE file_id = ?1 AND resolved_file_id IS NULL AND target NOT LIKE '.%'
                     LIMIT 5",
                )?
                .query_map(params![id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            out.push(TreeEntry {
                path,
                lang,
                loc,
                api_count: api,
                used_by_count: used_by,
                deps,
            });
        }
        Ok(out)
    }

    pub fn related(&self, rel: &str) -> Result<RelatedInfo> {
        let file_id = self.file_id(rel)?;
        let mut internal = Vec::new();
        let mut external = Vec::new();
        let rows: Vec<(String, Option<String>)> = self
            .conn
            .prepare(
                "SELECT i.target, f.path FROM imports i
                 LEFT JOIN files f ON f.id = i.resolved_file_id
                 WHERE i.file_id = ?1",
            )?
            .query_map(params![file_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (target, resolved) in rows {
            match resolved {
                Some(p) => internal.push(p),
                None => external.push(target),
            }
        }
        internal.sort();
        internal.dedup();
        external.sort();
        external.dedup();
        Ok(RelatedInfo {
            path: rel.to_string(),
            internal_deps: internal,
            external_deps: external,
            used_by: self.used_by(file_id)?,
        })
    }

    /// Attach an LLM-written doc to `rel:symbol_path`. Bound to the current
    /// body hash — survives reindexing, flagged stale when the body changes.
    pub fn annotate(&self, rel: &str, symbol_path: &str, doc: &str) -> Result<()> {
        let file_id = self.file_id(rel)?;
        let mut parent_id: Option<i64> = None;
        let mut body_hash = String::new();
        for part in symbol_path.split('.') {
            let row: Option<(i64, String)> = self
                .conn
                .query_row(
                    "SELECT id, body_hash FROM symbols
                     WHERE file_id = ?1 AND parent_id IS ?2 AND name = ?3",
                    params![file_id, parent_id, part],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let (id, hash) =
                row.ok_or_else(|| anyhow!("symbol not found: {rel}:{symbol_path}"))?;
            parent_id = Some(id);
            body_hash = hash;
        }
        self.conn.execute(
            "INSERT OR REPLACE INTO annotations(key, body_hash, doc) VALUES (?1, ?2, ?3)",
            params![format!("{rel}:{symbol_path}"), body_hash, doc],
        )?;
        // Make the note findable via full-text search alongside the docstring.
        let sym_id = parent_id.unwrap();
        let docstring: Option<String> = self.conn.query_row(
            "SELECT doc FROM symbols WHERE id = ?1",
            params![sym_id],
            |r| r.get(0),
        )?;
        let combined = match docstring {
            Some(d) => format!("{d} {doc}"),
            None => doc.to_string(),
        };
        self.conn.execute(
            "UPDATE symbols_fts SET doc = ?1 WHERE rowid = ?2",
            params![combined, sym_id],
        )?;
        Ok(())
    }

    /// All indexed file paths under an optional dir prefix.
    pub fn files_under(&self, dir_prefix: Option<&str>) -> Result<Vec<String>> {
        let like = match dir_prefix {
            Some(d) if !d.is_empty() => format!("{}/%", d.trim_end_matches('/')),
            _ => "%".to_string(),
        };
        Ok(self
            .conn
            .prepare("SELECT path FROM files WHERE path LIKE ?1 ORDER BY path")?
            .query_map(params![like], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }
}

fn upsert_file(
    tx: &rusqlite::Transaction,
    rel: &str,
    lang: &str,
    parsed: &ParsedFile,
    hash: &str,
    mtime: i64,
    size: i64,
) -> Result<()> {
    // Full replace: cascade wipes symbols/imports/refs; annotations live in
    // their own table keyed by path+symbol and survive. FTS rows are not
    // FK-aware — clear them explicitly before the cascade removes symbols.
    tx.execute(
        "DELETE FROM symbols_fts WHERE rowid IN
           (SELECT s.id FROM symbols s JOIN files f ON f.id = s.file_id WHERE f.path = ?1)",
        params![rel],
    )?;
    tx.execute("DELETE FROM files WHERE path = ?1", params![rel])?;
    tx.execute(
        "INSERT INTO files(path, lang, loc, hash, mtime, size) VALUES (?1,?2,?3,?4,?5,?6)",
        params![rel, lang, parsed.loc, hash, mtime, size],
    )?;
    let file_id = tx.last_insert_rowid();

    for sym in &parsed.symbols {
        insert_symbol(tx, file_id, None, sym, rel)?;
    }
    let mut imp = tx.prepare_cached("INSERT INTO imports(file_id, target) VALUES (?1, ?2)")?;
    for target in &parsed.imports {
        imp.execute(params![file_id, target])?;
    }
    let mut rf = tx
        .prepare_cached("INSERT INTO refs(file_id, name, line, kind) VALUES (?1, ?2, ?3, ?4)")?;
    for r in &parsed.refs {
        rf.execute(params![file_id, r.name, r.line, r.kind.as_str()])?;
    }
    Ok(())
}

fn insert_symbol(
    tx: &rusqlite::Transaction,
    file_id: i64,
    parent_id: Option<i64>,
    sym: &ParsedSymbol,
    rel: &str,
) -> Result<()> {
    let mut stmt = tx.prepare_cached(
        "INSERT INTO symbols(file_id, parent_id, name, kind, signature, start_line, end_line, exported, body_hash, doc)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
    )?;
    stmt.execute(params![
        file_id,
        parent_id,
        sym.name,
        sym.kind.as_str(),
        sym.signature,
        sym.start_line,
        sym.end_line,
        sym.exported,
        sym.body_hash,
        sym.doc
    ])?;
    let id = tx.last_insert_rowid();
    // FTS default tokenizer treats `MemberPasswordGenerator` as one token —
    // index the split words alongside the original so `search password` hits.
    let name_tokens = format!("{} {}", sym.name, split_ident(&sym.name));
    tx.prepare_cached(
        "INSERT INTO symbols_fts(rowid, name, signature, doc, path) VALUES (?1,?2,?3,?4,?5)",
    )?
    .execute(params![
        id,
        name_tokens,
        sym.signature,
        sym.doc.as_deref().unwrap_or(""),
        rel
    ])?;
    for ch in &sym.children {
        insert_symbol(tx, file_id, Some(id), ch, rel)?;
    }
    Ok(())
}

/// `MemberPasswordGenerator` / `get_user_id` -> "member password generator" /
/// "get user id".
fn split_ident(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 8);
    let mut prev_lower = false;
    for ch in name.chars() {
        if ch == '_' || ch == '-' {
            out.push(' ');
            prev_lower = false;
        } else if ch.is_uppercase() {
            if prev_lower {
                out.push(' ');
            }
            out.extend(ch.to_lowercase());
            prev_lower = false;
        } else {
            out.push(ch);
            prev_lower = ch.is_lowercase();
        }
    }
    out
}

/// Resolve import targets to indexed files. JS/TS `./relative`, Python
/// dotted/relative modules, Rust `crate::` paths, PHP relative paths and
/// PSR-4 namespaces (composer.json). Go module paths need go.mod context
/// and stay unresolved for now.
fn resolve_imports(tx: &rusqlite::Transaction, root: &Path) -> Result<()> {
    let files: HashMap<String, i64> = tx
        .prepare("SELECT path, id FROM files")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;

    let psr4 = std::fs::read_to_string(root.join("composer.json"))
        .map(|text| parse_psr4(&text))
        .unwrap_or_default();
    let go_module = std::fs::read_to_string(root.join("go.mod"))
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|l| l.trim().strip_prefix("module ").map(|m| m.trim().to_string()))
        });

    let rows: Vec<(i64, String, String, String)> = tx
        .prepare(
            "SELECT i.rowid, f.path, i.target, f.lang FROM imports i
             JOIN files f ON f.id = i.file_id",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;

    for (rowid, from, target, lang) in rows {
        let resolved = match lang.as_str() {
            "py" => resolve_python(&files, &from, &target),
            "rs" => resolve_rust(&files, &target),
            "php" => resolve_php(&files, &from, &target, &psr4),
            "go" => go_module
                .as_deref()
                .and_then(|m| resolve_go(&files, m, &target)),
            "java" | "kt" => resolve_jvm(&files, &target, &lang),
            "rb" => resolve_ruby(&files, &from, &target),
            _ if target.starts_with('.') => resolve_relative(&files, &from, &target),
            _ => None,
        };
        tx.execute(
            "UPDATE imports SET resolved_file_id = ?1 WHERE rowid = ?2",
            params![resolved, rowid],
        )?;
    }
    Ok(())
}

fn resolve_relative(files: &HashMap<String, i64>, from: &str, target: &str) -> Option<i64> {
    let base = Path::new(from).parent().unwrap_or(Path::new(""));
    let joined = normalize(&base.join(target));
    let joined = joined.to_string_lossy().replace('\\', "/");
    let mut candidates = vec![joined.clone()];
    for ext in SUPPORTED_EXTS {
        candidates.push(format!("{joined}.{ext}"));
        candidates.push(format!("{joined}/index.{ext}"));
    }
    candidates.iter().find_map(|c| files.get(c).copied())
}

fn resolve_python(files: &HashMap<String, i64>, from: &str, target: &str) -> Option<i64> {
    let lookup = |module_path: &str| -> Option<i64> {
        if module_path.is_empty() {
            return None;
        }
        files
            .get(&format!("{module_path}.py"))
            .or_else(|| files.get(&format!("{module_path}/__init__.py")))
            .copied()
    };
    let dots = target.chars().take_while(|c| *c == '.').count();
    if dots > 0 {
        // from .foo / ..foo import x — walk up (dots-1) from the file's package
        let rest = &target[dots..];
        let mut base = Path::new(from).parent().unwrap_or(Path::new(""));
        for _ in 1..dots {
            base = base.parent().unwrap_or(Path::new(""));
        }
        let mut p = base.to_string_lossy().replace('\\', "/");
        if !rest.is_empty() {
            if !p.is_empty() {
                p.push('/');
            }
            p.push_str(&rest.replace('.', "/"));
        }
        lookup(&p)
    } else {
        // absolute module: try repo root and common source roots
        let as_path = target.replace('.', "/");
        ["", "src/", "lib/"]
            .iter()
            .find_map(|prefix| lookup(&format!("{prefix}{as_path}")))
    }
}

/// `require '../lib/foo.php'` / `require './foo.php'` resolve as relative
/// file paths. `use Foo\Bar;` namespace imports resolve through the PSR-4
/// map from composer.json; namespaces outside the map (vendor, classmap,
/// PSR-0) stay unresolved.
fn resolve_php(
    files: &HashMap<String, i64>,
    from: &str,
    target: &str,
    psr4: &[(String, Vec<String>)],
) -> Option<i64> {
    if target.contains('/') || target.ends_with(".php") {
        return resolve_relative(files, from, target);
    }
    let fqn = target.trim_start_matches('\\');
    for (prefix, dirs) in psr4 {
        let Some(rest) = fqn.strip_prefix(prefix.as_str()) else {
            continue;
        };
        let rel = rest.replace('\\', "/");
        for dir in dirs {
            let dir = dir.trim_start_matches("./").trim_end_matches('/');
            let candidate = if dir.is_empty() {
                format!("{rel}.php")
            } else {
                format!("{dir}/{rel}.php")
            };
            if let Some(id) = files.get(&candidate) {
                return Some(*id);
            }
        }
    }
    None
}

/// PSR-4 prefix → source dirs from composer.json `autoload` and
/// `autoload-dev`. Prefixes are normalized to end with `\` and sorted
/// longest-first so the most specific namespace wins.
fn parse_psr4(composer_json: &str) -> Vec<(String, Vec<String>)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(composer_json) else {
        return Vec::new();
    };
    let mut map: Vec<(String, Vec<String>)> = Vec::new();
    for section in ["autoload", "autoload-dev"] {
        let Some(psr4) = v
            .get(section)
            .and_then(|s| s.get("psr-4"))
            .and_then(|p| p.as_object())
        else {
            continue;
        };
        for (prefix, dirs) in psr4 {
            let dirs: Vec<String> = match dirs {
                serde_json::Value::String(s) => vec![s.clone()],
                serde_json::Value::Array(a) => a
                    .iter()
                    .filter_map(|d| d.as_str().map(str::to_string))
                    .collect(),
                _ => continue,
            };
            let mut prefix = prefix.trim_start_matches('\\').to_string();
            if !prefix.is_empty() && !prefix.ends_with('\\') {
                prefix.push('\\');
            }
            map.push((prefix, dirs));
        }
    }
    map.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    map
}

/// Go imports name a package (= directory) under the module path from
/// go.mod. A directory has many files but resolved_file_id is single, so
/// pick the file matching the package name (`auth/auth.go`), else the
/// first .go file in the directory — enough for used-by navigation.
fn resolve_go(files: &HashMap<String, i64>, module: &str, target: &str) -> Option<i64> {
    let dir = if target == module {
        ""
    } else {
        target.strip_prefix(module)?.strip_prefix('/')?
    };
    if dir.is_empty() {
        // Root package: prefer the file named after the module's last
        // segment, skipping a /vN major-version suffix (chi/v5 → chi.go).
        let seg = module.rsplit('/').find(|s| {
            !(s.len() > 1 && s.starts_with('v') && s[1..].chars().all(|c| c.is_ascii_digit()))
        });
        if let Some(id) = seg.and_then(|s| files.get(&format!("{s}.go"))) {
            return Some(*id);
        }
    } else {
        let base = dir.rsplit('/').next().unwrap_or(dir);
        if let Some(id) = files.get(&format!("{dir}/{base}.go")) {
            return Some(*id);
        }
    }
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    files
        .iter()
        .filter(|(p, _)| {
            p.strip_prefix(prefix.as_str())
                .is_some_and(|rest| !rest.contains('/') && rest.ends_with(".go"))
        })
        .min_by_key(|(p, _)| p.as_str())
        .map(|(_, id)| *id)
}

/// `import a.b.C` (Java) / `import a.b.C` (Kotlin) → a/b/C.java under
/// common source roots. Wildcard/package imports resolve to nothing (a
/// package is a directory, not a file).
fn resolve_jvm(files: &HashMap<String, i64>, target: &str, lang: &str) -> Option<i64> {
    let as_path = target.replace('.', "/");
    const ROOTS: &[&str] = &[
        "",
        "src/main/java/",
        "src/main/kotlin/",
        "src/",
        "app/src/main/java/",
        "app/src/main/kotlin/",
        "src/test/java/",
        "src/test/kotlin/",
    ];
    let exts: &[&str] = if lang == "kt" {
        &["kt", "java"]
    } else {
        &["java", "kt"]
    };
    for root in ROOTS {
        for ext in exts {
            if let Some(id) = files.get(&format!("{root}{as_path}.{ext}")) {
                return Some(*id);
            }
        }
    }
    None
}

/// `require_relative 'x'` resolves against the file's directory;
/// `require 'x'` against the conventional lib/ load path, then the root.
fn resolve_ruby(files: &HashMap<String, i64>, from: &str, target: &str) -> Option<i64> {
    resolve_relative(files, from, target)
        .or_else(|| files.get(&format!("lib/{target}.rb")).copied())
        .or_else(|| files.get(&format!("{target}.rb")).copied())
}

fn resolve_rust(files: &HashMap<String, i64>, target: &str) -> Option<i64> {
    let path = target.strip_prefix("crate::")?;
    // use crate::a::b::Item — Item may be a symbol, not a module; try both depths
    let segs: Vec<&str> = path.split("::").collect();
    let mut candidates = Vec::new();
    for depth in (1..=segs.len()).rev() {
        let p = segs[..depth].join("/");
        candidates.push(format!("src/{p}.rs"));
        candidates.push(format!("src/{p}/mod.rs"));
    }
    candidates.iter().find_map(|c| files.get(c).copied())
}

/// Minified or generated code: no navigation value, expensive to parse.
fn is_generated(source: &str, size: i64) -> bool {
    if size > 2_000_000 {
        return true;
    }
    source.lines().take(20).any(|l| l.len() > 2500)
}

/// Lexical path normalization (resolves `.` and `..` without touching the fs).
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn psr4_map_and_lookup() {
        let composer = r#"{
            "autoload": {"psr-4": {
                "App\\": "src/",
                "App\\Legacy\\": ["legacy/", "compat/"]
            }},
            "autoload-dev": {"psr-4": {"App\\Tests\\": "tests"}}
        }"#;
        let psr4 = parse_psr4(composer);
        // longest prefix first: App\Tests\ and App\Legacy\ before App\
        assert_eq!(psr4[0].0, "App\\Legacy\\");
        assert_eq!(psr4.last().unwrap().0, "App\\");

        let mut files = HashMap::new();
        files.insert("src/Service/Mailer.php".to_string(), 1_i64);
        files.insert("compat/Old.php".to_string(), 2_i64);
        files.insert("tests/MailerTest.php".to_string(), 3_i64);

        let r = |t: &str| resolve_php(&files, "src/App.php", t, &psr4);
        assert_eq!(r("App\\Service\\Mailer"), Some(1));
        assert_eq!(r("\\App\\Service\\Mailer"), Some(1)); // fully qualified
        assert_eq!(r("App\\Legacy\\Old"), Some(2)); // second dir of the array
        assert_eq!(r("App\\Tests\\MailerTest"), Some(3)); // autoload-dev
        assert_eq!(r("Vendor\\Pkg\\Thing"), None); // outside the map
    }

    #[test]
    fn go_module_lookup() {
        let mut files = HashMap::new();
        files.insert("internal/auth/auth.go".to_string(), 1_i64);
        files.insert("internal/auth/token.go".to_string(), 2_i64);
        files.insert("pkg/util/helpers.go".to_string(), 3_i64);
        files.insert("main.go".to_string(), 4_i64);

        let m = "github.com/acme/app";
        // package-name file preferred over alphabetical order
        assert_eq!(resolve_go(&files, m, "github.com/acme/app/internal/auth"), Some(1));
        // no <dir>/<base>.go — first .go in the directory
        assert_eq!(resolve_go(&files, m, "github.com/acme/app/pkg/util"), Some(3));
        // module root import — no app.go, falls back to first root .go
        assert_eq!(resolve_go(&files, m, "github.com/acme/app"), Some(4));
        // foreign module untouched
        assert_eq!(resolve_go(&files, m, "github.com/other/dep/x"), None);

        // /vN major-version suffix: chi/v5 root import → chi.go
        files.insert("chi.go".to_string(), 5_i64);
        let mv = "github.com/go-chi/chi/v5";
        assert_eq!(resolve_go(&files, mv, "github.com/go-chi/chi/v5"), Some(5));
    }

    #[test]
    fn jvm_import_lookup() {
        let mut files = HashMap::new();
        files.insert("src/main/java/com/acme/Auth.java".to_string(), 1_i64);
        files.insert("src/main/kotlin/com/acme/Token.kt".to_string(), 2_i64);
        files.insert("com/acme/Flat.java".to_string(), 3_i64);

        assert_eq!(resolve_jvm(&files, "com.acme.Auth", "java"), Some(1));
        // Kotlin importing a Java class and vice versa
        assert_eq!(resolve_jvm(&files, "com.acme.Token", "java"), Some(2));
        assert_eq!(resolve_jvm(&files, "com.acme.Auth", "kt"), Some(1));
        // repo-root source layout
        assert_eq!(resolve_jvm(&files, "com.acme.Flat", "java"), Some(3));
        // stdlib / external packages stay unresolved
        assert_eq!(resolve_jvm(&files, "java.util.List", "java"), None);
    }
}
