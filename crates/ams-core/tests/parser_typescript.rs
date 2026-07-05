use ams_core::model::{ParsedSymbol, SymbolKind};
use ams_core::parser::typescript::TypeScriptParser;
use ams_core::parser::LangParser;

fn find<'a>(symbols: &'a [ParsedSymbol], name: &str) -> &'a ParsedSymbol {
    symbols
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| {
            panic!(
                "symbol `{}` not found among {:?}",
                name,
                symbols.iter().map(|s| &s.name).collect::<Vec<_>>()
            )
        })
}

// js/jsx/mjs/cjs all route through the tsx grammar in this crate.
const JS: TypeScriptParser = TypeScriptParser { tsx: true };

const TEST_SRC: &str = r#"const { describe, it } = require('node:test');

describe('cell creation', () => {
  it('creates child cells bound to branches', () => {});
  it('rejects when region missing', () => {});

  describe('nested perms', () => {
    it.only('allows admin', () => {});
  });
});

test('standalone top-level check', () => {});
lab.test('lab-style case', () => {});

// must NOT be treated as a test block: property-form head with no callback.
if (re.test("not a test block")) {}
"#;

#[test]
fn test_blocks_become_test_symbols() {
    let parsed = JS.parse(TEST_SRC).unwrap();

    let suite = find(&parsed.symbols, "cell creation");
    assert_eq!(suite.kind, SymbolKind::Test);
    assert_eq!(suite.signature, r#"describe "cell creation""#);
    assert!(!suite.exported);

    // Nested cases hang off the describe block as children.
    let case = find(&suite.children, "creates child cells bound to branches");
    assert_eq!(case.kind, SymbolKind::Test);
    assert_eq!(case.signature, r#"it "creates child cells bound to branches""#);

    // describe nests inside describe; `.only` modifier is kept in the head.
    let nested = find(&suite.children, "nested perms");
    let only = find(&nested.children, "allows admin");
    assert_eq!(only.signature, r#"it.only "allows admin""#);

    // Top-level `test(...)` and `lab.test(...)` are both captured.
    assert_eq!(
        find(&parsed.symbols, "standalone top-level check").kind,
        SymbolKind::Test
    );
    assert_eq!(find(&parsed.symbols, "lab-style case").kind, SymbolKind::Test);
}

#[test]
fn property_head_without_callback_is_not_a_test() {
    let parsed = JS.parse(TEST_SRC).unwrap();
    // `re.test("not a test block")` has a string arg but no callback — skipped.
    assert!(
        parsed.symbols.iter().all(|s| s.name != "not a test block"),
        "regex .test() must not be indexed as a test block"
    );
}
