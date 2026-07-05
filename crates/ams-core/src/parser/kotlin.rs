use super::{
    body_hash, collect_value_refs, count_loc, line_span, node_text, preceding_doc, signature,
    LangParser,
};
use crate::model::{ParsedFile, ParsedSymbol, RefKind, RefOccurrence, SymbolKind};
use anyhow::{Context, Result};
use tree_sitter::Node;

pub struct KotlinParser;

impl LangParser for KotlinParser {
    fn lang_id(&self) -> &'static str {
        "kt"
    }

    fn parse(&self, source: &str) -> Result<ParsedFile> {
        let mut parser = tree_sitter::Parser::new();
        let lang: tree_sitter::Language = tree_sitter_kotlin_ng::LANGUAGE.into();
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
        collect_value_refs(root, source, &mut out.refs);
        Ok(out)
    }
}

fn collect_top(node: Node, src: &str, out: &mut ParsedFile) {
    match node.kind() {
        // `import a.b.C`, `import a.b.C as D`, `import a.b.*` — in every case
        // the `qualified_identifier` child is exactly the dotted path we
        // want; the alias identifier / wildcard `*` are separate siblings.
        "import" => {
            if let Some(target) = import_target(node, src) {
                out.imports.push(target);
            }
        }
        // `class` and `interface` share the same node kind in this grammar;
        // the only difference is the anonymous `interface` keyword token
        // among the (unnamed) children.
        "class_declaration" => {
            let kind = if is_interface(node) {
                SymbolKind::Interface
            } else {
                SymbolKind::Class
            };
            let exported = !is_private_or_internal(node, src);
            if let Some(mut sym) = class_like_symbol(node, src, kind, exported) {
                collect_methods(node, src, &mut sym);
                out.symbols.push(sym);
            }
        }
        "object_declaration" => {
            let exported = !is_private_or_internal(node, src);
            if let Some(mut sym) = class_like_symbol(node, src, SymbolKind::Class, exported) {
                collect_methods(node, src, &mut sym);
                out.symbols.push(sym);
            }
        }
        "function_declaration" => {
            let exported = !is_private_or_internal(node, src);
            if let Some(sym) = function_symbol(node, src, exported) {
                out.symbols.push(sym);
            }
        }
        "property_declaration" => {
            let exported = !is_private_or_internal(node, src);
            if let Some(sym) = property_symbol(node, src, exported) {
                out.symbols.push(sym);
            }
        }
        _ => {}
    }
}

/// `function_declaration`s directly inside a `class_body` become `Method`
/// children (always non-exported, mirroring the TS/PHP parsers). Everything
/// else in a class body — properties, secondary constructors, nested types,
/// and `companion_object` members — is intentionally skipped.
fn collect_methods(node: Node, src: &str, sym: &mut ParsedSymbol) {
    let Some(body) = find_child(node, &["class_body"]) else {
        return;
    };
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        if member.kind() == "function_declaration" {
            if let Some(mut m) = function_symbol(member, src, false) {
                m.kind = SymbolKind::Method;
                sym.children.push(m);
            }
        }
    }
}

fn class_like_symbol(node: Node, src: &str, kind: SymbolKind, exported: bool) -> Option<ParsedSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let body = find_child(node, &["class_body", "enum_class_body"]);
    let (start, end) = line_span(node);
    Some(ParsedSymbol {
        name: node_text(src, name_node).to_string(),
        kind,
        signature: signature(src, node, body),
        start_line: start,
        end_line: end,
        exported,
        body_hash: body_hash(src, node),
        doc: preceding_doc(node, src),
        children: vec![],
    })
}

fn function_symbol(node: Node, src: &str, exported: bool) -> Option<ParsedSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let body = find_child(node, &["function_body"]);
    let (start, end) = line_span(node);
    Some(ParsedSymbol {
        name: node_text(src, name_node).to_string(),
        kind: SymbolKind::Function,
        signature: signature(src, node, body),
        start_line: start,
        end_line: end,
        exported,
        body_hash: body_hash(src, node),
        doc: preceding_doc(node, src),
        children: vec![],
    })
}

/// Top-level `val`/`var` become `Const`. Destructuring (`val (a, b) = ...`)
/// is skipped — it has no single name to anchor a symbol on.
fn property_symbol(node: Node, src: &str, exported: bool) -> Option<ParsedSymbol> {
    let var_decl = find_child(node, &["variable_declaration"])?;
    let name_node = find_child(var_decl, &["identifier"])?;
    let value = property_value(node);
    let (start, end) = line_span(node);
    let sig = signature(src, node, value)
        .trim_end_matches(|c| c == '=' || c == ' ')
        .to_string();
    Some(ParsedSymbol {
        name: node_text(src, name_node).to_string(),
        kind: SymbolKind::Const,
        signature: sig,
        start_line: start,
        end_line: end,
        exported,
        body_hash: body_hash(src, node),
        doc: preceding_doc(node, src),
        children: vec![],
    })
}

/// The initializer expression of a `property_declaration`, if any — it's the
/// first child that isn't one of the declaration's non-value parts (name,
/// type annotation, modifiers, accessors, delegate).
fn property_value(node: Node) -> Option<Node> {
    const NON_VALUE: &[&str] = &[
        "modifiers",
        "variable_declaration",
        "multi_variable_declaration",
        "user_type",
        "nullable_type",
        "parenthesized_type",
        "type_constraints",
        "type_parameters",
        "getter",
        "setter",
        "property_delegate",
    ];
    find_child_excluding(node, NON_VALUE)
}

fn find_child<'a>(node: Node<'a>, kinds: &[&str]) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .named_children(&mut cursor)
        .find(|c| kinds.contains(&c.kind()));
    found
}

fn find_child_excluding<'a>(node: Node<'a>, deny: &[&str]) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let found = node
        .named_children(&mut cursor)
        .find(|c| !deny.contains(&c.kind()));
    found
}

/// `class`/`interface` are keyword tokens, not fields — the interface variant
/// is the only one carrying the anonymous `interface` node among children.
fn is_interface(node: Node) -> bool {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).any(|c| c.kind() == "interface");
    found
}

/// Kotlin defaults to public visibility; only an explicit `private`/`internal`
/// modifier makes a top-level declaration non-exported.
fn is_private_or_internal(node: Node, src: &str) -> bool {
    let Some(mods) = find_child(node, &["modifiers"]) else {
        return false;
    };
    let mut cursor = mods.walk();
    let found = mods.named_children(&mut cursor).any(|c| {
        c.kind() == "visibility_modifier" && matches!(node_text(src, c), "private" | "internal")
    });
    found
}

fn import_target(node: Node, src: &str) -> Option<String> {
    let q = find_child(node, &["qualified_identifier"])?;
    Some(node_text(src, q).to_string())
}

fn collect_refs(root: Node, src: &str, refs: &mut Vec<RefOccurrence>) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "call_expression" {
            if let Some(callee) = call_callee(node) {
                match callee.kind() {
                    "identifier" => {
                        refs.push(RefOccurrence {
                            name: node_text(src, callee).to_string(),
                            line: callee.start_position().row as u32 + 1,
                            kind: RefKind::Call,
                        });
                    }
                    // `obj.method(...)` — the callable is the last segment
                    // of the navigation chain.
                    "navigation_expression" => {
                        if let Some(member) = last_named_child(callee) {
                            if member.kind() == "identifier" {
                                refs.push(RefOccurrence {
                                    name: node_text(src, member).to_string(),
                                    line: member.start_position().row as u32 + 1,
                                    kind: RefKind::Call,
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut cursor = node.walk();
        for ch in node.named_children(&mut cursor) {
            stack.push(ch);
        }
    }
}

/// `call_expression`'s callee isn't fielded in this grammar — it's the first
/// named child that isn't the argument list / type args / trailing lambda.
fn call_callee(node: Node) -> Option<Node> {
    let mut cursor = node.walk();
    let found = node
        .named_children(&mut cursor)
        .find(|c| !matches!(c.kind(), "value_arguments" | "type_arguments" | "annotated_lambda"));
    found
}

fn last_named_child(node: Node) -> Option<Node> {
    let mut cursor = node.walk();
    let found = node.named_children(&mut cursor).last();
    found
}
