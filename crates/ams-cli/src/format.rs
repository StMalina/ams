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
            "{:<w$}  {:>5} loc  api:{:<3} used-by:{:<3}",
            e.path,
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
    }
    out
}

pub fn refs(hits: &[RefHit], name: &str) -> String {
    if hits.is_empty() {
        return format!("no usages of '{name}'\n");
    }
    let mut out = String::new();
    let mut cur: Option<&str> = None;
    let mut lines: Vec<String> = Vec::new();
    for h in hits {
        if cur != Some(h.path.as_str()) {
            if let Some(p) = cur {
                out.push_str(&format!("{p}: {}\n", lines.join(", ")));
                lines.clear();
            }
            cur = Some(&h.path);
        }
        lines.push(h.line.to_string());
    }
    if let Some(p) = cur {
        out.push_str(&format!("{p}: {}\n", lines.join(", ")));
    }
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
        out.push_str(&format!("  used-by: {}\n", info.used_by.join(", ")));
    }
    if info.internal_deps.is_empty() && info.external_deps.is_empty() && info.used_by.is_empty() {
        out.push_str("  no known relations\n");
    }
    out
}
