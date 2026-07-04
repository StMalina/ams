use super::{body_hash, count_loc, line_span, node_text, signature, unquote, LangParser};
use crate::model::{ParsedFile, ParsedSymbol, RefOccurrence, SymbolKind};
use anyhow::{Context, Result};
use std::collections::HashSet;
use tree_sitter::Node;

pub struct TypeScriptParser {
    pub tsx: bool,
}

impl LangParser for TypeScriptParser {
    fn lang_id(&self) -> &'static str {
        if self.tsx {
            "tsx"
        } else {
            "ts"
        }
    }

    fn parse(&self, source: &str) -> Result<ParsedFile> {
        let mut parser = tree_sitter::Parser::new();
        let lang: tree_sitter::Language = if self.tsx {
            tree_sitter_typescript::LANGUAGE_TSX.into()
        } else {
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
        };
        parser.set_language(&lang)?;
        let tree = parser
            .parse(source, None)
            .context("tree-sitter failed to parse")?;
        let root = tree.root_node();

        let mut out = ParsedFile {
            loc: count_loc(source),
            ..Default::default()
        };
        let mut named_exports: HashSet<String> = HashSet::new();

        let mut cursor = root.walk();
        for child in root.named_children(&mut cursor) {
            collect_top(child, source, false, &mut out, &mut named_exports);
        }

        // `export { a, b }` refers to symbols declared elsewhere in the file.
        for sym in &mut out.symbols {
            if named_exports.contains(&sym.name) {
                sym.exported = true;
            }
        }

        collect_refs(root, source, &mut out.refs);
        Ok(out)
    }
}

fn collect_top(
    node: Node,
    src: &str,
    exported: bool,
    out: &mut ParsedFile,
    named_exports: &mut HashSet<String>,
) {
    match node.kind() {
        "import_statement" => {
            if let Some(source_node) = node.child_by_field_name("source") {
                out.imports.push(unquote(node_text(src, source_node)));
            }
        }
        "export_statement" => {
            if let Some(decl) = node.child_by_field_name("declaration") {
                collect_top(decl, src, true, out, named_exports);
            } else if let Some(value) = node.child_by_field_name("value") {
                // export default <identifier>
                if value.kind() == "identifier" {
                    named_exports.insert(node_text(src, value).to_string());
                }
            } else {
                let mut cursor = node.walk();
                for ch in node.named_children(&mut cursor) {
                    if ch.kind() == "export_clause" {
                        let mut c2 = ch.walk();
                        for spec in ch.named_children(&mut c2) {
                            if let Some(name) = spec.child_by_field_name("name") {
                                named_exports.insert(node_text(src, name).to_string());
                            }
                        }
                    }
                }
            }
        }
        "function_declaration" | "generator_function_declaration" => {
            if let Some(sym) = simple_symbol(node, src, SymbolKind::Function, exported) {
                out.symbols.push(sym);
            }
        }
        "class_declaration" | "abstract_class_declaration" => {
            if let Some(mut sym) = simple_symbol(node, src, SymbolKind::Class, exported) {
                if let Some(body) = node.child_by_field_name("body") {
                    let mut cursor = body.walk();
                    for member in body.named_children(&mut cursor) {
                        if matches!(
                            member.kind(),
                            "method_definition" | "abstract_method_signature"
                        ) {
                            if let Some(m) =
                                simple_symbol(member, src, SymbolKind::Method, false)
                            {
                                sym.children.push(m);
                            }
                        }
                    }
                }
                out.symbols.push(sym);
            }
        }
        "interface_declaration" => {
            if let Some(sym) = simple_symbol(node, src, SymbolKind::Interface, exported) {
                out.symbols.push(sym);
            }
        }
        "type_alias_declaration" => {
            if let Some(sym) = simple_symbol(node, src, SymbolKind::TypeAlias, exported) {
                out.symbols.push(sym);
            }
        }
        "enum_declaration" => {
            if let Some(sym) = simple_symbol(node, src, SymbolKind::Enum, exported) {
                out.symbols.push(sym);
            }
        }
        "lexical_declaration" | "variable_declaration" => {
            let mut cursor = node.walk();
            for decl in node.named_children(&mut cursor) {
                if decl.kind() != "variable_declarator" {
                    continue;
                }
                let Some(name_node) = decl.child_by_field_name("name") else {
                    continue;
                };
                if name_node.kind() != "identifier" {
                    continue; // destructuring patterns
                }
                let name = node_text(src, name_node).to_string();
                let value = decl.child_by_field_name("value");
                let is_fn = value.map_or(false, |v| {
                    matches!(v.kind(), "arrow_function" | "function_expression" | "function")
                });
                let (kind, body) = if is_fn {
                    let v = value.unwrap();
                    (SymbolKind::Function, v.child_by_field_name("body"))
                } else {
                    (SymbolKind::Const, value)
                };
                let (start, end) = line_span(node);
                let sig = signature(src, node, body)
                    .trim_end_matches(|c| c == '=' || c == ' ')
                    .to_string();
                out.symbols.push(ParsedSymbol {
                    name,
                    kind,
                    signature: sig,
                    start_line: start,
                    end_line: end,
                    exported,
                    body_hash: body_hash(src, node),
                    children: vec![],
                });
            }
        }
        // namespace Foo { ... }
        "module" | "internal_module" => {
            if let Some(mut sym) = simple_symbol(node, src, SymbolKind::Module, exported) {
                if let Some(body) = node.child_by_field_name("body") {
                    let mut inner = ParsedFile::default();
                    let mut cursor = body.walk();
                    for ch in body.named_children(&mut cursor) {
                        collect_top(ch, src, exported, &mut inner, named_exports);
                    }
                    sym.children = inner.symbols;
                    out.imports.extend(inner.imports);
                }
                out.symbols.push(sym);
            }
        }
        _ => {}
    }
}

fn simple_symbol(node: Node, src: &str, kind: SymbolKind, exported: bool) -> Option<ParsedSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let body = node.child_by_field_name("body");
    let (start, end) = line_span(node);
    Some(ParsedSymbol {
        name: node_text(src, name_node).to_string(),
        kind,
        signature: signature(src, node, body),
        start_line: start,
        end_line: end,
        exported,
        body_hash: body_hash(src, node),
        children: vec![],
    })
}

fn collect_refs(root: Node, src: &str, refs: &mut Vec<RefOccurrence>) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "call_expression" => {
                if let Some(f) = node.child_by_field_name("function") {
                    let name_node = match f.kind() {
                        "identifier" => Some(f),
                        "member_expression" => f.child_by_field_name("property"),
                        _ => None,
                    };
                    if let Some(n) = name_node {
                        refs.push(RefOccurrence {
                            name: node_text(src, n).to_string(),
                            line: n.start_position().row as u32 + 1,
                        });
                    }
                }
            }
            "new_expression" => {
                if let Some(c) = node.child_by_field_name("constructor") {
                    if c.kind() == "identifier" {
                        refs.push(RefOccurrence {
                            name: node_text(src, c).to_string(),
                            line: c.start_position().row as u32 + 1,
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
