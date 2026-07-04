use super::{body_hash, count_loc, line_span, node_text, signature, unquote, LangParser};
use crate::model::{ParsedFile, ParsedSymbol, RefKind, RefOccurrence, SymbolKind};
use anyhow::{Context, Result};
use tree_sitter::Node;

pub struct GoParser;

impl LangParser for GoParser {
    fn lang_id(&self) -> &'static str {
        "go"
    }

    fn parse(&self, source: &str) -> Result<ParsedFile> {
        let mut parser = tree_sitter::Parser::new();
        let lang: tree_sitter::Language = tree_sitter_go::LANGUAGE.into();
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

fn is_exported(name: &str) -> bool {
    name.chars().next().map_or(false, |c| c.is_uppercase())
}

fn collect_top(node: Node, src: &str, out: &mut ParsedFile) {
    match node.kind() {
        "import_declaration" => {
            let mut cursor = node.walk();
            for ch in node.named_children(&mut cursor) {
                match ch.kind() {
                    "import_spec" => push_import_spec(ch, src, out),
                    "import_spec_list" => {
                        let mut c2 = ch.walk();
                        for spec in ch.named_children(&mut c2) {
                            if spec.kind() == "import_spec" {
                                push_import_spec(spec, src, out);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        "function_declaration" => {
            if let Some(sym) = simple_symbol(node, src, SymbolKind::Function) {
                out.symbols.push(sym);
            }
        }
        "method_declaration" => {
            if let Some(sym) = simple_symbol(node, src, SymbolKind::Method) {
                out.symbols.push(sym);
            }
        }
        "type_declaration" => {
            let mut cursor = node.walk();
            for spec in node.named_children(&mut cursor) {
                if spec.kind() == "type_spec" {
                    if let Some(sym) = build_type_spec(spec, src) {
                        out.symbols.push(sym);
                    }
                }
            }
        }
        "const_declaration" => {
            let mut cursor = node.walk();
            for spec in node.named_children(&mut cursor) {
                if spec.kind() == "const_spec" {
                    collect_spec_names(spec, src, out);
                }
            }
        }
        "var_declaration" => {
            let mut cursor = node.walk();
            for ch in node.named_children(&mut cursor) {
                match ch.kind() {
                    "var_spec" => collect_spec_names(ch, src, out),
                    "var_spec_list" => {
                        let mut c2 = ch.walk();
                        for spec in ch.named_children(&mut c2) {
                            if spec.kind() == "var_spec" {
                                collect_spec_names(spec, src, out);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn push_import_spec(spec: Node, src: &str, out: &mut ParsedFile) {
    if let Some(path) = spec.child_by_field_name("path") {
        out.imports.push(unquote(node_text(src, path)));
    }
}

fn collect_spec_names(spec: Node, src: &str, out: &mut ParsedFile) {
    let (start, end) = line_span(spec);
    let mut cursor = spec.walk();
    for name_node in spec.children_by_field_name("name", &mut cursor) {
        if name_node.kind() != "identifier" {
            continue;
        }
        let name = node_text(src, name_node).to_string();
        out.symbols.push(ParsedSymbol {
            exported: is_exported(&name),
            name,
            kind: SymbolKind::Const,
            signature: signature(src, spec, None),
            start_line: start,
            end_line: end,
            body_hash: body_hash(src, spec),
            children: vec![],
        });
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
        exported: is_exported(node_text(src, name_node)),
        body_hash: body_hash(src, node),
        children: vec![],
    })
}

fn build_type_spec(spec: Node, src: &str) -> Option<ParsedSymbol> {
    let name_node = spec.child_by_field_name("name")?;
    let ty = spec.child_by_field_name("type")?;
    let name = node_text(src, name_node).to_string();
    let kind = match ty.kind() {
        "struct_type" => SymbolKind::Struct,
        "interface_type" => SymbolKind::Interface,
        _ => SymbolKind::TypeAlias,
    };
    let body = match ty.kind() {
        "struct_type" | "interface_type" => {
            let mut c = ty.walk();
            let first = ty.named_children(&mut c).next();
            first
        }
        _ => None,
    };
    let (start, end) = line_span(spec);
    let sig = format!("type {}", signature(src, spec, body));
    Some(ParsedSymbol {
        exported: is_exported(&name),
        name,
        kind,
        signature: sig,
        start_line: start,
        end_line: end,
        body_hash: body_hash(src, spec),
        children: vec![],
    })
}

fn collect_refs(root: Node, src: &str, refs: &mut Vec<RefOccurrence>) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "call_expression" {
            if let Some(f) = node.child_by_field_name("function") {
                let name_node = match f.kind() {
                    "identifier" => Some(f),
                    "selector_expression" => f.child_by_field_name("field"),
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
        let mut cursor = node.walk();
        for ch in node.named_children(&mut cursor) {
            stack.push(ch);
        }
    }
}
