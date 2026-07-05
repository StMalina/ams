use ams_core::model::{RefKind, SymbolKind};
use ams_core::parser::java::JavaParser;
use ams_core::parser::LangParser;

fn find<'a>(symbols: &'a [ams_core::model::ParsedSymbol], name: &str) -> &'a ams_core::model::ParsedSymbol {
    symbols
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("symbol `{}` not found among {:?}", name, symbols.iter().map(|s| &s.name).collect::<Vec<_>>()))
}

const JAVA_SRC: &str = r#"package com.example.app;

import java.util.List;
import static java.util.Collections.emptyList;
import java.util.*;

/**
 * Renders widgets to the screen.
 */
public class Widget implements Renderable {
    public static final int MAX = 5;
    private int x;

    /**
     * Builds a widget bound to the given value.
     */
    public Widget(int x) {
        this.x = x;
        helper(x);
    }

    public int render() {
        return this.x;
    }

    private void secret() {
    }
}

interface Renderable {
    void render();
}

enum Color {
    RED, GREEN, BLUE;
}
"#;

#[test]
fn java_parses_public_class_with_javadoc() {
    let out = JavaParser.parse(JAVA_SRC).unwrap();
    let widget = find(&out.symbols, "Widget");
    assert_eq!(widget.kind, SymbolKind::Class);
    assert!(widget.exported);
    assert!(widget.start_line < widget.end_line);
    assert_eq!(
        widget.doc.as_deref(),
        Some("Renders widgets to the screen.")
    );
}

#[test]
fn java_parses_methods_and_constructor() {
    let out = JavaParser.parse(JAVA_SRC).unwrap();
    let widget = find(&out.symbols, "Widget");

    let ctor = find(&widget.children, "Widget");
    assert_eq!(ctor.kind, SymbolKind::Method);
    assert!(!ctor.exported);
    assert!(ctor.start_line < ctor.end_line);
    assert_eq!(
        ctor.doc.as_deref(),
        Some("Builds a widget bound to the given value.")
    );

    let render = find(&widget.children, "render");
    assert_eq!(render.kind, SymbolKind::Method);
    assert!(!render.exported); // methods are always exported=false per spec

    let secret = find(&widget.children, "secret");
    assert_eq!(secret.kind, SymbolKind::Method);
    assert!(!secret.exported);
}

#[test]
fn java_parses_const_field() {
    let out = JavaParser.parse(JAVA_SRC).unwrap();
    let widget = find(&out.symbols, "Widget");
    let max = find(&widget.children, "MAX");
    assert_eq!(max.kind, SymbolKind::Const);
    assert!(max.exported);
    assert!(max.signature.contains("MAX"));

    // non-const field is not surfaced as a symbol
    assert!(!widget.children.iter().any(|c| c.name == "x"));
}

#[test]
fn java_parses_interface_and_enum() {
    let out = JavaParser.parse(JAVA_SRC).unwrap();

    let renderable = find(&out.symbols, "Renderable");
    assert_eq!(renderable.kind, SymbolKind::Interface);
    assert!(!renderable.exported); // package-private, no `public` modifier
    assert!(renderable.start_line < renderable.end_line);

    let color = find(&out.symbols, "Color");
    assert_eq!(color.kind, SymbolKind::Enum);
    assert!(!color.exported);
}

#[test]
fn java_parses_imports() {
    let out = JavaParser.parse(JAVA_SRC).unwrap();
    assert!(out.imports.iter().any(|i| i == "java.util.List"), "imports: {:?}", out.imports);
    assert!(out.imports.iter().any(|i| i == "java.util.Collections"), "imports: {:?}", out.imports);
    assert!(out.imports.iter().any(|i| i == "java.util"), "imports: {:?}", out.imports);
}

#[test]
fn java_parses_call_refs() {
    let out = JavaParser.parse(JAVA_SRC).unwrap();
    let calls: Vec<(&str, u32)> = out
        .refs
        .iter()
        .filter(|r| r.kind == RefKind::Call)
        .map(|r| (r.name.as_str(), r.line))
        .collect();
    assert!(calls.contains(&("helper", 19)), "calls: {:?}", calls);
}

#[test]
fn java_parses_object_creation_ref() {
    let src = "class C {\n    void m() {\n        Object o = new Foo();\n    }\n}\n";
    let out = JavaParser.parse(src).unwrap();
    let calls: Vec<(&str, u32)> = out
        .refs
        .iter()
        .filter(|r| r.kind == RefKind::Call)
        .map(|r| (r.name.as_str(), r.line))
        .collect();
    assert!(calls.contains(&("Foo", 3)), "calls: {:?}", calls);
}
