use crate::model::{ParsedFile, RefKind, RefOccurrence};
use anyhow::Result;
use std::collections::HashMap;
use tree_sitter::Node;

pub mod go;
pub mod php;
pub mod python;
pub mod rust;
pub mod typescript;

pub trait LangParser: Send + Sync {
    fn lang_id(&self) -> &'static str;
    fn parse(&self, source: &str) -> Result<ParsedFile>;
}

pub const SUPPORTED_EXTS: &[&str] = &[
    "ts", "tsx", "js", "jsx", "mjs", "cjs", "rs", "py", "go", "php",
];

pub fn parser_for_ext(ext: &str) -> Option<&'static dyn LangParser> {
    static TS: typescript::TypeScriptParser = typescript::TypeScriptParser { tsx: false };
    static TSX: typescript::TypeScriptParser = typescript::TypeScriptParser { tsx: true };
    static RS: rust::RustParser = rust::RustParser;
    static PY: python::PythonParser = python::PythonParser;
    static GO: go::GoParser = go::GoParser;
    static PHP: php::PhpParser = php::PhpParser;
    Some(match ext {
        "ts" => &TS,
        "tsx" | "js" | "jsx" | "mjs" | "cjs" => &TSX,
        "rs" => &RS,
        "py" => &PY,
        "go" => &GO,
        "php" => &PHP,
        _ => return None,
    })
}

// ---- shared helpers for language parsers ----

pub(crate) fn node_text<'a>(src: &'a str, node: Node) -> &'a str {
    &src[node.byte_range()]
}

/// 1-based inclusive line span.
pub(crate) fn line_span(node: Node) -> (u32, u32) {
    (
        node.start_position().row as u32 + 1,
        node.end_position().row as u32 + 1,
    )
}

pub(crate) fn body_hash(src: &str, node: Node) -> String {
    blake3::hash(node_text(src, node).as_bytes())
        .to_hex()
        .as_str()[..16]
        .to_string()
}

/// Signature: node text up to the body (exclusive), whitespace-collapsed.
pub(crate) fn signature(src: &str, node: Node, body: Option<Node>) -> String {
    let end = body.map(|b| b.start_byte()).unwrap_or(node.end_byte());
    let raw = &src[node.start_byte()..end];
    collapse(raw)
}

pub(crate) fn collapse(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(160));
    let mut last_ws = false;
    for ch in raw.chars() {
        if ch.is_whitespace() {
            if !last_ws && !out.is_empty() {
                out.push(' ');
            }
            last_ws = true;
        } else {
            out.push(ch);
            last_ws = false;
        }
    }
    let trimmed = out.trim_end().to_string();
    if trimmed.chars().count() > 160 {
        let mut s: String = trimmed.chars().take(157).collect();
        s.push_str("...");
        s
    } else {
        trimmed
    }
}

pub(crate) fn count_loc(source: &str) -> u32 {
    source.lines().count() as u32
}

/// Strip surrounding quotes from a string-literal node text.
pub(crate) fn unquote(s: &str) -> String {
    s.trim_matches(|c| c == '"' || c == '\'' || c == '`').to_string()
}

/// First meaningful line of the doc comment directly above a symbol
/// (`///`, `//!`, `/** */`, `//`). Skips tag lines (`@param`), TODO/FIXME,
/// and linter pragmas. Python docstrings are handled in the Python parser.
pub(crate) fn preceding_doc(node: Node, src: &str) -> Option<String> {
    let mut anchor = node;
    while let Some(p) = anchor.parent() {
        if matches!(p.kind(), "export_statement" | "decorated_definition") {
            anchor = p;
        } else {
            break;
        }
    }
    let mut comments: Vec<Node> = Vec::new();
    let mut cur = anchor;
    for _ in 0..20 {
        let Some(sib) = cur.prev_named_sibling() else {
            break;
        };
        if !sib.kind().contains("comment") {
            break;
        }
        // adjacency: no blank-line gap between the comment and the symbol
        if cur.start_position().row.saturating_sub(sib.end_position().row) > 1 {
            break;
        }
        comments.push(sib);
        cur = sib;
    }
    for c in comments.iter().rev() {
        for line in node_text(src, *c).lines() {
            let t = line
                .trim()
                .trim_start_matches("/**")
                .trim_start_matches("/*!")
                .trim_start_matches("/*")
                .trim_start_matches("//!")
                .trim_start_matches("///")
                .trim_start_matches("//")
                .trim_start_matches('*')
                .trim_end_matches("*/")
                .trim();
            if t.is_empty()
                || t.starts_with('@')
                || t.starts_with("TODO")
                || t.starts_with("FIXME")
                || t.contains("eslint-")
                || t.contains("prettier-")
                || t.starts_with("#[")
            {
                continue;
            }
            return Some(cap_line(t));
        }
    }
    None
}

pub(crate) fn cap_line(t: &str) -> String {
    if t.chars().count() > 120 {
        t.chars().take(117).collect::<String>() + "..."
    } else {
        t.to_string()
    }
}

/// Identifiers read as values (`route(handler)`, `map(parse)`) — catches the
/// references that call-position tracking misses, e.g. router registrations.
/// Language-agnostic heuristic over field names / parent kinds; capped at 5
/// occurrences per name per file to bound noise from locals.
pub(crate) fn collect_value_refs(root: Node, src: &str, refs: &mut Vec<RefOccurrence>) {
    // Fields where the identifier is a definition/target, not a use.
    const DENY_FIELD: &[&str] = &[
        "name", "property", "key", "alias", "label", "field", "attribute",
        "pattern", "left", "function", "constructor", "parameter", "index",
    ];
    // Parents where identifiers are imports, parameters, or declarations.
    const DENY_PARENT: &[&str] = &[
        "import_specifier", "import_clause", "namespace_import", "import_statement",
        "import_from_statement", "dotted_name", "aliased_import",
        "use_declaration", "scoped_use_list", "use_as_clause", "use_list",
        "use_wildcard", "import_spec", "import_declaration",
        "formal_parameters", "required_parameter", "optional_parameter",
        "parameters", "lambda_parameters", "default_parameter", "typed_parameter",
        "typed_default_parameter", "parameter_list", "parameter_declaration",
        "variadic_parameter_declaration", "self_parameter", "parameter",
    ];

    let mut counts: HashMap<String, u32> = HashMap::new();
    let mut cursor = root.walk();
    'outer: loop {
        let node = cursor.node();
        if node.kind() == "identifier" {
            let denied = cursor
                .field_name()
                .map_or(false, |f| DENY_FIELD.contains(&f))
                || node
                    .parent()
                    .map_or(true, |p| DENY_PARENT.contains(&p.kind()));
            if !denied {
                let name = node_text(src, node);
                if name.len() >= 3 {
                    let c = counts.entry(name.to_string()).or_insert(0);
                    if *c < 5 {
                        *c += 1;
                        refs.push(RefOccurrence {
                            name: name.to_string(),
                            line: node.start_position().row as u32 + 1,
                            kind: RefKind::Value,
                        });
                    }
                }
            }
        }
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                continue 'outer;
            }
            if !cursor.goto_parent() {
                break 'outer;
            }
        }
    }
}
