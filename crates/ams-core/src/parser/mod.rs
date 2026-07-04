use crate::model::ParsedFile;
use anyhow::Result;
use tree_sitter::Node;

pub mod go;
pub mod python;
pub mod rust;
pub mod typescript;

pub trait LangParser: Send + Sync {
    fn lang_id(&self) -> &'static str;
    fn parse(&self, source: &str) -> Result<ParsedFile>;
}

pub const SUPPORTED_EXTS: &[&str] = &[
    "ts", "tsx", "js", "jsx", "mjs", "cjs", "rs", "py", "go",
];

pub fn parser_for_ext(ext: &str) -> Option<&'static dyn LangParser> {
    static TS: typescript::TypeScriptParser = typescript::TypeScriptParser { tsx: false };
    static TSX: typescript::TypeScriptParser = typescript::TypeScriptParser { tsx: true };
    static RS: rust::RustParser = rust::RustParser;
    static PY: python::PythonParser = python::PythonParser;
    static GO: go::GoParser = go::GoParser;
    Some(match ext {
        "ts" => &TS,
        "tsx" | "js" | "jsx" | "mjs" | "cjs" => &TSX,
        "rs" => &RS,
        "py" => &PY,
        "go" => &GO,
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
