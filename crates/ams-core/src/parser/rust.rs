use super::{body_hash, collapse, count_loc, line_span, node_text, signature, LangParser};
use crate::model::{ParsedFile, ParsedSymbol, RefKind, RefOccurrence, SymbolKind};
use anyhow::{Context, Result};
use tree_sitter::Node;

pub struct RustParser;

impl LangParser for RustParser {
    fn lang_id(&self) -> &'static str {
        "rs"
    }

    fn parse(&self, source: &str) -> Result<ParsedFile> {
        let mut parser = tree_sitter::Parser::new();
        let lang: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
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

fn has_pub(node: Node) -> bool {
    let mut cursor = node.walk();
    let found = node
        .named_children(&mut cursor)
        .any(|c| c.kind() == "visibility_modifier");
    found
}

fn collect_top(node: Node, src: &str, out: &mut ParsedFile) {
    match node.kind() {
        "use_declaration" => {
            if let Some(arg) = node.child_by_field_name("argument") {
                out.imports.push(collapse(node_text(src, arg)));
            }
        }
        "function_item" => {
            if let Some(sym) = simple_symbol(node, src, SymbolKind::Function) {
                out.symbols.push(sym);
            }
        }
        "struct_item" => {
            if let Some(sym) = simple_symbol(node, src, SymbolKind::Struct) {
                out.symbols.push(sym);
            }
        }
        "enum_item" => {
            if let Some(sym) = simple_symbol(node, src, SymbolKind::Enum) {
                out.symbols.push(sym);
            }
        }
        "trait_item" => {
            if let Some(mut sym) = simple_symbol(node, src, SymbolKind::Trait) {
                if let Some(body) = node.child_by_field_name("body") {
                    let mut cursor = body.walk();
                    for member in body.named_children(&mut cursor) {
                        if matches!(member.kind(), "function_item" | "function_signature_item") {
                            if let Some(mut m) = simple_symbol(member, src, SymbolKind::Method) {
                                m.exported = false;
                                sym.children.push(m);
                            }
                        }
                    }
                }
                out.symbols.push(sym);
            }
        }
        "impl_item" => {
            if let Some(sym) = build_impl(node, src) {
                out.symbols.push(sym);
            }
        }
        "const_item" | "static_item" => {
            if let Some(sym) = simple_symbol(node, src, SymbolKind::Const) {
                out.symbols.push(sym);
            }
        }
        "type_item" => {
            if let Some(sym) = simple_symbol(node, src, SymbolKind::TypeAlias) {
                out.symbols.push(sym);
            }
        }
        "mod_item" => {
            if let Some(body) = node.child_by_field_name("body") {
                if let Some(mut sym) = simple_symbol(node, src, SymbolKind::Module) {
                    let mut inner = ParsedFile::default();
                    let mut cursor = body.walk();
                    for ch in body.named_children(&mut cursor) {
                        collect_top(ch, src, &mut inner);
                    }
                    sym.children = inner.symbols;
                    out.imports.extend(inner.imports);
                    out.symbols.push(sym);
                }
            }
        }
        _ => {}
    }
}

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
        exported: has_pub(node),
        body_hash: body_hash(src, node),
        children: vec![],
    })
}

fn build_impl(node: Node, src: &str) -> Option<ParsedSymbol> {
    let ty = node.child_by_field_name("type")?;
    let name = if let Some(tr) = node.child_by_field_name("trait") {
        format!(
            "{} for {}",
            collapse(node_text(src, tr)),
            collapse(node_text(src, ty))
        )
    } else {
        collapse(node_text(src, ty))
    };
    let body = node.child_by_field_name("body");
    let (start, end) = line_span(node);

    let mut children = Vec::new();
    if let Some(b) = body {
        let mut cursor = b.walk();
        for member in b.named_children(&mut cursor) {
            if member.kind() == "function_item" {
                if let Some(m) = simple_symbol(member, src, SymbolKind::Method) {
                    children.push(m);
                }
            }
        }
    }

    Some(ParsedSymbol {
        name,
        kind: SymbolKind::Class,
        signature: signature(src, node, body),
        start_line: start,
        end_line: end,
        exported: false,
        body_hash: body_hash(src, node),
        children,
    })
}

fn collect_refs(root: Node, src: &str, refs: &mut Vec<RefOccurrence>) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "call_expression" => {
                if let Some(f) = node.child_by_field_name("function") {
                    if let Some((n, name)) = call_target(f, src) {
                        refs.push(RefOccurrence {
                            name,
                            line: n.start_position().row as u32 + 1,
                            kind: RefKind::Call,
                        });
                    }
                }
            }
            "macro_invocation" => {
                if let Some(m) = node.child_by_field_name("macro") {
                    let name_node = match m.kind() {
                        "identifier" => Some(m),
                        "scoped_identifier" => m.child_by_field_name("name"),
                        _ => None,
                    };
                    if let Some(n) = name_node {
                        refs.push(RefOccurrence {
                            name: node_text(src, n).to_string(),
                            line: n.start_position().row as u32 + 1,
                            kind: RefKind::Call,
                        });
                    }
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

fn call_target<'a>(f: Node<'a>, src: &str) -> Option<(Node<'a>, String)> {
    match f.kind() {
        "identifier" => Some((f, node_text(src, f).to_string())),
        "scoped_identifier" => {
            let name = f.child_by_field_name("name")?;
            Some((name, node_text(src, name).to_string()))
        }
        "field_expression" => {
            let field = f.child_by_field_name("field")?;
            Some((field, node_text(src, field).to_string()))
        }
        _ => None,
    }
}
