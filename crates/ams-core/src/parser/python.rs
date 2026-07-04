use super::{body_hash, collapse, count_loc, line_span, node_text, signature, LangParser};
use crate::model::{ParsedFile, ParsedSymbol, RefKind, RefOccurrence, SymbolKind};
use anyhow::{Context, Result};
use tree_sitter::Node;

pub struct PythonParser;

impl LangParser for PythonParser {
    fn lang_id(&self) -> &'static str {
        "py"
    }

    fn parse(&self, source: &str) -> Result<ParsedFile> {
        let mut parser = tree_sitter::Parser::new();
        let lang: tree_sitter::Language = tree_sitter_python::LANGUAGE.into();
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

fn is_exported_name(name: &str) -> bool {
    !name.starts_with('_')
}

fn is_const_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_uppercase() || c == '_' || c.is_ascii_digit())
        && name.chars().any(|c| c.is_alphabetic())
}

fn collect_top(node: Node, src: &str, out: &mut ParsedFile) {
    match node.kind() {
        "import_statement" => {
            let mut cursor = node.walk();
            for n in node.children_by_field_name("name", &mut cursor) {
                push_import_name(n, src, out);
            }
        }
        "import_from_statement" => {
            if let Some(m) = node.child_by_field_name("module_name") {
                out.imports.push(collapse(node_text(src, m)));
            }
        }
        "function_definition" => {
            let name = node.child_by_field_name("name");
            let exported = name.map_or(true, |n| is_exported_name(node_text(src, n)));
            if let Some(sym) = simple_symbol(node, node, src, SymbolKind::Function, exported) {
                out.symbols.push(sym);
            }
        }
        "class_definition" => {
            let name = node.child_by_field_name("name");
            let exported = name.map_or(true, |n| is_exported_name(node_text(src, n)));
            if let Some(mut sym) = simple_symbol(node, node, src, SymbolKind::Class, exported) {
                if let Some(body) = node.child_by_field_name("body") {
                    collect_class_body(body, src, &mut sym.children);
                }
                out.symbols.push(sym);
            }
        }
        "decorated_definition" => {
            if let Some(inner) = node.child_by_field_name("definition") {
                match inner.kind() {
                    "function_definition" => {
                        let name = inner.child_by_field_name("name");
                        let exported = name.map_or(true, |n| is_exported_name(node_text(src, n)));
                        if let Some(sym) =
                            simple_symbol(node, inner, src, SymbolKind::Function, exported)
                        {
                            out.symbols.push(sym);
                        }
                    }
                    "class_definition" => {
                        let name = inner.child_by_field_name("name");
                        let exported = name.map_or(true, |n| is_exported_name(node_text(src, n)));
                        if let Some(mut sym) =
                            simple_symbol(node, inner, src, SymbolKind::Class, exported)
                        {
                            if let Some(body) = inner.child_by_field_name("body") {
                                collect_class_body(body, src, &mut sym.children);
                            }
                            out.symbols.push(sym);
                        }
                    }
                    _ => {}
                }
            }
        }
        "expression_statement" => {
            let mut cursor = node.walk();
            for ch in node.named_children(&mut cursor) {
                if ch.kind() == "assignment" {
                    collect_const(node, ch, src, out);
                }
            }
        }
        _ => {}
    }
}

fn collect_const(stmt: Node, assignment: Node, src: &str, out: &mut ParsedFile) {
    let Some(left) = assignment.child_by_field_name("left") else {
        return;
    };
    if left.kind() != "identifier" {
        return;
    }
    let name = node_text(src, left);
    if !is_const_name(name) {
        return;
    }
    let (start, end) = line_span(stmt);
    out.symbols.push(ParsedSymbol {
        name: name.to_string(),
        kind: SymbolKind::Const,
        signature: signature(src, stmt, None),
        start_line: start,
        end_line: end,
        exported: is_exported_name(name),
        body_hash: body_hash(src, stmt),
        children: vec![],
    });
}

fn collect_class_body(body: Node, src: &str, children: &mut Vec<ParsedSymbol>) {
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        match member.kind() {
            "function_definition" => {
                if let Some(sym) = simple_symbol(member, member, src, SymbolKind::Method, false) {
                    children.push(sym);
                }
            }
            "decorated_definition" => {
                if let Some(inner) = member.child_by_field_name("definition") {
                    if inner.kind() == "function_definition" {
                        if let Some(sym) =
                            simple_symbol(member, inner, src, SymbolKind::Method, false)
                        {
                            children.push(sym);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn push_import_name(n: Node, src: &str, out: &mut ParsedFile) {
    match n.kind() {
        "dotted_name" => out.imports.push(collapse(node_text(src, n))),
        "aliased_import" => {
            if let Some(name) = n.child_by_field_name("name") {
                out.imports.push(collapse(node_text(src, name)));
            }
        }
        _ => {}
    }
}

fn simple_symbol(
    outer: Node,
    inner: Node,
    src: &str,
    kind: SymbolKind,
    exported: bool,
) -> Option<ParsedSymbol> {
    let name_node = inner.child_by_field_name("name")?;
    let body = inner.child_by_field_name("body");
    let (start, end) = line_span(outer);
    Some(ParsedSymbol {
        name: node_text(src, name_node).to_string(),
        kind,
        signature: signature(src, outer, body),
        start_line: start,
        end_line: end,
        exported,
        body_hash: body_hash(src, outer),
        children: vec![],
    })
}

fn collect_refs(root: Node, src: &str, refs: &mut Vec<RefOccurrence>) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "call" {
            if let Some(f) = node.child_by_field_name("function") {
                let name_node = match f.kind() {
                    "identifier" => Some(f),
                    "attribute" => f.child_by_field_name("attribute"),
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
