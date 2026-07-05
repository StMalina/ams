use ams_core::model::{RefKind, SymbolKind};
use ams_core::parser::kotlin::KotlinParser;
use ams_core::parser::LangParser;

fn find<'a>(symbols: &'a [ams_core::model::ParsedSymbol], name: &str) -> &'a ams_core::model::ParsedSymbol {
    symbols
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("symbol `{}` not found among {:?}", name, symbols.iter().map(|s| &s.name).collect::<Vec<_>>()))
}

const KT_SRC: &str = r#"package com.example.widgets

import kotlin.collections.List
import kotlin.io.println as p
import kotlin.math.*

/**
 * Renders things on screen.
 */
class Widget(val x: Int) {
    /** Renders the widget to a string. */
    fun render(): String {
        helper(x)
        return "ok"
    }

    private fun secret() {}
}

data class Point(val x: Int, val y: Int)

object Registry {
    fun register() {
        println("registered")
    }
}

interface Shape {
    fun area(): Double
}

fun topFun(a: Int): Int {
    return helper(a)
}

private fun hidden(): Int {
    return 0
}

val MAX_ITEMS: Int = 10

fun caller() {
    topFun(1)
    val w = Widget(1)
    w.render()
}
"#;

#[test]
fn kotlin_parses_class_with_methods_and_doc() {
    let out = KotlinParser.parse(KT_SRC).unwrap();
    let widget = find(&out.symbols, "Widget");
    assert_eq!(widget.kind, SymbolKind::Class);
    assert!(widget.exported);
    assert!(widget.start_line < widget.end_line);
    assert_eq!(
        widget.doc.as_deref(),
        Some("Renders things on screen.")
    );

    let render = find(&widget.children, "render");
    assert_eq!(render.kind, SymbolKind::Method);
    assert!(!render.exported);
    assert!(render.start_line < render.end_line);
    assert_eq!(
        render.doc.as_deref(),
        Some("Renders the widget to a string.")
    );

    let secret = find(&widget.children, "secret");
    assert_eq!(secret.kind, SymbolKind::Method);
    assert!(!secret.exported);
}

#[test]
fn kotlin_parses_data_class() {
    let out = KotlinParser.parse(KT_SRC).unwrap();
    let point = find(&out.symbols, "Point");
    assert_eq!(point.kind, SymbolKind::Class);
    assert!(point.exported);
    assert!(point.start_line <= point.end_line);
}

#[test]
fn kotlin_parses_object_declaration() {
    let out = KotlinParser.parse(KT_SRC).unwrap();
    let registry = find(&out.symbols, "Registry");
    assert_eq!(registry.kind, SymbolKind::Class);
    assert!(registry.exported);
    assert!(registry.start_line < registry.end_line);

    let register = find(&registry.children, "register");
    assert_eq!(register.kind, SymbolKind::Method);
    assert!(!register.exported);
}

#[test]
fn kotlin_parses_interface() {
    let out = KotlinParser.parse(KT_SRC).unwrap();
    let shape = find(&out.symbols, "Shape");
    assert_eq!(shape.kind, SymbolKind::Interface);
    assert!(shape.exported);
}

#[test]
fn kotlin_parses_top_level_fun_and_visibility() {
    let out = KotlinParser.parse(KT_SRC).unwrap();
    let top_fun = find(&out.symbols, "topFun");
    assert_eq!(top_fun.kind, SymbolKind::Function);
    assert!(top_fun.exported);
    assert!(top_fun.start_line < top_fun.end_line);

    let hidden = find(&out.symbols, "hidden");
    assert_eq!(hidden.kind, SymbolKind::Function);
    assert!(!hidden.exported);
}

#[test]
fn kotlin_parses_top_level_const() {
    let out = KotlinParser.parse(KT_SRC).unwrap();
    let max_items = find(&out.symbols, "MAX_ITEMS");
    assert_eq!(max_items.kind, SymbolKind::Const);
    assert!(max_items.exported);
    assert_eq!(max_items.start_line, max_items.end_line);
}

#[test]
fn kotlin_parses_imports() {
    let out = KotlinParser.parse(KT_SRC).unwrap();
    assert!(
        out.imports.iter().any(|i| i == "kotlin.collections.List"),
        "imports: {:?}",
        out.imports
    );
    assert!(
        out.imports.iter().any(|i| i == "kotlin.io.println"),
        "imports: {:?}",
        out.imports
    );
    assert!(
        out.imports.iter().any(|i| i == "kotlin.math"),
        "imports: {:?}",
        out.imports
    );
}

#[test]
fn kotlin_parses_call_refs() {
    let out = KotlinParser.parse(KT_SRC).unwrap();
    let calls: Vec<&str> = out
        .refs
        .iter()
        .filter(|r| r.kind == RefKind::Call)
        .map(|r| r.name.as_str())
        .collect();
    assert!(calls.contains(&"helper"), "calls: {:?}", calls);
    assert!(calls.contains(&"topFun"), "calls: {:?}", calls);
    assert!(calls.contains(&"Widget"), "calls: {:?}", calls);
    assert!(calls.contains(&"render"), "calls: {:?}", calls);
}
