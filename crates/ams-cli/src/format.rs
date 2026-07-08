use ams_core::model::*;

pub fn describe(d: &FileDescription, exported_only: bool) -> String {
    let mut out = String::new();
    out.push_str(&format!("{} [{} loc, {}", d.path, d.loc, d.lang));
    if !d.used_by.is_empty() {
        out.push_str(&format!(", used-by:{}", d.used_by.len()));
    }
    out.push_str("]\n");
    for s in &d.symbols {
        push_symbol(&mut out, s, 1, exported_only);
    }
    if !d.imports.is_empty() {
        out.push_str(&format!("  imports: {}\n", d.imports.join(", ")));
    }
    if !d.used_by.is_empty() {
        out.push_str(&format!("  used-by: {}\n", d.used_by.join(", ")));
    }
    out
}

fn push_symbol(out: &mut String, s: &SymbolInfo, depth: usize, exported_only: bool) {
    if exported_only && !s.exported && depth == 1 {
        return;
    }
    let indent = "  ".repeat(depth);
    out.push_str(&format!(
        "{}{}  @{}-{}{}\n",
        indent,
        s.signature,
        s.start_line,
        s.end_line,
        if s.exported { " exported" } else { "" }
    ));
    // doc* = from the source (docstring); doc = out-of-band annotate note
    if let Some(d) = &s.docstring {
        out.push_str(&format!("{}  doc*: {}\n", indent, d));
    }
    if let Some(doc) = &s.doc {
        out.push_str(&format!(
            "{}  doc: {}{}\n",
            indent,
            doc,
            if s.doc_stale { " [stale]" } else { "" }
        ));
    }
    for ch in &s.children {
        push_symbol(out, ch, depth + 1, exported_only);
    }
}

pub fn tree(entries: &[TreeEntry]) -> String {
    if entries.is_empty() {
        return "no indexed files\n".to_string();
    }
    let w = entries.iter().map(|e| e.path.len()).max().unwrap_or(0);
    let mut out = String::new();
    for e in entries {
        out.push_str(&format!(
            "{:<w$}  {:>4} {:>5} loc  api:{:<3} used-by:{:<3}",
            e.path,
            e.lang,
            e.loc,
            e.api_count,
            e.used_by_count,
            w = w
        ));
        if !e.deps.is_empty() {
            out.push_str(&format!("  {}", e.deps.join(",")));
        }
        out.push('\n');
    }
    out
}

pub fn find(hits: &[FindHit], pattern: &str) -> String {
    if hits.is_empty() {
        return format!("no symbols matching '{pattern}'\n");
    }
    let mut out = String::new();
    for h in hits {
        out.push_str(&format!(
            "{}:{}-{}  [{}] {}{}\n  {}\n",
            h.path,
            h.start_line,
            h.end_line,
            h.kind.as_str(),
            h.symbol_path,
            if h.exported { " exported" } else { "" },
            h.signature
        ));
        if let Some(d) = &h.doc {
            out.push_str(&format!("  doc*: {d}\n"));
        }
    }
    out
}

#[derive(serde::Serialize)]
pub struct DirRollup {
    pub dir: String,
    pub files: u32,
    pub loc: u32,
    pub api: u32,
    pub langs: Vec<String>,
    pub hub: Option<(String, u32)>,
}

/// Aggregate per-file entries into directory groups at `depth` path
/// components below `prefix`.
pub fn rollup(entries: &[TreeEntry], prefix: Option<&str>, depth: usize) -> Vec<DirRollup> {
    use std::collections::BTreeMap;
    let strip = prefix.map(|p| format!("{}/", p.trim_end_matches('/')));
    let mut groups: BTreeMap<String, Vec<&TreeEntry>> = BTreeMap::new();
    for e in entries {
        let rel = match &strip {
            Some(s) => e.path.strip_prefix(s.as_str()).unwrap_or(&e.path),
            None => &e.path,
        };
        let comps: Vec<&str> = rel.split('/').collect();
        let key = if comps.len() > depth {
            comps[..depth].join("/") + "/"
        } else {
            // file sits above the rollup depth — list it by itself
            rel.to_string()
        };
        groups.entry(key).or_default().push(e);
    }
    groups
        .into_iter()
        .map(|(dir, es)| {
            let mut langs: Vec<String> = es.iter().map(|e| e.lang.clone()).collect();
            langs.sort();
            langs.dedup();
            let hub = es
                .iter()
                .max_by_key(|e| e.used_by_count)
                .filter(|e| e.used_by_count > 0)
                .map(|e| (e.path.clone(), e.used_by_count));
            DirRollup {
                dir,
                files: es.len() as u32,
                loc: es.iter().map(|e| e.loc).sum(),
                api: es.iter().map(|e| e.api_count).sum(),
                langs,
                hub,
            }
        })
        .collect()
}

pub fn tree_rollup(entries: &[TreeEntry], prefix: Option<&str>, depth: usize) -> String {
    let rolled = rollup(entries, prefix, depth);
    let w = rolled.iter().map(|r| r.dir.len()).max().unwrap_or(0);
    let mut out = String::new();
    for r in &rolled {
        out.push_str(&format!(
            "{:<w$}  {:>4} files  {:>7} loc  api:{:<5} {}",
            r.dir,
            r.files,
            r.loc,
            r.api,
            r.langs.join(","),
            w = w
        ));
        if let Some((hub, n)) = &r.hub {
            out.push_str(&format!("  hub: {hub} (used-by {n})"));
        }
        out.push('\n');
    }
    out
}

pub fn refs(hits: &[RefHit], name: &str) -> String {
    if hits.is_empty() {
        return format!(
            "no indexed usages of '{name}' (calls or value refs); \
             strings/dynamic dispatch are not indexed — try a text grep\n"
        );
    }
    let file_count = {
        let mut paths: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
        paths.dedup();
        paths.len()
    };
    // Common names (get, run, init...) explode into hundreds of lines of
    // line numbers; collapse to per-file counts and point at --in.
    if file_count > 20 {
        let total = if hits.len() >= 500 {
            "500+".to_string()
        } else {
            hits.len().to_string()
        };
        let mut out = format!(
            "{total} usages in {file_count} files — common name; narrow with \
             `ams refs {name} --in <dir>` or a more specific symbol\n"
        );
        let mut cur: Option<&str> = None;
        let mut n = 0usize;
        let mut shown = 0usize;
        let flush = |out: &mut String, p: &str, n: usize, shown: &mut usize| {
            if *shown < 25 {
                out.push_str(&format!("{p}: {n} refs\n"));
            }
            *shown += 1;
        };
        for h in hits {
            if cur != Some(h.path.as_str()) {
                if let Some(p) = cur {
                    flush(&mut out, p, n, &mut shown);
                }
                cur = Some(&h.path);
                n = 0;
            }
            n += 1;
        }
        if let Some(p) = cur {
            flush(&mut out, p, n, &mut shown);
        }
        if shown > 25 {
            out.push_str(&format!("… and {} more files\n", shown - 25));
        }
        return out;
    }
    let mut out = String::new();
    let mut cur: Option<&str> = None;
    let mut calls: Vec<String> = Vec::new();
    let mut values: Vec<String> = Vec::new();
    let flush = |out: &mut String, p: &str, calls: &mut Vec<String>, values: &mut Vec<String>| {
        let mut parts = Vec::new();
        if !calls.is_empty() {
            parts.push(format!("calls {}", calls.join(", ")));
        }
        if !values.is_empty() {
            parts.push(format!("value {}", values.join(", ")));
        }
        out.push_str(&format!("{p}: {}\n", parts.join(" | ")));
        calls.clear();
        values.clear();
    };
    for h in hits {
        if cur != Some(h.path.as_str()) {
            if let Some(p) = cur {
                flush(&mut out, p, &mut calls, &mut values);
            }
            cur = Some(&h.path);
        }
        match h.kind {
            RefKind::Call => calls.push(h.line.to_string()),
            RefKind::Value => values.push(h.line.to_string()),
        }
    }
    if let Some(p) = cur {
        flush(&mut out, p, &mut calls, &mut values);
    }
    out
}

fn human_bytes(b: i64) -> String {
    if b < 1024 {
        format!("{b} B")
    } else if b < 1024 * 1024 {
        format!("{:.1} KB", b as f64 / 1024.0)
    } else {
        format!("{:.1} MB", b as f64 / (1024.0 * 1024.0))
    }
}

pub fn gain(rows: &[GainRow]) -> String {
    if rows.is_empty() {
        return "no queries recorded yet — describe/tree/find/refs/search/related \
                log output size vs covered source here\n"
            .to_string();
    }
    let mut out = format!(
        "{:<10} {:>6} {:>10} {:>12} {:>7}\n",
        "cmd", "calls", "output", "source", "ratio"
    );
    let (mut calls, mut output, mut source) = (0i64, 0i64, 0i64);
    for r in rows {
        let ratio = if r.output_bytes > 0 {
            format!("{:.0}x", r.source_bytes as f64 / r.output_bytes as f64)
        } else {
            "-".to_string()
        };
        out.push_str(&format!(
            "{:<10} {:>6} {:>10} {:>12} {:>7}\n",
            r.cmd,
            r.calls,
            human_bytes(r.output_bytes),
            human_bytes(r.source_bytes),
            ratio,
        ));
        calls += r.calls;
        output += r.output_bytes;
        source += r.source_bytes;
    }
    let ratio = if output > 0 {
        format!("{:.0}x", source as f64 / output as f64)
    } else {
        "-".to_string()
    };
    out.push_str(&format!(
        "{:<10} {:>6} {:>10} {:>12} {:>7}\n",
        "total",
        calls,
        human_bytes(output),
        human_bytes(source),
        ratio,
    ));
    out.push_str(
        "source = files covered by answers (what a full read would cost); \
         output = what ams actually printed\n",
    );
    out
}

pub fn misses(rows: &[MissRow]) -> String {
    if rows.is_empty() {
        return "no coverage misses recorded yet — no agent has fallen back to \
                grep for an unindexed symbol, and no file parsed to zero \
                symbols\n"
            .to_string();
    }
    let mut out =
        String::from("coverage misses — where ams didn't serve what an agent wanted:\n");
    for r in rows {
        match r.kind.as_str() {
            // symbol: agent grepped this identifier, `ams find` was empty, yet
            // it exists in code text — a real indexing gap.
            "symbol" => out.push_str(&format!(
                "  symbol  {:<32} {:>3}x  grep fell back; not indexed\n",
                r.token, r.count
            )),
            // parse: a non-trivial file ams indexed with no symbols at all.
            "parse" => out.push_str(&format!(
                "  parse   {:<32} {:>3}x  {} — parsed to zero symbols\n",
                r.token,
                r.count,
                r.detail.as_deref().unwrap_or(""),
            )),
            other => out.push_str(&format!("  {other}  {}  {}x\n", r.token, r.count)),
        }
    }
    out.push_str(
        "symbol = an identifier agents grep for that ams can't find but that \
         lives in the code; parse = a file ams indexed structure-free. Both are \
         gaps worth closing in the parser/resolver.\n",
    );
    out
}

pub fn related(info: &RelatedInfo) -> String {
    let mut out = format!("{}\n", info.path);
    if !info.internal_deps.is_empty() {
        out.push_str(&format!("  deps internal: {}\n", info.internal_deps.join(", ")));
    }
    if !info.external_deps.is_empty() {
        out.push_str(&format!("  deps external: {}\n", info.external_deps.join(", ")));
    }
    if !info.used_by.is_empty() {
        // Hub files can have 100+ reverse deps; cap the list, keep the count.
        if info.used_by.len() > 30 {
            out.push_str(&format!(
                "  used-by ({} files): {}, … and {} more (--json for all)\n",
                info.used_by.len(),
                info.used_by[..30].join(", "),
                info.used_by.len() - 30,
            ));
        } else {
            out.push_str(&format!("  used-by: {}\n", info.used_by.join(", ")));
        }
    }
    for lvl in &info.impact {
        let mut dirs: Vec<String> = lvl
            .dirs
            .iter()
            .take(15)
            .map(|(d, n)| format!("{d} ({n})"))
            .collect();
        if lvl.dirs.len() > 15 {
            dirs.push(format!("… {} more dirs", lvl.dirs.len() - 15));
        }
        out.push_str(&format!(
            "  impact level {}: {} files — {}\n",
            lvl.level,
            lvl.total,
            dirs.join(", ")
        ));
    }
    if info.internal_deps.is_empty() && info.external_deps.is_empty() && info.used_by.is_empty() {
        out.push_str("  no known relations\n");
    }
    out
}

pub fn cycles(cycles: &[Vec<String>]) -> String {
    if cycles.is_empty() {
        return "no dependency cycles\n".to_string();
    }
    let mut out = format!(
        "{} dependency cycle{}:\n",
        cycles.len(),
        if cycles.len() == 1 { "" } else { "s" }
    );
    for c in cycles {
        out.push_str(&format!("  ({} files) {}\n", c.len(), c.join(" <-> ")));
    }
    out
}
