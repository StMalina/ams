use ams_core::model::SymbolKind;
use ams_core::parser::go::GoParser;
use ams_core::parser::python::PythonParser;
use ams_core::parser::rust::RustParser;
use ams_core::parser::LangParser;

fn find<'a>(symbols: &'a [ams_core::model::ParsedSymbol], name: &str) -> &'a ams_core::model::ParsedSymbol {
    symbols
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("symbol `{}` not found among {:?}", name, symbols.iter().map(|s| &s.name).collect::<Vec<_>>()))
}

// ---------------------------------------------------------------------
// Rust
// ---------------------------------------------------------------------

const RUST_SRC: &str = r#"use std::collections::HashMap;
use crate::utils::helper_fn;

pub fn add(a: i32, b: i32) -> i32 {
    helper_fn(a) + b
}

fn private_helper() {}

pub struct Point {
    pub x: i32,
    y: i32,
}

impl Point {
    pub fn new() -> Self {
        Self { x: 0, y: 0 }
    }

    fn area(&self) -> i32 {
        self.x * self.y
    }
}

pub trait Shape {
    fn area(&self) -> f64;
}

pub const MAX_POINTS: i32 = 100;

fn call_things() {
    println!("hi");
    add(1, 2);
    Point::new();
}
"#;

#[test]
fn rust_parses_top_level_function() {
    let out = RustParser.parse(RUST_SRC).unwrap();
    let add = find(&out.symbols, "add");
    assert_eq!(add.kind, SymbolKind::Function);
    assert!(add.exported);
    assert_eq!(add.start_line, 4);
    assert_eq!(add.end_line, 6);

    let private = find(&out.symbols, "private_helper");
    assert_eq!(private.kind, SymbolKind::Function);
    assert!(!private.exported);
    assert_eq!(private.start_line, 8);
    assert_eq!(private.end_line, 8);
}

#[test]
fn rust_parses_struct() {
    let out = RustParser.parse(RUST_SRC).unwrap();
    let point = find(&out.symbols, "Point");
    assert_eq!(point.kind, SymbolKind::Struct);
    assert!(point.exported);
    assert_eq!(point.start_line, 10);
    assert_eq!(point.end_line, 13);
}

#[test]
fn rust_parses_impl_methods() {
    let out = RustParser.parse(RUST_SRC).unwrap();
    let impl_sym = out
        .symbols
        .iter()
        .find(|s| s.kind == SymbolKind::Class && s.name == "Point")
        .expect("impl Point block");
    assert_eq!(impl_sym.start_line, 15);
    assert_eq!(impl_sym.end_line, 23);

    let new_method = find(&impl_sym.children, "new");
    assert_eq!(new_method.kind, SymbolKind::Method);
    assert!(new_method.exported);

    let area_method = find(&impl_sym.children, "area");
    assert_eq!(area_method.kind, SymbolKind::Method);
    assert!(!area_method.exported);
}

#[test]
fn rust_parses_trait_and_const() {
    let out = RustParser.parse(RUST_SRC).unwrap();
    let shape = find(&out.symbols, "Shape");
    assert_eq!(shape.kind, SymbolKind::Trait);
    assert!(shape.exported);
    let trait_method = find(&shape.children, "area");
    assert_eq!(trait_method.kind, SymbolKind::Method);
    assert!(!trait_method.exported);

    let max_points = find(&out.symbols, "MAX_POINTS");
    assert_eq!(max_points.kind, SymbolKind::Const);
    assert!(max_points.exported);
}

#[test]
fn rust_parses_imports() {
    let out = RustParser.parse(RUST_SRC).unwrap();
    assert!(out.imports.iter().any(|i| i == "std::collections::HashMap"));
    assert!(out.imports.iter().any(|i| i == "crate::utils::helper_fn"));
}

#[test]
fn rust_parses_refs() {
    let out = RustParser.parse(RUST_SRC).unwrap();
    let names: Vec<&str> = out.refs.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"println"), "refs: {:?}", names);
    assert!(names.contains(&"add"), "refs: {:?}", names);
    assert!(names.contains(&"new"), "refs: {:?}", names);
    assert!(names.contains(&"helper_fn"), "refs: {:?}", names);
}

// ---------------------------------------------------------------------
// Python
// ---------------------------------------------------------------------

const PY_SRC: &str = r#"import os
from os import path
from . import sibling

MAX_ITEMS = 10
_private_const = 1

def top_func(a, b):
    return helper(a) + os.path.join(b)

def _hidden():
    pass

class Widget(Base):
    def __init__(self, x):
        self.x = x

    def render(self):
        return self.x

    def _internal(self):
        pass
"#;

#[test]
fn python_parses_top_level_function() {
    let out = PythonParser.parse(PY_SRC).unwrap();
    let f = find(&out.symbols, "top_func");
    assert_eq!(f.kind, SymbolKind::Function);
    assert!(f.exported);
    assert_eq!(f.start_line, 8);
    assert_eq!(f.end_line, 9);

    let hidden = find(&out.symbols, "_hidden");
    assert_eq!(hidden.kind, SymbolKind::Function);
    assert!(!hidden.exported);
}

#[test]
fn python_parses_class_and_methods() {
    let out = PythonParser.parse(PY_SRC).unwrap();
    let widget = find(&out.symbols, "Widget");
    assert_eq!(widget.kind, SymbolKind::Class);
    assert!(widget.exported);
    assert_eq!(widget.start_line, 14);
    assert_eq!(widget.end_line, 22);

    let init = find(&widget.children, "__init__");
    assert_eq!(init.kind, SymbolKind::Method);
    assert!(!init.exported);

    let render = find(&widget.children, "render");
    assert_eq!(render.kind, SymbolKind::Method);
    assert!(!render.exported);
}

#[test]
fn python_parses_const() {
    let out = PythonParser.parse(PY_SRC).unwrap();
    let max_items = find(&out.symbols, "MAX_ITEMS");
    assert_eq!(max_items.kind, SymbolKind::Const);
    assert!(max_items.exported);
}

#[test]
fn python_parses_imports() {
    let out = PythonParser.parse(PY_SRC).unwrap();
    assert!(out.imports.iter().any(|i| i == "os"));
    assert!(out.imports.iter().any(|i| i == "."));
}

#[test]
fn python_parses_refs() {
    let out = PythonParser.parse(PY_SRC).unwrap();
    let names: Vec<&str> = out.refs.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"helper"), "refs: {:?}", names);
    assert!(names.contains(&"join"), "refs: {:?}", names);
}

// ---------------------------------------------------------------------
// Go
// ---------------------------------------------------------------------

const GO_SRC: &str = r#"package widgets

import (
	"fmt"
	str "strings"
)

const MaxItems = 10

type Point struct {
	X int
	Y int
}

type Shaper interface {
	Area() int
}

func NewPoint(x int) Point {
	fmt.Println(x)
	return Point{X: x}
}

func (p *Point) Area() int {
	return p.X * p.Y
}

func doStuff() {
	str.ToUpper("hi")
	NewPoint(1)
}
"#;

#[test]
fn go_parses_top_level_function() {
    let out = GoParser.parse(GO_SRC).unwrap();
    let f = find(&out.symbols, "NewPoint");
    assert_eq!(f.kind, SymbolKind::Function);
    assert!(f.exported);
    assert_eq!(f.start_line, 19);
    assert_eq!(f.end_line, 22);

    let g = find(&out.symbols, "doStuff");
    assert_eq!(g.kind, SymbolKind::Function);
    assert!(!g.exported);
}

#[test]
fn go_parses_method_with_receiver() {
    let out = GoParser.parse(GO_SRC).unwrap();
    let m = find(&out.symbols, "Area");
    assert_eq!(m.kind, SymbolKind::Method);
    assert!(m.exported);
    assert_eq!(m.start_line, 24);
    assert_eq!(m.end_line, 26);
    assert!(m.signature.contains("Point"));
}

#[test]
fn go_parses_struct_interface_const() {
    let out = GoParser.parse(GO_SRC).unwrap();
    let point = find(&out.symbols, "Point");
    assert_eq!(point.kind, SymbolKind::Struct);
    assert!(point.exported);
    assert_eq!(point.start_line, 10);
    assert_eq!(point.end_line, 13);

    let shaper = find(&out.symbols, "Shaper");
    assert_eq!(shaper.kind, SymbolKind::Interface);
    assert!(shaper.exported);

    let max_items = find(&out.symbols, "MaxItems");
    assert_eq!(max_items.kind, SymbolKind::Const);
    assert!(max_items.exported);
}

#[test]
fn go_parses_imports() {
    let out = GoParser.parse(GO_SRC).unwrap();
    assert!(out.imports.iter().any(|i| i == "fmt"));
    assert!(out.imports.iter().any(|i| i == "strings"));
}

#[test]
fn go_parses_refs() {
    let out = GoParser.parse(GO_SRC).unwrap();
    let names: Vec<&str> = out.refs.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"Println"), "refs: {:?}", names);
    assert!(names.contains(&"ToUpper"), "refs: {:?}", names);
    assert!(names.contains(&"NewPoint"), "refs: {:?}", names);
}

// ---------------------------------------------------------------------
// CommonJS (JS via TSX grammar)

const JS_COMMONJS: &str = r#"const _ = require('lodash')
const { db } = require('./db')

function listUsers(filter) {
  return db.query(filter)
}

const helper = () => 42

exports.directFn = function (a, b) {
  return a + b
}

module.exports = { listUsers, helper }
"#;

#[test]
fn commonjs_require_becomes_import_not_const() {
    let parsed = ams_core::parser::parser_for_ext("js")
        .unwrap()
        .parse(JS_COMMONJS)
        .unwrap();
    assert!(parsed.imports.contains(&"lodash".to_string()));
    assert!(parsed.imports.contains(&"./db".to_string()));
    assert!(!parsed.symbols.iter().any(|s| s.name == "_" || s.name == "db"));
}

#[test]
fn commonjs_module_exports_marks_symbols_exported() {
    let parsed = ams_core::parser::parser_for_ext("js")
        .unwrap()
        .parse(JS_COMMONJS)
        .unwrap();
    assert!(find(&parsed.symbols, "listUsers").exported);
    assert!(find(&parsed.symbols, "helper").exported);
    let direct = find(&parsed.symbols, "directFn");
    assert!(direct.exported);
    assert_eq!(direct.kind, SymbolKind::Function);
}
