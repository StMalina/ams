use ams_core::model::{RefKind, SymbolKind};
use ams_core::parser::csharp::CSharpParser;
use ams_core::parser::LangParser;

fn find<'a>(symbols: &'a [ams_core::model::ParsedSymbol], name: &str) -> &'a ams_core::model::ParsedSymbol {
    symbols
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("symbol `{}` not found among {:?}", name, symbols.iter().map(|s| &s.name).collect::<Vec<_>>()))
}

const CS_SRC: &str = r#"global using System;
using System.Collections.Generic;
using static System.Console;
using Alias = System.Text.StringBuilder;

namespace App.Services;

/// <summary>
/// Handles widget rendering.
/// </summary>
public class Widget : IWidget
{
    /// <summary>
    /// Gets the display name.
    /// </summary>
    public string Name { get; set; }

    public const int Max = 5;

    public Widget(string name)
    {
        Name = name;
        Helper(name);
        var sb = new StringBuilder();
    }

    public void Render()
    {
        Console.WriteLine(Name);
    }

    private void Secret()
    {
    }
}

internal class Hidden
{
}

public interface IWidget
{
    void Render();
}
"#;

#[test]
fn csharp_parses_namespace_and_class() {
    let out = CSharpParser.parse(CS_SRC).unwrap();

    let widget = find(&out.symbols, "Widget");
    assert_eq!(widget.kind, SymbolKind::Class);
    assert!(widget.exported);
    assert!(widget.start_line < widget.end_line);
    assert_eq!(
        widget.doc.as_deref(),
        Some("Handles widget rendering.")
    );

    let hidden = find(&out.symbols, "Hidden");
    assert_eq!(hidden.kind, SymbolKind::Class);
    assert!(!hidden.exported);
    assert!(hidden.start_line <= hidden.end_line);

    let iface = find(&out.symbols, "IWidget");
    assert_eq!(iface.kind, SymbolKind::Interface);
    assert!(iface.exported);
}

#[test]
fn csharp_parses_methods_and_property() {
    let out = CSharpParser.parse(CS_SRC).unwrap();
    let widget = find(&out.symbols, "Widget");

    let ctor = find(&widget.children, "Widget");
    assert_eq!(ctor.kind, SymbolKind::Method);
    assert!(!ctor.exported);
    assert!(ctor.start_line < ctor.end_line);

    let render = find(&widget.children, "Render");
    assert_eq!(render.kind, SymbolKind::Method);
    assert!(!render.exported);

    let secret = find(&widget.children, "Secret");
    assert_eq!(secret.kind, SymbolKind::Method);
    assert!(!secret.exported);

    let name_prop = find(&widget.children, "Name");
    assert_eq!(name_prop.kind, SymbolKind::Method);
    assert!(!name_prop.exported);
    assert_eq!(
        name_prop.doc.as_deref(),
        Some("Gets the display name.")
    );

    let max_const = find(&widget.children, "Max");
    assert_eq!(max_const.kind, SymbolKind::Const);
    assert!(max_const.exported);
}

#[test]
fn csharp_parses_imports() {
    let out = CSharpParser.parse(CS_SRC).unwrap();
    assert!(out.imports.iter().any(|i| i == "System"), "imports: {:?}", out.imports);
    assert!(
        out.imports.iter().any(|i| i == "System.Collections.Generic"),
        "imports: {:?}",
        out.imports
    );
    assert!(
        out.imports.iter().any(|i| i == "System.Console"),
        "imports: {:?}",
        out.imports
    );
    assert!(
        out.imports.iter().any(|i| i == "System.Text.StringBuilder"),
        "imports: {:?}",
        out.imports
    );
}

#[test]
fn csharp_parses_call_refs() {
    let out = CSharpParser.parse(CS_SRC).unwrap();
    let calls: Vec<(&str, u32)> = out
        .refs
        .iter()
        .filter(|r| r.kind == RefKind::Call)
        .map(|r| (r.name.as_str(), r.line))
        .collect();
    assert!(calls.iter().any(|(n, _)| *n == "Helper"), "calls: {:?}", calls);
    assert!(calls.iter().any(|(n, _)| *n == "StringBuilder"), "calls: {:?}", calls);
    assert!(calls.iter().any(|(n, _)| *n == "WriteLine"), "calls: {:?}", calls);
}
