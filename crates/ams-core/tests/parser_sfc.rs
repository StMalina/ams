use ams_core::model::{ParsedSymbol, SymbolKind};
use ams_core::parser::{parser_for_ext, LangParser};

fn find<'a>(symbols: &'a [ParsedSymbol], name: &str) -> &'a ParsedSymbol {
    symbols.iter().find(|s| s.name == name).unwrap_or_else(|| {
        panic!(
            "symbol `{}` not found among {:?}",
            name,
            symbols.iter().map(|s| &s.name).collect::<Vec<_>>()
        )
    })
}

const VUE_SRC: &str = r#"<template>
  <div>{{ x }}</div>
</template>

<script setup lang="ts">
import { ref } from 'vue'
function greet(name: string) { return name }
const x = ref(0)
</script>
"#;

#[test]
fn vue_script_block_symbols_map_to_outer_lines() {
    let out = parser_for_ext("vue").unwrap().parse(VUE_SRC).unwrap();

    // Span points at the OUTER-file line (7), not the slice-relative line (3).
    let greet = find(&out.symbols, "greet");
    assert_eq!(greet.kind, SymbolKind::Function);
    assert_eq!(greet.start_line, 7);

    let x = find(&out.symbols, "x");
    assert_eq!(x.start_line, 8);

    assert!(out.imports.iter().any(|i| i == "vue"), "imports: {:?}", out.imports);
}

const SVELTE_SRC: &str = r#"<script context="module">
export const store = 0
</script>

<div>hi</div>

<script>
function handleClick() {}
</script>
"#;

#[test]
fn svelte_merges_multiple_script_blocks() {
    let out = parser_for_ext("svelte").unwrap().parse(SVELTE_SRC).unwrap();

    // module block
    let store = find(&out.symbols, "store");
    assert_eq!(store.start_line, 2);

    // instance block, further down the file
    let click = find(&out.symbols, "handleClick");
    assert_eq!(click.kind, SymbolKind::Function);
    assert_eq!(click.start_line, 8);
}

const ASTRO_SRC: &str = r#"---
import Layout from './Layout.astro'
const title = 'Home'
function greet() { return title }
---
<h1>{title}</h1>
"#;

#[test]
fn astro_frontmatter_symbols_map_to_outer_lines() {
    let out = parser_for_ext("astro").unwrap().parse(ASTRO_SRC).unwrap();

    let greet = find(&out.symbols, "greet");
    assert_eq!(greet.kind, SymbolKind::Function);
    assert_eq!(greet.start_line, 4);

    assert!(
        out.imports.iter().any(|i| i == "./Layout.astro"),
        "imports: {:?}",
        out.imports
    );

    // Template markup below the fence is not indexed — no stray symbols from it.
    assert!(out.symbols.iter().all(|s| s.name != "h1"));
}
