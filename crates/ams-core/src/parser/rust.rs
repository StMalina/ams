// TODO: implemented by subagent following typescript.rs as the reference.
use super::LangParser;
use crate::model::ParsedFile;
use anyhow::Result;

pub struct RustParser;

impl LangParser for RustParser {
    fn lang_id(&self) -> &'static str {
        "rs"
    }

    fn parse(&self, source: &str) -> Result<ParsedFile> {
        Ok(ParsedFile {
            loc: super::count_loc(source),
            ..Default::default()
        })
    }
}
