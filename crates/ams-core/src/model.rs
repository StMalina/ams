use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    Const,
    TypeAlias,
    Module,
}

impl SymbolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SymbolKind::Function => "fn",
            SymbolKind::Method => "method",
            SymbolKind::Class => "class",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Trait => "trait",
            SymbolKind::Interface => "interface",
            SymbolKind::Const => "const",
            SymbolKind::TypeAlias => "type",
            SymbolKind::Module => "mod",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "fn" | "function" => SymbolKind::Function,
            "method" => SymbolKind::Method,
            "class" => SymbolKind::Class,
            "struct" => SymbolKind::Struct,
            "enum" => SymbolKind::Enum,
            "trait" => SymbolKind::Trait,
            "interface" => SymbolKind::Interface,
            "const" => SymbolKind::Const,
            "type" => SymbolKind::TypeAlias,
            "mod" | "module" => SymbolKind::Module,
            _ => return None,
        })
    }
}

/// Symbol extracted from a single file by a language parser.
#[derive(Debug, Clone, Serialize)]
pub struct ParsedSymbol {
    pub name: String,
    pub kind: SymbolKind,
    /// One-line signature, normalized whitespace, no body.
    pub signature: String,
    /// 1-based, inclusive.
    pub start_line: u32,
    pub end_line: u32,
    pub exported: bool,
    /// blake3 hex of the full node text; anchors annotations.
    pub body_hash: String,
    /// First meaningful line of the source doc comment / docstring.
    pub doc: Option<String>,
    pub children: Vec<ParsedSymbol>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    /// Direct call or constructor position: `foo()`, `new Foo()`.
    Call,
    /// Identifier passed/read as a value: `route(foo)`, `map(foo)`.
    Value,
}

impl RefKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            RefKind::Call => "call",
            RefKind::Value => "value",
        }
    }
}

/// Identifier observed in call position or as a value reference.
#[derive(Debug, Clone, Serialize)]
pub struct RefOccurrence {
    pub name: String,
    pub line: u32,
    pub kind: RefKind,
}

#[derive(Debug, Default, Serialize)]
pub struct ParsedFile {
    pub imports: Vec<String>,
    pub symbols: Vec<ParsedSymbol>,
    pub refs: Vec<RefOccurrence>,
    pub loc: u32,
}

// ---- Query result types (index -> CLI) ----

#[derive(Debug, Serialize)]
pub struct FileDescription {
    pub path: String,
    pub lang: String,
    pub loc: u32,
    pub symbols: Vec<SymbolInfo>,
    pub imports: Vec<String>,
    pub used_by: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SymbolInfo {
    pub name: String,
    pub kind: SymbolKind,
    pub signature: String,
    pub start_line: u32,
    pub end_line: u32,
    pub exported: bool,
    /// From the source (docstring / doc comment).
    pub docstring: Option<String>,
    /// From `ams annotate` (out-of-band LLM note).
    pub doc: Option<String>,
    pub doc_stale: bool,
    pub children: Vec<SymbolInfo>,
}

#[derive(Debug, Serialize)]
pub struct FindHit {
    pub path: String,
    pub symbol_path: String,
    pub kind: SymbolKind,
    pub signature: String,
    pub start_line: u32,
    pub end_line: u32,
    pub exported: bool,
    pub doc: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RefHit {
    pub path: String,
    pub line: u32,
    pub kind: RefKind,
}

#[derive(Debug, Serialize)]
pub struct TreeEntry {
    pub path: String,
    pub lang: String,
    pub loc: u32,
    pub api_count: u32,
    pub used_by_count: u32,
    pub deps: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RelatedInfo {
    pub path: String,
    pub internal_deps: Vec<String>,
    pub external_deps: Vec<String>,
    pub used_by: Vec<String>,
}

/// Aggregated per-command usage stats for `ams gain`.
#[derive(Debug, Serialize)]
pub struct GainRow {
    pub cmd: String,
    pub calls: i64,
    pub output_bytes: i64,
    pub source_bytes: i64,
}
