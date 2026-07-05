//! `ams init` — register the AMS workflow in the user's Claude Code setup.
//!
//! Model (borrowed from rtk): a slim instructions file `~/.claude/AMS.md`
//! owned by ams, plus a single `@AMS.md` import line in `~/.claude/CLAUDE.md`.
//! Legacy installs injected a full `<!-- ams:start -->..<!-- ams:end -->`
//! block straight into CLAUDE.md; init migrates that to the import line.
//!
//! Every write is backup (`.bak`) + temp-file + rename. Idempotent: a second
//! run changes nothing and says so.

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

const AMS_MD: &str = include_str!("../assets/AMS.md");
const IMPORT_LINE: &str = "@AMS.md";
const LEGACY_START: &str = "<!-- ams:start -->";
const LEGACY_END: &str = "<!-- ams:end -->";

fn claude_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(Path::new(&home).join(".claude"))
}

/// Write via temp file + rename so a mid-write failure never corrupts the target.
fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let dir = path.parent().context("target has no parent directory")?;
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    fs::write(&tmp, content).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}

fn backup(path: &Path) -> Result<()> {
    if path.exists() {
        fs::copy(path, path.with_extension("md.bak"))
            .with_context(|| format!("backing up {}", path.display()))?;
    }
    Ok(())
}

fn has_import(content: &str) -> bool {
    content.lines().any(|l| l.trim() == IMPORT_LINE)
}

/// Strip a legacy marker block. Ok(None) = no block. Err = damaged markers.
fn strip_legacy(content: &str) -> Result<Option<String>> {
    let Some(start) = content.find(LEGACY_START) else {
        if content.contains(LEGACY_END) {
            bail!("CLAUDE.md has {LEGACY_END} without {LEGACY_START} — fix it manually, refusing to edit");
        }
        return Ok(None);
    };
    let Some(end) = content[start..].find(LEGACY_END).map(|i| start + i + LEGACY_END.len()) else {
        bail!("CLAUDE.md has {LEGACY_START} without {LEGACY_END} — fix it manually, refusing to edit");
    };
    Ok(Some(format!("{}{}", &content[..start], &content[end..])))
}

fn clean_double_blanks(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut blanks = 0;
    for line in content.lines() {
        if line.trim().is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

pub fn run(show: bool, uninstall: bool) -> Result<()> {
    let dir = claude_dir()?;
    let ams_md = dir.join("AMS.md");
    let claude_md = dir.join("CLAUDE.md");

    if show {
        return status(&ams_md, &claude_md);
    }
    if uninstall {
        return remove(&ams_md, &claude_md);
    }

    // 1. AMS.md — canonical content shipped inside the binary.
    let ams_md_current = fs::read_to_string(&ams_md).unwrap_or_default();
    if ams_md_current == AMS_MD {
        println!("[ok] {} up to date", ams_md.display());
    } else {
        atomic_write(&ams_md, AMS_MD)?;
        println!("[ok] {} written", ams_md.display());
    }

    // 2. CLAUDE.md — ensure the @AMS.md import line, migrating the legacy block.
    let current = fs::read_to_string(&claude_md).unwrap_or_default();
    let mut content = current.clone();
    let mut migrated = false;
    if let Some(stripped) = strip_legacy(&content)? {
        content = stripped;
        migrated = true;
    }
    if !has_import(&content) {
        // First line, so the workflow is loaded before project-specific rules.
        content = format!("{IMPORT_LINE}\n{content}");
    }
    if migrated {
        content = clean_double_blanks(&content);
    }
    if content == current {
        println!("[ok] {} already imports {IMPORT_LINE}", claude_md.display());
    } else {
        let backed_up = claude_md.exists();
        backup(&claude_md)?;
        atomic_write(&claude_md, &content)?;
        let suffix = if backed_up { " (backup: CLAUDE.md.bak)" } else { "" };
        if migrated {
            println!(
                "[ok] {} migrated: legacy ams block replaced with {IMPORT_LINE}{suffix}",
                claude_md.display()
            );
        } else {
            println!(
                "[ok] {} now imports {IMPORT_LINE}{suffix}",
                claude_md.display()
            );
        }
    }

    println!(
        "\nDone. Claude Code will load the AMS workflow in every project.\n\
         Undo anytime: ams init --uninstall"
    );
    Ok(())
}

fn status(ams_md: &Path, claude_md: &Path) -> Result<()> {
    let ok = |b: bool| if b { "[ok]" } else { "[--]" };

    let ams_md_current = fs::read_to_string(ams_md).unwrap_or_default();
    let up_to_date = ams_md_current == AMS_MD;
    println!(
        "{} AMS.md: {} ({})",
        ok(!ams_md_current.is_empty()),
        ams_md.display(),
        if up_to_date {
            "up to date"
        } else if ams_md_current.is_empty() {
            "missing — run `ams init`"
        } else {
            "outdated — run `ams init` to refresh"
        }
    );

    let content = fs::read_to_string(claude_md).unwrap_or_default();
    println!(
        "{} CLAUDE.md: {} import in {}",
        ok(has_import(&content)),
        IMPORT_LINE,
        claude_md.display()
    );
    if content.contains(LEGACY_START) || content.contains(LEGACY_END) {
        println!("[!!] CLAUDE.md still contains a legacy ams marker block — run `ams init` to migrate");
    }
    Ok(())
}

fn remove(ams_md: &Path, claude_md: &Path) -> Result<()> {
    if let Ok(current) = fs::read_to_string(claude_md) {
        let mut content = strip_legacy(&current)?.unwrap_or(current.clone());
        content = content
            .lines()
            .filter(|l| l.trim() != IMPORT_LINE)
            .map(|l| format!("{l}\n"))
            .collect();
        content = clean_double_blanks(&content);
        if content != current {
            backup(claude_md)?;
            atomic_write(claude_md, &content)?;
            println!("[ok] {} cleaned (backup: CLAUDE.md.bak)", claude_md.display());
        } else {
            println!("[ok] {}: nothing to remove", claude_md.display());
        }
    }
    if ams_md.exists() {
        fs::remove_file(ams_md)?;
        println!("[ok] {} removed", ams_md.display());
    } else {
        println!("[ok] {}: already absent", ams_md.display());
    }
    Ok(())
}
