use super::{body_hash, cap_line, collapse, collect_value_refs, count_loc, line_span, node_text, signature, LangParser};
use crate::model::{ParsedFile, ParsedSymbol, RefKind, RefOccurrence, SymbolKind};
use anyhow::{Context, Result};
use tree_sitter::Node;

pub struct CSharpParser;

impl LangParser for CSharpParser {
    fn lang_id(&self) -> &'static str {
        "cs"
    }

    fn parse(&self, source: &str) -> Result<ParsedFile> {
        let mut parser = tree_sitter::Parser::new();
        let lang: tree_sitter::Language = tree_sitter_c_sharp::LANGUAGE.into();
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
        "using_directive" => collect_using(node, src, out),
        // `namespace Foo.Bar { ... }` — recurse and flatten into the file.
        "namespace_declaration" => {
            if let Some(body) = node.child_by_field_name("body") {
                let mut cursor = body.walk();
                for ch in body.named_children(&mut cursor) {
                    collect_top(ch, src, out);
                }
            }
        }
        // `namespace Foo.Bar;` (file-scoped, no body) — applies to the rest
        // of the file; its siblings are already visited by the caller's
        // top-level loop, so there is nothing to do here.
        "file_scoped_namespace_declaration" => {}
        "class_declaration" => {
            push_type_symbol(node, src, SymbolKind::Class, out);
        }
        "struct_declaration" => {
            push_type_symbol(node, src, SymbolKind::Struct, out);
        }
        "record_declaration" => {
            push_type_symbol(node, src, SymbolKind::Class, out);
        }
        "interface_declaration" => {
            push_type_symbol(node, src, SymbolKind::Interface, out);
        }
        "enum_declaration" => {
            // Enum members (`enum_member_declaration`) are intentionally not
            // surfaced as child symbols.
            if let Some(sym) = simple_symbol(node, src, SymbolKind::Enum, is_public(node, src)) {
                out.symbols.push(sym);
            }
        }
        _ => {}
    }
}

fn push_type_symbol(node: Node, src: &str, kind: SymbolKind, out: &mut ParsedFile) {
    let Some(mut sym) = simple_symbol(node, src, kind, is_public(node, src)) else {
        return;
    };
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for member in body.named_children(&mut cursor) {
            match member.kind() {
                "method_declaration" | "constructor_declaration" => {
                    if let Some(m) = simple_symbol(member, src, SymbolKind::Method, false) {
                        sym.children.push(m);
                    }
                }
                "property_declaration" => {
                    if let Some(m) = property_symbol(member, src) {
                        sym.children.push(m);
                    }
                }
                "field_declaration" => {
                    if is_const_or_static_readonly(member, src) {
                        sym.children.extend(const_field_symbols(member, src));
                    }
                }
                _ => {}
            }
        }
    }
    out.symbols.push(sym);
}

/// `true` if the declaration carries a `public` modifier.
fn is_public(node: Node, src: &str) -> bool {
    has_modifier(node, src, "public")
}

fn has_modifier(node: Node, src: &str, text: &str) -> bool {
    let mut cursor = node.walk();
    let found = node
        .named_children(&mut cursor)
        .any(|c| c.kind() == "modifier" && node_text(src, c) == text);
    found
}

fn is_const_or_static_readonly(field: Node, src: &str) -> bool {
    let mut cursor = field.walk();
    let mods: Vec<&str> = field
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "modifier")
        .map(|c| node_text(src, c))
        .collect();
    mods.contains(&"const") || (mods.contains(&"static") && mods.contains(&"readonly"))
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
        doc: xml_doc(node, src),
        children: vec![],
    })
}

/// `property_declaration` — the "body" is exposed as the `accessors` field
/// (`{ get; set; }`) or, for expression-bodied properties, `value`; either
/// truncates the signature the same way a block body would.
fn property_symbol(node: Node, src: &str) -> Option<ParsedSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let trunc = node
        .child_by_field_name("accessors")
        .or_else(|| node.child_by_field_name("value"));
    let (start, end) = line_span(node);
    Some(ParsedSymbol {
        name: node_text(src, name_node).to_string(),
        kind: SymbolKind::Method,
        signature: signature(src, node, trunc),
        start_line: start,
        end_line: end,
        exported: false,
        body_hash: body_hash(src, node),
        doc: xml_doc(node, src),
        children: vec![],
    })
}

/// `[public] const int Max = 5;` / `public static readonly int Limit = 10;`
/// — one symbol per declarator (`int A = 1, B = 2;` declares two).
fn const_field_symbols(field: Node, src: &str) -> Vec<ParsedSymbol> {
    let exported = has_modifier(field, src, "public");
    let mut mod_cursor = field.walk();
    let mods: Vec<&str> = field
        .named_children(&mut mod_cursor)
        .filter(|c| c.kind() == "modifier")
        .map(|c| node_text(src, c))
        .collect();
    let mods_text = mods.join(" ");

    let mut cursor = field.walk();
    let Some(var_decl) = field
        .named_children(&mut cursor)
        .find(|c| c.kind() == "variable_declaration")
    else {
        return vec![];
    };
    let type_text = var_decl
        .child_by_field_name("type")
        .map(|t| node_text(src, t))
        .unwrap_or("");

    let (start, end) = line_span(field);
    let doc = xml_doc(field, src);

    let mut out = vec![];
    let mut dcursor = var_decl.walk();
    for decl in var_decl
        .named_children(&mut dcursor)
        .filter(|c| c.kind() == "variable_declarator")
    {
        let Some(name_node) = decl.child_by_field_name("name") else {
            continue;
        };
        let name = node_text(src, name_node).to_string();
        let sig = collapse(&format!("{mods_text} {type_text} {name}"));
        out.push(ParsedSymbol {
            name,
            kind: SymbolKind::Const,
            signature: sig,
            start_line: start,
            end_line: end,
            exported,
            body_hash: body_hash(src, field),
            doc: doc.clone(),
            children: vec![],
        });
    }
    out
}

/// `using A.B;` -> `A.B`; `using static A.B.C;` -> `A.B.C`;
/// `using D = A.B.C;` -> `A.B.C`; `global using ...` alike.
fn collect_using(node: Node, src: &str, out: &mut ParsedFile) {
    if let Some(name_node) = node.child_by_field_name("name") {
        // Alias form: the aliased target is the other named child.
        let name_id = name_node.id();
        let mut cursor = node.walk();
        let target = node.named_children(&mut cursor).find(|c| c.id() != name_id);
        if let Some(target) = target {
            out.imports.push(node_text(src, target).to_string());
        }
    } else {
        let mut cursor = node.walk();
        if let Some(target) = node.named_children(&mut cursor).last() {
            out.imports.push(node_text(src, target).to_string());
        }
    }
}

fn collect_refs(root: Node, src: &str, refs: &mut Vec<RefOccurrence>) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "invocation_expression" => {
                if let Some(f) = node.child_by_field_name("function") {
                    if let Some(n) = resolve_name_node(f) {
                        refs.push(RefOccurrence {
                            name: node_text(src, n).to_string(),
                            line: n.start_position().row as u32 + 1,
                            kind: RefKind::Call,
                        });
                    }
                }
            }
            "object_creation_expression" => {
                if let Some(t) = node.child_by_field_name("type") {
                    if let Some(n) = resolve_name_node(t) {
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

/// Resolves an expression/type node down to the trailing `identifier`:
/// `foo` -> `foo`; `foo<T>` (generic_name) -> `foo`;
/// `Ns.Foo` (qualified_name) -> `Foo`; `obj.Method` (member_access) -> `Method`.
fn resolve_name_node(n: Node) -> Option<Node> {
    match n.kind() {
        "identifier" => Some(n),
        "generic_name" => {
            let mut cursor = n.walk();
            let found = n.named_children(&mut cursor).find(|c| c.kind() == "identifier");
            found
        }
        "qualified_name" => {
            let name = n.child_by_field_name("name")?;
            resolve_name_node(name)
        }
        "member_access_expression" => {
            let name = n.child_by_field_name("name")?;
            resolve_name_node(name)
        }
        _ => None,
    }
}

/// First meaningful line of a `///` XML doc comment directly above a
/// declaration. Strips the `///` marker and any XML tags (`<summary>`,
/// `</summary>`, `<param .../>`, ...), returning the first non-empty
/// resulting line. Only contiguous `///` comments are considered — plain
/// `//`/`/* */` comments are not treated as doc comments in C#.
fn xml_doc(node: Node, src: &str) -> Option<String> {
    let mut comments: Vec<Node> = Vec::new();
    let mut cur = node;
    for _ in 0..40 {
        let Some(sib) = cur.prev_named_sibling() else {
            break;
        };
        if sib.kind() != "comment" {
            break;
        }
        if !node_text(src, sib).trim_start().starts_with("///") {
            break;
        }
        if cur.start_position().row.saturating_sub(sib.end_position().row) > 1 {
            break;
        }
        comments.push(sib);
        cur = sib;
    }
    for c in comments.iter().rev() {
        let raw = node_text(src, *c);
        let stripped = raw.trim_start().trim_start_matches("///").trim();
        let no_tags = strip_xml_tags(stripped);
        let t = no_tags.trim();
        if t.is_empty() {
            continue;
        }
        return Some(cap_line(t));
    }
    None
}

fn strip_xml_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}
