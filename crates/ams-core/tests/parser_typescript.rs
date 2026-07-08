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

const OBJECT_SRC: &str = r#"const ordersRegistry = {
  limit: 50,
  customer: {
    async updateContact(orderId, contact) {
      return db.update(orderId, contact);
    },
    remove: async (id) => db.remove(id),
  },
  list(filter) {
    return db.query(filter);
  },
};

const config = { host: "localhost", port: 5432 };

module.exports = {
  ordersRegistry,
  async handleBulkSync(req, res) {
    return res.send(await bulk(req.body));
  },
  ping: () => "pong",
};
"#;

#[test]
fn object_literal_methods_are_children() {
    let parsed = JS.parse(OBJECT_SRC).unwrap();

    let obj = find(&parsed.symbols, "ordersRegistry");
    assert_eq!(obj.kind, SymbolKind::Const);

    // Shorthand method at the top level of the literal.
    let list = find(&obj.children, "list");
    assert_eq!(list.kind, SymbolKind::Method);
    assert_eq!(list.signature, "list(filter)");
    assert_eq!((list.start_line, list.end_line), (9, 11));

    // Nested object carries its own methods; plain data keys are skipped.
    let customer = find(&obj.children, "customer");
    assert!(obj.children.iter().all(|c| c.name != "limit"));
    let update = find(&customer.children, "updateContact");
    assert_eq!(update.kind, SymbolKind::Method);
    assert_eq!(update.signature, "async updateContact(orderId, contact)");
    assert_eq!((update.start_line, update.end_line), (4, 6));

    // Arrow-function property counts as a method too.
    let remove = find(&customer.children, "remove");
    assert_eq!(remove.signature, "remove: async (id) =>");

    // Pure-data object stays a childless Const.
    assert!(find(&parsed.symbols, "config").children.is_empty());
}

#[test]
fn module_exports_inline_methods_are_exported_symbols() {
    let parsed = JS.parse(OBJECT_SRC).unwrap();

    let bulk = find(&parsed.symbols, "handleBulkSync");
    assert!(bulk.exported);
    assert_eq!(bulk.kind, SymbolKind::Method);

    let ping = find(&parsed.symbols, "ping");
    assert!(ping.exported);

    // `ordersRegistry` shorthand re-export still flips the const to exported.
    assert!(find(&parsed.symbols, "ordersRegistry").exported);
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
