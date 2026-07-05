use super::{body_hash, collapse, count_loc, line_span, node_text, preceding_doc, signature, LangParser};
use crate::model::{ParsedFile, ParsedSymbol, RefKind, RefOccurrence, SymbolKind};
use anyhow::{Context, Result};
use tree_sitter::Node;

pub struct JavaParser;

impl LangParser for JavaParser {
    fn lang_id(&self) -> &'static str {
        "java"
    }

    fn parse(&self, source: &str) -> Result<ParsedFile> {
        let mut parser = tree_sitter::Parser::new();
        let lang: tree_sitter::Language = tree_sitter_java::LANGUAGE.into();
        parser.set_language(&lang)?;
        let tree = parser
            .parse(source, None)
            .context("tree-sitter failed to parse")?;
        let root = tree.root_node();

        let mut out = ParsedFile {
            loc: count_loc(source),
            ..Default::default()
        };

        let mut cursor = root.walk();
        for child in root.named_children(&mut cursor) {
            collect_top(child, source, &mut out);
        }

        collect_refs(root, source, &mut out.refs);
        super::collect_value_refs(root, source, &mut out.refs);
        Ok(out)
    }
}

fn collect_top(node: Node, src: &str, out: &mut ParsedFile) {
    match node.kind() {
        "import_declaration" => collect_import(node, src, out),
        // Records are structurally classes (fields + accessor-like methods).
        "class_declaration" | "record_declaration" => {
            if let Some(sym) = type_symbol(node, src, SymbolKind::Class) {
                out.symbols.push(sym);
            }
        }
        // Annotation types are, at the bytecode level, a flavor of interface.
        "interface_declaration" | "annotation_type_declaration" => {
            if let Some(sym) = type_symbol(node, src, SymbolKind::Interface) {
                out.symbols.push(sym);
            }
        }
        "enum_declaration" => {
            if let Some(sym) = type_symbol(node, src, SymbolKind::Enum) {
                out.symbols.push(sym);
            }
        }
        _ => {}
    }
}

/// `import a.b.C;` -> "a.b.C"; `import static a.b.C.d;` -> "a.b.C" (member
/// dropped); `import a.b.*;` -> "a.b" (the scoped_identifier already excludes
/// the trailing `.*`, which parses as a separate `asterisk` sibling node).
fn collect_import(node: Node, src: &str, out: &mut ParsedFile) {
    let mut cursor = node.walk();
    let is_static = node.children(&mut cursor).any(|c| c.kind() == "static");

    let mut cursor2 = node.walk();
    let path_node = node
        .named_children(&mut cursor2)
        .find(|c| matches!(c.kind(), "scoped_identifier" | "identifier"));
    let Some(path_node) = path_node else {
        return;
    };
    let mut path = node_text(src, path_node).to_string();
    if is_static {
        if let Some(idx) = path.rfind('.') {
            path.truncate(idx);
        }
    }
    out.imports.push(path);
}

/// A top-level type declaration (class/interface/enum/record/@interface):
/// builds the symbol itself plus method/constructor/const-field children.
fn type_symbol(node: Node, src: &str, kind: SymbolKind) -> Option<ParsedSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let body = node.child_by_field_name("body");
    let (start, end) = line_span(node);
    let mut sym = ParsedSymbol {
        name: node_text(src, name_node).to_string(),
        kind,
        signature: signature(src, node, body),
        start_line: start,
        end_line: end,
        exported: has_modifier(node, src, "public"),
        body_hash: body_hash(src, node),
        doc: preceding_doc(node, src),
        children: vec![],
    };

    if let Some(body) = body {
        let mut cursor = body.walk();
        for member in body.named_children(&mut cursor) {
            match member.kind() {
                "method_declaration" | "constructor_declaration" => {
                    if let Some(m) = simple_symbol(member, src, SymbolKind::Method) {
                        sym.children.push(m);
                    }
                }
                "field_declaration" => {
                    if has_modifier(member, src, "public")
                        && has_modifier(member, src, "static")
                        && has_modifier(member, src, "final")
                    {
                        collect_const_fields(member, src, &mut sym.children);
                    }
                }
                _ => {}
            }
        }
    }
    Some(sym)
}

/// Method / constructor declarations are always recorded as non-exported —
/// per spec, top-level type export status is what matters for indexing.
fn simple_symbol(node: Node, src: &str, kind: SymbolKind) -> Option<ParsedSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let body = node.child_by_field_name("body");
    let (start, end) = line_span(node);
    Some(ParsedSymbol {
        name: node_text(src, name_node).to_string(),
        kind,
        signature: signature(src, node, body),
        start_line: start,
        end_line: end,
        exported: false,
        body_hash: body_hash(src, node),
        doc: preceding_doc(node, src),
        children: vec![],
    })
}

/// `public static final int MAX = 5, OTHER = 6;` -> one Const per declarator.
fn collect_const_fields(field: Node, src: &str, out: &mut Vec<ParsedSymbol>) {
    let mut mcursor = field.walk();
    let modifiers_text = field
        .named_children(&mut mcursor)
        .find(|c| c.kind() == "modifiers")
        .map(|m| node_text(src, m))
        .unwrap_or("");
    let type_text = field
        .child_by_field_name("type")
        .map(|t| node_text(src, t))
        .unwrap_or("");

    let (start, end) = line_span(field);
    let doc = preceding_doc(field, src);
    let mut cursor = field.walk();
    for decl in field.named_children(&mut cursor) {
        if decl.kind() != "variable_declarator" {
            continue;
        }
        let Some(name_node) = decl.child_by_field_name("name") else {
            continue;
        };
        let sig = collapse(&format!(
            "{modifiers_text} {type_text} {}",
            node_text(src, name_node)
        ));
        out.push(ParsedSymbol {
            name: node_text(src, name_node).to_string(),
            kind: SymbolKind::Const,
            signature: sig,
            start_line: start,
            end_line: end,
            exported: true,
            body_hash: body_hash(src, decl),
            doc: doc.clone(),
            children: vec![],
        });
    }
}

/// Whether `node` carries a `modifiers` child whose text contains `kw` as a
/// standalone word (Java modifier keywords are anonymous tokens, so this has
/// to be a text check rather than a child-kind check).
fn has_modifier(node: Node, src: &str, kw: &str) -> bool {
    let mut cursor = node.walk();
    let found = node.named_children(&mut cursor).any(|c| {
        c.kind() == "modifiers" && node_text(src, c).split_whitespace().any(|w| w == kw)
    });
    found
}

fn collect_refs(root: Node, src: &str, refs: &mut Vec<RefOccurrence>) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "method_invocation" => {
                if let Some(n) = node.child_by_field_name("name") {
                    refs.push(RefOccurrence {
                        name: node_text(src, n).to_string(),
                        line: n.start_position().row as u32 + 1,
                        kind: RefKind::Call,
                    });
                }
            }
            "object_creation_expression" => {
                if let Some(t) = node.child_by_field_name("type") {
                    let name_node = last_type_identifier(t);
                    refs.push(RefOccurrence {
                        name: node_text(src, name_node).to_string(),
                        line: name_node.start_position().row as u32 + 1,
                        kind: RefKind::Call,
                    });
                }
            }
            _ => {}
        }
        let mut cursor = node.walk();
        for ch in node.named_children(&mut cursor) {
            stack.push(ch);
        }
    }
}

/// `new Foo()` -> `type_identifier` directly; `new Foo<Bar>()` -> unwrap the
/// `generic_type`; `new Outer.Inner()` -> take the rightmost segment of the
/// `scoped_type_identifier`.
fn last_type_identifier(t: Node) -> Node {
    if t.kind() == "type_identifier" {
        return t;
    }
    let mut cursor = t.walk();
    let mut found = t;
    for ch in t.named_children(&mut cursor) {
        if matches!(
            ch.kind(),
            "type_identifier" | "generic_type" | "scoped_type_identifier"
        ) {
            found = last_type_identifier(ch);
        }
    }
    found
}
