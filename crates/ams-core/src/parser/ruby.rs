use super::{body_hash, cap_line, collapse, count_loc, line_span, node_text, unquote, LangParser};
use crate::model::{ParsedFile, ParsedSymbol, RefKind, RefOccurrence, SymbolKind};
use anyhow::{Context, Result};
use std::collections::HashMap;
use tree_sitter::Node;

pub struct RubyParser;

impl LangParser for RubyParser {
    fn lang_id(&self) -> &'static str {
        "rb"
    }

    fn parse(&self, source: &str) -> Result<ParsedFile> {
        let mut parser = tree_sitter::Parser::new();
        let lang: tree_sitter::Language = tree_sitter_ruby::LANGUAGE.into();
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
        // The generic `collect_value_refs` helper (identifiers passed as
        // values) is skipped for Ruby: bare, receiverless, arg-less method
        // calls (`helper`, `private`, attr reads) parse to the very same
        // `identifier` node kind as local-variable reads, so the heuristic
        // would flag most local variables in the file as "value refs" noise.
        // Instead we run a Ruby-specific pass below that only picks up
        // `:symbol` literals passed positionally as call arguments — the
        // idiomatic Ruby way to reference a method by name (callback/hook
        // registration: `before_action :require_login`, `attr_accessor
        // :name`), which is cheap and low-noise since symbols are rarely
        // used for anything else in argument position.
        collect_symbol_value_refs(root, source, &mut out.refs);
        Ok(out)
    }
}

fn collect_top(node: Node, src: &str, out: &mut ParsedFile) {
    match node.kind() {
        "call" => collect_top_level_require(node, src, out),
        "method" => {
            if let Some(sym) = def_symbol(node, src, SymbolKind::Function, true) {
                out.symbols.push(sym);
            }
        }
        // `def self.x` at the top level of a file is rare (it defines a
        // singleton method on `main`); treat it like any other top-level
        // def rather than leaving it symbol-less.
        "singleton_method" => {
            if let Some(sym) = def_symbol(node, src, SymbolKind::Method, true) {
                out.symbols.push(sym);
            }
        }
        "class" => {
            if let Some(sym) = container_symbol(node, src, SymbolKind::Class) {
                out.symbols.push(sym);
            }
        }
        "module" => {
            if let Some(sym) = container_symbol(node, src, SymbolKind::Module) {
                out.symbols.push(sym);
            }
        }
        _ => {}
    }
}

/// `require 'x'` / `require_relative '../y'` as a bare top-level call (no
/// `expression_statement` wrapper in this grammar — see `to_sexp()`).
/// Only top-level calls are treated as imports; the same call form nested
/// inside a method body is just a regular call captured by `collect_refs`.
fn collect_top_level_require(node: Node, src: &str, out: &mut ParsedFile) {
    let Some(method) = node.child_by_field_name("method") else {
        return;
    };
    if method.kind() != "identifier" {
        return;
    }
    let name = node_text(src, method);
    if name != "require" && name != "require_relative" {
        return;
    }
    let Some(args) = node.child_by_field_name("arguments") else {
        return;
    };
    let Some(arg) = args.named_child(0) else {
        return;
    };
    if arg.kind() == "string" {
        out.imports.push(unquote(node_text(src, arg)));
    }
}

/// A `class`/`module` node plus its recursively-collected children: nested
/// `def`/`def self.x` become `Method` children (exported=false — see note
/// below), nested `class`/`module` recurse as further container children.
fn container_symbol(node: Node, src: &str, kind: SymbolKind) -> Option<ParsedSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let (start, end) = line_span(node);
    let mut sym = ParsedSymbol {
        name: node_text(src, name_node).to_string(),
        kind,
        signature: first_line(src, node),
        start_line: start,
        end_line: end,
        // Classes/modules are always exported=true, whether top-level or
        // nested inside another class/module — Ruby has no notion of a
        // "private class". Only `def`s inside a body drop to false.
        exported: true,
        body_hash: body_hash(src, node),
        doc: ruby_doc(node, src),
        children: vec![],
    };
    if let Some(body) = node.child_by_field_name("body") {
        collect_body(body, src, &mut sym.children);
    }
    Some(sym)
}

/// Members of a class/module `body_statement`. Ruby's `private`/`public`
/// bare-word calls (parsed as plain `identifier` nodes here — see
/// `to_sexp()`) are intentionally not tracked: every method child is
/// already emitted with `exported: false`, which is consistent regardless
/// of whether the source also called `private`.
fn collect_body(body: Node, src: &str, children: &mut Vec<ParsedSymbol>) {
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        match member.kind() {
            "method" | "singleton_method" => {
                if let Some(sym) = def_symbol(member, src, SymbolKind::Method, false) {
                    children.push(sym);
                }
            }
            "class" => {
                if let Some(sym) = container_symbol(member, src, SymbolKind::Class) {
                    children.push(sym);
                }
            }
            "module" => {
                if let Some(sym) = container_symbol(member, src, SymbolKind::Module) {
                    children.push(sym);
                }
            }
            _ => {}
        }
    }
}

fn def_symbol(node: Node, src: &str, kind: SymbolKind, exported: bool) -> Option<ParsedSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let (start, end) = line_span(node);
    Some(ParsedSymbol {
        name: node_text(src, name_node).to_string(),
        kind,
        signature: first_line(src, node),
        start_line: start,
        end_line: end,
        exported,
        body_hash: body_hash(src, node),
        doc: ruby_doc(node, src),
        children: vec![],
    })
}

/// First physical line of the node's own text, whitespace-collapsed. The
/// shared `signature()` helper (node text up to the body's start byte)
/// grabs too much for Ruby: `def`/`class`/`module` bodies are a single
/// `body_statement` field, but any doc comment sitting between the
/// class/module head and its body (see `ruby_doc`) lands *before* that
/// field too, so `signature()` would swallow the comment. A def's parameter
/// list can also span multiple lines; taking just the first line is a
/// deliberate simplification.
fn first_line(src: &str, node: Node) -> String {
    let end = node.end_byte().min(src.len());
    let text = &src[node.start_byte()..end];
    let line = text.split('\n').next().unwrap_or("");
    collapse(line)
}

/// Doc comment directly above a `def`/`class`/`module`. Ruby's grammar
/// attaches a comment preceding the *first* statement of a body as a child
/// of the enclosing node itself (positioned just before the `body` field),
/// not as a sibling inside `body_statement` — confirmed via `to_sexp()`:
/// `(class name: ... (comment) body: (body_statement (method ...)))`. So a
/// comment documenting the first member of a class/module doesn't show up
/// as that member's `prev_named_sibling`; we climb through `body_statement`
/// in that case and look one level up instead.
fn ruby_doc(node: Node, src: &str) -> Option<String> {
    let mut anchor = node;
    loop {
        if anchor.prev_named_sibling().is_some() {
            break;
        }
        let parent = anchor.parent()?;
        let is_first_in_body = parent.kind() == "body_statement"
            && parent.named_child(0).map(|c| c.id()) == Some(anchor.id());
        if is_first_in_body {
            anchor = parent;
            continue;
        }
        return None;
    }

    let mut comments: Vec<Node> = Vec::new();
    let mut cur = anchor;
    for _ in 0..20 {
        let Some(sib) = cur.prev_named_sibling() else {
            break;
        };
        if sib.kind() != "comment" {
            break;
        }
        if cur.start_position().row.saturating_sub(sib.end_position().row) > 1 {
            break;
        }
        comments.push(sib);
        cur = sib;
    }
    for c in comments.iter().rev() {
        let t = node_text(src, *c).trim_start_matches('#').trim();
        if t.is_empty()
            || t.starts_with("!/") // shebang: #!/usr/bin/env ruby
            || t.starts_with("frozen_string_literal")
            || t.starts_with("rubocop")
            || t.starts_with("encoding:")
            || t.starts_with("coding:")
        {
            continue;
        }
        return Some(cap_line(t));
    }
    None
}

/// Method-call names, both receiverless (`foo(...)`) and with a receiver
/// (`obj.foo`, `Klass.new`, `obj&.foo`) — this grammar folds all of those
/// into a single `call` node with a `method` field, so one node kind covers
/// them. Bare, arg-less, receiverless invocations (`helper` with no parens)
/// are indistinguishable from local-variable reads at the grammar level
/// (they parse as plain `identifier`) and are not captured here.
fn collect_refs(root: Node, src: &str, refs: &mut Vec<RefOccurrence>) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "call" {
            if let Some(m) = node.child_by_field_name("method") {
                if m.kind() == "identifier" {
                    refs.push(RefOccurrence {
                        name: node_text(src, m).to_string(),
                        line: m.start_position().row as u32 + 1,
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

/// `:symbol` literals passed positionally as call arguments (`argument_list`
/// direct children only — not e.g. `render json: :ok`'s `pair` values, which
/// are usually config, not method names). Capped at 5 occurrences per name
/// per file, mirroring the other languages' value-ref passes.
fn collect_symbol_value_refs(root: Node, src: &str, refs: &mut Vec<RefOccurrence>) {
    let mut counts: HashMap<String, u32> = HashMap::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "simple_symbol"
            && node.parent().is_some_and(|p| p.kind() == "argument_list")
        {
            let name = node_text(src, node).trim_start_matches(':');
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
