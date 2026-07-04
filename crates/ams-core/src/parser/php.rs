use super::{body_hash, collapse, count_loc, line_span, node_text, signature, unquote, LangParser};
use crate::model::{ParsedFile, ParsedSymbol, RefKind, RefOccurrence, SymbolKind};
use anyhow::{Context, Result};
use std::collections::HashMap;
use tree_sitter::Node;

pub struct PhpParser;

impl LangParser for PhpParser {
    fn lang_id(&self) -> &'static str {
        "php"
    }

    fn parse(&self, source: &str) -> Result<ParsedFile> {
        let mut parser = tree_sitter::Parser::new();
        // Embedded-HTML variant: legacy PHP is routinely interleaved with HTML,
        // and this grammar still exposes top-level statements as direct
        // children of `program` alongside `text`/`php_tag` nodes.
        let lang: tree_sitter::Language = tree_sitter_php::LANGUAGE_PHP.into();
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
        // `namespace Foo\Bar { ... }` — recurse and flatten into the file;
        // `namespace Foo\Bar;` (no body) applies to the rest of the file, so
        // its siblings are visited normally by the caller's loop.
        "namespace_definition" => {
            if let Some(body) = node.child_by_field_name("body") {
                let mut cursor = body.walk();
                for ch in body.named_children(&mut cursor) {
                    collect_top(ch, src, out);
                }
            }
        }
        "namespace_use_declaration" => collect_use_declaration(node, src, out),
        "expression_statement" => {
            if let Some(expr) = node.named_child(0) {
                collect_include(expr, src, out);
            }
        }
        // Legacy procedural PHP declares functions inside if/foreach/try
        // blocks (registered globally at runtime) — descend into control
        // flow so those files don't come out symbol-less.
        "if_statement" | "else_clause" | "else_if_clause" | "switch_statement"
        | "switch_block" | "case_statement" | "default_statement" | "while_statement"
        | "for_statement" | "foreach_statement" | "try_statement" | "catch_clause"
        | "finally_clause" | "compound_statement" | "declare_statement" => {
            let mut cursor = node.walk();
            for ch in node.named_children(&mut cursor) {
                collect_top(ch, src, out);
            }
        }
        "function_definition" => {
            if let Some(sym) = simple_symbol(node, src, SymbolKind::Function, true) {
                out.symbols.push(sym);
            }
        }
        "class_declaration" => {
            if let Some(mut sym) = simple_symbol(node, src, SymbolKind::Class, true) {
                if let Some(body) = node.child_by_field_name("body") {
                    let mut cursor = body.walk();
                    for member in body.named_children(&mut cursor) {
                        if member.kind() == "method_declaration" {
                            let exported = !is_private_or_protected(member, src);
                            if let Some(m) =
                                simple_symbol(member, src, SymbolKind::Method, exported)
                            {
                                sym.children.push(m);
                            }
                        }
                        // property_declaration / const_declaration inside a
                        // class are intentionally skipped.
                    }
                }
                out.symbols.push(sym);
            }
        }
        "interface_declaration" => {
            if let Some(sym) = simple_symbol(node, src, SymbolKind::Interface, true) {
                out.symbols.push(sym);
            }
        }
        "trait_declaration" => {
            if let Some(sym) = simple_symbol(node, src, SymbolKind::Trait, true) {
                out.symbols.push(sym);
            }
        }
        "enum_declaration" => {
            if let Some(sym) = simple_symbol(node, src, SymbolKind::Enum, true) {
                out.symbols.push(sym);
            }
        }
        // Top-level `const NAME = value;` (possibly multiple names per stmt).
        "const_declaration" => {
            let mut cursor = node.walk();
            for el in node.named_children(&mut cursor) {
                if el.kind() != "const_element" {
                    continue;
                }
                let mut c2 = el.walk();
                let Some(name_node) = el.named_children(&mut c2).find(|c| c.kind() == "name")
                else {
                    continue;
                };
                let (start, end) = line_span(el);
                out.symbols.push(ParsedSymbol {
                    name: node_text(src, name_node).to_string(),
                    kind: SymbolKind::Const,
                    signature: format!("const {}", collapse(node_text(src, el))),
                    start_line: start,
                    end_line: end,
                    exported: true,
                    body_hash: body_hash(src, el),
                    children: vec![],
                });
            }
        }
        _ => {}
    }
}

/// `use Foo\Bar;` and grouped `use Foo\{Bar, Baz as B};`. Aliases are dropped
/// from the recorded path — only the imported symbol path is kept.
fn collect_use_declaration(node: Node, src: &str, out: &mut ParsedFile) {
    if let Some(body) = node.child_by_field_name("body") {
        // Grouped form: a `namespace_name` prefix child plus a
        // `namespace_use_group` body of clauses.
        let mut cursor = node.walk();
        let base = node
            .named_children(&mut cursor)
            .find(|c| c.kind() == "namespace_name")
            .map(|c| node_text(src, c).to_string());
        let mut gcursor = body.walk();
        for clause in body.named_children(&mut gcursor) {
            if clause.kind() != "namespace_use_clause" {
                continue;
            }
            if let Some(path) = use_clause_path(clause, src) {
                let full = match &base {
                    Some(b) => format!("{b}\\{path}"),
                    None => path,
                };
                out.imports.push(full);
            }
        }
    } else {
        let mut cursor = node.walk();
        for clause in node.named_children(&mut cursor) {
            if clause.kind() != "namespace_use_clause" {
                continue;
            }
            if let Some(path) = use_clause_path(clause, src) {
                out.imports.push(path);
            }
        }
    }
}

fn use_clause_path(clause: Node, src: &str) -> Option<String> {
    let alias = clause.child_by_field_name("alias");
    let mut cursor = clause.walk();
    for ch in clause.named_children(&mut cursor) {
        if Some(ch) == alias {
            continue;
        }
        if matches!(ch.kind(), "name" | "qualified_name") {
            return Some(node_text(src, ch).to_string());
        }
    }
    None
}

/// `require`/`require_once`/`include`/`include_once` with a literal string
/// argument are treated as file-relative imports.
fn collect_include(expr: Node, src: &str, out: &mut ParsedFile) {
    if !matches!(
        expr.kind(),
        "require_expression" | "require_once_expression" | "include_expression" | "include_once_expression"
    ) {
        return;
    }
    let Some(arg) = expr.named_child(0) else {
        return;
    };
    if arg.kind() == "string" {
        out.imports.push(unquote(node_text(src, arg)));
    }
}

fn is_private_or_protected(node: Node, src: &str) -> bool {
    let mut cursor = node.walk();
    let found = node.named_children(&mut cursor).any(|c| {
        c.kind() == "visibility_modifier" && {
            let t = node_text(src, c);
            t == "private" || t == "protected"
        }
    });
    found
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
            "function_call_expression" => {
                if let Some(f) = node.child_by_field_name("function") {
                    if let Some(n) = call_name_node(f) {
                        refs.push(RefOccurrence {
                            name: node_text(src, n).to_string(),
                            line: n.start_position().row as u32 + 1,
                            kind: RefKind::Call,
                        });
                    }
                }
            }
            "member_call_expression" | "scoped_call_expression" => {
                if let Some(n) = node.child_by_field_name("name") {
                    if n.kind() == "name" {
                        refs.push(RefOccurrence {
                            name: node_text(src, n).to_string(),
                            line: n.start_position().row as u32 + 1,
                            kind: RefKind::Call,
                        });
                    }
                }
            }
            "object_creation_expression" => {
                let mut cursor = node.walk();
                let found = node
                    .named_children(&mut cursor)
                    .find(|c| matches!(c.kind(), "name" | "qualified_name"));
                if let Some(n) = found {
                    let name_node = last_name_segment(n);
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

/// `foo()` -> `foo`; `\Ns\foo()` (qualified_name) -> last segment `foo`.
fn call_name_node(f: Node) -> Option<Node> {
    match f.kind() {
        "name" => Some(f),
        "qualified_name" => Some(last_name_segment(f)),
        _ => None,
    }
}

fn last_name_segment(n: Node) -> Node {
    if n.kind() == "qualified_name" {
        let mut cursor = n.walk();
        if let Some(last) = n.named_children(&mut cursor).last() {
            return last;
        }
    }
    n
}

/// Bare identifiers passed as call arguments — constants and class-name
/// references that plain call-position tracking misses. The generic
/// `collect_value_refs` helper targets grammars whose identifier node kind is
/// `identifier`; PHP's is `name`, so this is a dedicated PHP variant capped
/// at 5 occurrences per name per file.
fn collect_value_refs(root: Node, src: &str, refs: &mut Vec<RefOccurrence>) {
    let mut counts: HashMap<String, u32> = HashMap::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "name" && node.parent().map_or(false, |p| p.kind() == "argument") {
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
        let mut cursor = node.walk();
        for ch in node.named_children(&mut cursor) {
            stack.push(ch);
        }
    }
}
