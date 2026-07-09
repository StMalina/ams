use super::typescript::TypeScriptParser;
use super::{count_loc, LangParser};
use crate::model::{ParsedFile, ParsedSymbol};
use anyhow::Result;

/// Single-file-component formats whose real code lives inside a markup wrapper:
/// Vue/Svelte `<script>` blocks and Astro `---` frontmatter. The template markup
/// stays unindexed (grep territory) — only the script region carries symbols.
#[derive(Clone, Copy)]
pub enum SfcFormat {
    Vue,
    Svelte,
    Astro,
}

pub struct SfcParser {
    pub format: SfcFormat,
}

/// One extracted script region plus where it sits in the outer file.
struct Block<'a> {
    content: &'a str,
    /// Byte offset of `content` within the outer source.
    start_byte: usize,
    /// TSX-capable grammar (plain JS / `lang="tsx"`) vs the TS grammar (`lang="ts"`).
    tsx: bool,
}

impl LangParser for SfcParser {
    fn lang_id(&self) -> &'static str {
        match self.format {
            SfcFormat::Vue => "vue",
            SfcFormat::Svelte => "svelte",
            SfcFormat::Astro => "astro",
        }
    }

    fn parse(&self, source: &str) -> Result<ParsedFile> {
        // loc counts the whole outer file; per-block loc is discarded.
        let mut out = ParsedFile {
            loc: count_loc(source),
            ..Default::default()
        };

        for block in self.locate_blocks(source) {
            let parser = TypeScriptParser { tsx: block.tsx };
            let sub = parser.parse(block.content)?;
            // `line_span` inside the TS parser is relative to the slice; slice
            // line 1 maps to outer line `offset + 1`, where `offset` is the count
            // of newlines before the slice start.
            let offset = source[..block.start_byte]
                .bytes()
                .filter(|&b| b == b'\n')
                .count() as u32;
            merge_block(&mut out, sub, offset);
        }

        Ok(out)
    }
}

impl SfcParser {
    fn locate_blocks<'a>(&self, source: &'a str) -> Vec<Block<'a>> {
        match self.format {
            SfcFormat::Vue | SfcFormat::Svelte => locate_script_blocks(source),
            SfcFormat::Astro => locate_frontmatter(source),
        }
    }
}

/// Shift every line number in `sub` by `offset` and fold it into `out`.
fn merge_block(out: &mut ParsedFile, mut sub: ParsedFile, offset: u32) {
    fn shift(sym: &mut ParsedSymbol, offset: u32) {
        sym.start_line += offset;
        sym.end_line += offset;
        for child in &mut sym.children {
            shift(child, offset);
        }
    }
    for sym in &mut sub.symbols {
        shift(sym, offset);
    }
    for r in &mut sub.refs {
        r.line += offset;
    }
    out.imports.extend(sub.imports);
    out.symbols.extend(sub.symbols);
    out.refs.extend(sub.refs);
    // out.loc is already the whole-file count; do not add sub.loc.
}

/// Vue/Svelte: every `<script ...>...</script>` region. Multiple blocks
/// (`<script setup>` + `<script>`, Svelte `context="module"` + main) are all
/// returned and merged by the caller.
fn locate_script_blocks(source: &str) -> Vec<Block<'_>> {
    let mut blocks = Vec::new();
    let mut search = 0;
    while let Some(rel) = source[search..].find("<script") {
        let tag_start = search + rel;
        let after_tag = tag_start + "<script".len();
        // Guard against `<scriptFoo`: the tag name must end at a boundary.
        let boundary = source[after_tag..]
            .chars()
            .next()
            .map_or(true, |c| c.is_whitespace() || c == '>' || c == '/');
        if !boundary {
            search = after_tag;
            continue;
        }
        // Opening tag ends at the next '>'.
        let Some(gt_rel) = source[after_tag..].find('>') else {
            break;
        };
        let tag_end = after_tag + gt_rel; // index of '>'
        let attr = &source[after_tag..tag_end];
        let content_start = tag_end + 1;
        let Some(close_rel) = source[content_start..].find("</script") else {
            break;
        };
        let content_end = content_start + close_rel;
        blocks.push(Block {
            content: &source[content_start..content_end],
            start_byte: content_start,
            tsx: script_is_tsx(attr),
        });
        search = content_end + "</script".len();
    }
    blocks
}

/// Astro leading frontmatter fenced by `---` lines. The fence must be the first
/// meaningful content (leading blank lines allowed). Frontmatter is pure TS.
fn locate_frontmatter(source: &str) -> Vec<Block<'_>> {
    let start = if source.as_bytes().starts_with(&[0xEF, 0xBB, 0xBF]) {
        3
    } else {
        0
    };
    let mut line_start = start;
    let mut content_start: Option<usize> = None;
    for line in source[start..].split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']).trim();
        match content_start {
            None => {
                if trimmed == "---" {
                    content_start = Some(line_start + line.len());
                } else if !trimmed.is_empty() {
                    return Vec::new(); // no frontmatter at the top of the file
                }
            }
            Some(cs) => {
                if trimmed == "---" {
                    return vec![Block {
                        content: &source[cs..line_start],
                        start_byte: cs,
                        tsx: false,
                    }];
                }
            }
        }
        line_start += line.len();
    }
    Vec::new()
}

/// Pick the grammar variant from the `<script>` attributes. `lang="ts"` uses the
/// TS grammar; anything mentioning `tsx`, plain JS, or no `lang` uses the
/// TSX-capable grammar (mirrors `parser_for_ext`'s js→TSX mapping).
fn script_is_tsx(attr: &str) -> bool {
    let a = attr.to_ascii_lowercase();
    if a.contains("tsx") {
        return true;
    }
    if a.contains("lang=\"ts\"") || a.contains("lang='ts'") || a.contains("lang=ts") {
        return false;
    }
    true
}
