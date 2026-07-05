use ams_core::model::{RefKind, SymbolKind};
use ams_core::parser::ruby::RubyParser;
use ams_core::parser::LangParser;

fn find<'a>(symbols: &'a [ams_core::model::ParsedSymbol], name: &str) -> &'a ams_core::model::ParsedSymbol {
    symbols
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("symbol `{}` not found among {:?}", name, symbols.iter().map(|s| &s.name).collect::<Vec<_>>()))
}

const RUBY_SRC: &str = r#"#!/usr/bin/env ruby
# frozen_string_literal: true

require 'json'
require_relative '../lib/helper'

# Adds two numbers together.
def top_level(a, b)
  a + b
end

module Widgets
  # Represents a single widget.
  class Widget < Base
    # Greets by name.
    def greet(name)
      puts "hi #{name}"
    end

    def self.build
      new
    end

    private

    def secret
      1
    end
  end
end
"#;

#[test]
fn ruby_parses_top_level_function() {
    let out = RubyParser.parse(RUBY_SRC).unwrap();
    let f = find(&out.symbols, "top_level");
    assert_eq!(f.kind, SymbolKind::Function);
    assert!(f.exported);
    assert!(f.start_line < f.end_line);
    assert_eq!(f.doc.as_deref(), Some("Adds two numbers together."));
}

#[test]
fn ruby_parses_module_and_class() {
    let out = RubyParser.parse(RUBY_SRC).unwrap();
    let widgets = find(&out.symbols, "Widgets");
    assert_eq!(widgets.kind, SymbolKind::Module);
    assert!(widgets.exported);
    assert!(widgets.start_line < widgets.end_line);

    let widget = find(&widgets.children, "Widget");
    assert_eq!(widget.kind, SymbolKind::Class);
    assert!(widget.exported);
    assert!(widget.start_line < widget.end_line);
    assert_eq!(widget.doc.as_deref(), Some("Represents a single widget."));
    assert!(widget.signature.contains("Widget"));
    assert!(widget.signature.contains("Base"));
}

#[test]
fn ruby_parses_methods_incl_singleton() {
    let out = RubyParser.parse(RUBY_SRC).unwrap();
    let widget = find(&find(&out.symbols, "Widgets").children, "Widget");

    let greet = find(&widget.children, "greet");
    assert_eq!(greet.kind, SymbolKind::Method);
    assert!(!greet.exported, "methods inside a class are not exported");
    assert!(greet.start_line < greet.end_line);
    assert_eq!(greet.doc.as_deref(), Some("Greets by name."));

    let build = find(&widget.children, "build");
    assert_eq!(build.kind, SymbolKind::Method);
    assert!(!build.exported);

    let secret = find(&widget.children, "secret");
    assert_eq!(secret.kind, SymbolKind::Method);
    assert!(
        !secret.exported,
        "methods after a bare `private` call are still exported=false"
    );
}

#[test]
fn ruby_parses_requires_as_imports() {
    let out = RubyParser.parse(RUBY_SRC).unwrap();
    assert!(out.imports.iter().any(|i| i == "json"), "imports: {:?}", out.imports);
    assert!(
        out.imports.iter().any(|i| i == "../lib/helper"),
        "imports: {:?}",
        out.imports
    );
}

#[test]
fn ruby_parses_method_call_refs() {
    let out = RubyParser.parse(RUBY_SRC).unwrap();
    let calls: Vec<(&str, u32)> = out
        .refs
        .iter()
        .filter(|r| r.kind == RefKind::Call)
        .map(|r| (r.name.as_str(), r.line))
        .collect();
    assert!(calls.contains(&("puts", 17)), "calls: {:?}", calls);
}

#[test]
fn ruby_parses_symbol_value_refs() {
    let src = "before_action :require_login\nrender json: :ok\n";
    let out = RubyParser.parse(src).unwrap();
    assert!(
        out.refs
            .iter()
            .any(|r| r.name == "require_login" && r.kind == RefKind::Value && r.line == 1),
        "refs: {:?}",
        out.refs
    );
    // The symbol nested inside `json: :ok`'s hash pair is deliberately not
    // captured (only direct positional argument_list children are).
    assert!(!out.refs.iter().any(|r| r.name == "ok"), "refs: {:?}", out.refs);
}
