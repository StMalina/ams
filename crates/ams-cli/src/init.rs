//! `ams init` — register the AMS workflow with the user's coding agents.
//!
//! Supported agents:
//!   claude  — slim `~/.claude/AMS.md` + one `@AMS.md` import in CLAUDE.md
//!   codex   — marker-guarded inline block in `~/.codex/AGENTS.md`
//!   gemini  — marker-guarded inline block in `~/.gemini/GEMINI.md`
//!
//! Selection: `--agents claude,codex` explicit; `--agents all`; default is
//! interactive pick over /dev/tty (works under `curl | sh`), falling back to
//! auto-detection (config dir exists) when no terminal is available.
//!
//! Every write is backup (`.bak`) + temp-file + rename. Idempotent: a second
//! run changes nothing and says so.

use anyhow::{bail, Context, Result};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

const AMS_MD: &str = include_str!("../assets/AMS.md");
const IMPORT_LINE: &str = "@AMS.md";
const BLOCK_START: &str = "<!-- ams:start -->";
const BLOCK_END: &str = "<!-- ams:end -->";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Agent {
    Claude,
    Codex,
    Gemini,
}

const ALL_AGENTS: [Agent; 3] = [Agent::Claude, Agent::Codex, Agent::Gemini];

impl Agent {
    fn name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Gemini => "gemini",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex CLI",
            Agent::Gemini => "Gemini CLI",
        }
    }

    fn config_dir(self) -> Result<PathBuf> {
        if self == Agent::Claude {
            if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
                if !dir.is_empty() {
                    return Ok(PathBuf::from(dir));
                }
            }
        }
        let home = std::env::var("HOME").context("HOME is not set")?;
        let sub = match self {
            Agent::Claude => ".claude",
            Agent::Codex => ".codex",
            Agent::Gemini => ".gemini",
        };
        Ok(Path::new(&home).join(sub))
    }

    /// The instructions file this agent loads globally.
    fn memory_file(self) -> Result<PathBuf> {
        let dir = self.config_dir()?;
        Ok(match self {
            Agent::Claude => dir.join("CLAUDE.md"),
            Agent::Codex => dir.join("AGENTS.md"),
            Agent::Gemini => dir.join("GEMINI.md"),
        })
    }

    fn detected(self) -> bool {
        self.config_dir().map(|d| d.is_dir()).unwrap_or(false)
    }
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

fn backup(path: &Path) -> Result<bool> {
    if path.exists() {
        fs::copy(path, path.with_extension("md.bak"))
            .with_context(|| format!("backing up {}", path.display()))?;
        return Ok(true);
    }
    Ok(false)
}

fn has_import(content: &str) -> bool {
    content.lines().any(|l| l.trim() == IMPORT_LINE)
}

fn marker_block() -> String {
    format!("{BLOCK_START}\n{}{BLOCK_END}\n", AMS_MD)
}

/// Locate a marker block. Ok(None) = absent. Err = damaged markers.
fn find_block(content: &str, file: &Path) -> Result<Option<(usize, usize)>> {
    let Some(start) = content.find(BLOCK_START) else {
        if content.contains(BLOCK_END) {
            bail!(
                "{} has {BLOCK_END} without {BLOCK_START} — fix it manually, refusing to edit",
                file.display()
            );
        }
        return Ok(None);
    };
    let Some(end) = content[start..]
        .find(BLOCK_END)
        .map(|i| start + i + BLOCK_END.len())
    else {
        bail!(
            "{} has {BLOCK_START} without {BLOCK_END} — fix it manually, refusing to edit",
            file.display()
        );
    };
    // Include one trailing newline in the block span.
    let end = if content[end..].starts_with('\n') { end + 1 } else { end };
    Ok(Some((start, end)))
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

fn write_if_changed(path: &Path, current: &str, new: String, what: &str) -> Result<()> {
    if new == current {
        println!("[ok] {} {what} already in place", path.display());
        return Ok(());
    }
    let backed_up = backup(path)?;
    atomic_write(path, &new)?;
    let suffix = if backed_up {
        format!(" (backup: {}.bak)", path.file_name().unwrap_or_default().to_string_lossy())
    } else {
        String::new()
    };
    println!("[ok] {} {what} written{suffix}", path.display());
    Ok(())
}

fn install_claude() -> Result<()> {
    let dir = Agent::Claude.config_dir()?;
    let ams_md = dir.join("AMS.md");
    let claude_md = Agent::Claude.memory_file()?;

    let ams_md_current = fs::read_to_string(&ams_md).unwrap_or_default();
    if ams_md_current == AMS_MD {
        println!("[ok] {} up to date", ams_md.display());
    } else {
        atomic_write(&ams_md, AMS_MD)?;
        println!("[ok] {} written", ams_md.display());
    }

    let current = fs::read_to_string(&claude_md).unwrap_or_default();
    let mut content = current.clone();
    // Migrate a legacy inline block to the slim import.
    if let Some((s, e)) = find_block(&content, &claude_md)? {
        content = clean_double_blanks(&format!("{}{}", &content[..s], &content[e..]));
    }
    if !has_import(&content) {
        // First line, so the workflow is loaded before project-specific rules.
        content = format!("{IMPORT_LINE}\n{content}");
    }
    write_if_changed(&claude_md, &current, content, IMPORT_LINE)
}

fn install_block(agent: Agent) -> Result<()> {
    let file = agent.memory_file()?;
    let current = fs::read_to_string(&file).unwrap_or_default();
    let block = marker_block();
    let content = match find_block(&current, &file)? {
        Some((s, e)) => format!("{}{}{}", &current[..s], block, &current[e..]),
        None if current.is_empty() => block,
        None => {
            let sep = if current.ends_with("\n\n") {
                ""
            } else if current.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            };
            format!("{current}{sep}{block}")
        }
    };
    write_if_changed(&file, &current, content, "ams block")
}

fn uninstall_agent(agent: Agent) -> Result<()> {
    let file = agent.memory_file()?;
    if let Ok(current) = fs::read_to_string(&file) {
        let mut content = match find_block(&current, &file)? {
            Some((s, e)) => format!("{}{}", &current[..s], &current[e..]),
            None => current.clone(),
        };
        if agent == Agent::Claude {
            content = content
                .lines()
                .filter(|l| l.trim() != IMPORT_LINE)
                .map(|l| format!("{l}\n"))
                .collect();
        }
        content = clean_double_blanks(&content);
        if content != current {
            backup(&file)?;
            atomic_write(&file, &content)?;
            println!("[ok] {} cleaned (backup kept)", file.display());
        }
    }
    if agent == Agent::Claude {
        let ams_md = Agent::Claude.config_dir()?.join("AMS.md");
        if ams_md.exists() {
            fs::remove_file(&ams_md)?;
            println!("[ok] {} removed", ams_md.display());
        }
    }
    Ok(())
}

fn status_agent(agent: Agent) -> Result<()> {
    let ok = |b: bool| if b { "[ok]" } else { "[--]" };
    let file = agent.memory_file()?;
    let content = fs::read_to_string(&file).unwrap_or_default();
    let registered = if agent == Agent::Claude {
        let ams_md = Agent::Claude.config_dir()?.join("AMS.md");
        let slim = fs::read_to_string(&ams_md).unwrap_or_default();
        has_import(&content) && slim == AMS_MD
    } else {
        find_block(&content, &file)
            .map(|b| {
                b.map(|(s, e)| content[s..e].contains(AMS_MD.trim()))
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    };
    println!(
        "{} {} ({}): {}",
        ok(registered),
        agent.label(),
        file.display(),
        match (registered, agent.detected()) {
            (true, _) => "registered".to_string(),
            (false, true) => format!("not registered — run `ams init --agents {}`", agent.name()),
            (false, false) => "not detected".to_string(),
        }
    );
    Ok(())
}

fn parse_agents(spec: &str) -> Result<Vec<Agent>> {
    if spec == "all" {
        return Ok(ALL_AGENTS.to_vec());
    }
    if spec == "auto" {
        return Ok(ALL_AGENTS.iter().copied().filter(|a| a.detected()).collect());
    }
    let mut out = Vec::new();
    for part in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let agent = ALL_AGENTS
            .iter()
            .copied()
            .find(|a| a.name() == part)
            .ok_or_else(|| anyhow::anyhow!("unknown agent '{part}' (known: claude, codex, gemini, all, auto)"))?;
        if !out.contains(&agent) {
            out.push(agent);
        }
    }
    Ok(out)
}

/// Interactive pick over /dev/tty (survives `curl | sh`); None when no tty.
fn tty_select(detected: &[Agent]) -> Option<Vec<Agent>> {
    let mut tty_in = BufReader::new(fs::File::open("/dev/tty").ok()?);
    let mut tty_out = fs::OpenOptions::new().write(true).open("/dev/tty").ok()?;

    let default: String = detected
        .iter()
        .map(|a| a.name())
        .collect::<Vec<_>>()
        .join(",");
    let default = if default.is_empty() { "none".to_string() } else { default };

    let _ = writeln!(tty_out, "\nRegister the AMS workflow for which agents?");
    for a in ALL_AGENTS {
        let _ = writeln!(
            tty_out,
            "  - {:<7} {} {}",
            a.name(),
            a.label(),
            if a.detected() { "(detected)" } else { "" }
        );
    }
    let _ = write!(
        tty_out,
        "Enter names (comma-separated), 'all' or 'none' [{default}]: "
    );
    let _ = tty_out.flush();

    let mut line = String::new();
    tty_in.read_line(&mut line).ok()?;
    let answer = line.trim();
    let answer = if answer.is_empty() { default.as_str() } else { answer };
    if answer == "none" {
        return Some(Vec::new());
    }
    match parse_agents(answer) {
        Ok(agents) => Some(agents),
        Err(e) => {
            let _ = writeln!(tty_out, "ams: {e}; nothing registered");
            Some(Vec::new())
        }
    }
}

pub fn run(show: bool, uninstall: bool, agents: Option<String>) -> Result<()> {
    if show {
        for a in ALL_AGENTS {
            status_agent(a)?;
        }
        return Ok(());
    }
    if uninstall {
        let targets = match agents.as_deref() {
            Some(spec) => parse_agents(spec)?,
            None => ALL_AGENTS.to_vec(),
        };
        for a in targets {
            uninstall_agent(a)?;
        }
        return Ok(());
    }

    let targets = match agents.as_deref() {
        Some(spec) => parse_agents(spec)?,
        None => {
            let detected: Vec<Agent> =
                ALL_AGENTS.iter().copied().filter(|a| a.detected()).collect();
            match tty_select(&detected) {
                Some(picked) => picked,
                None => {
                    // Non-interactive (curl | sh without tty, CI): detected only.
                    println!(
                        "no terminal — registering for detected agents: {}",
                        if detected.is_empty() {
                            "none".to_string()
                        } else {
                            detected.iter().map(|a| a.label()).collect::<Vec<_>>().join(", ")
                        }
                    );
                    detected
                }
            }
        }
    };

    if targets.is_empty() {
        println!("nothing registered; run `ams init --agents claude,codex,gemini` anytime");
        return Ok(());
    }

    for a in &targets {
        match a {
            Agent::Claude => install_claude()?,
            _ => install_block(*a)?,
        }
    }
    println!(
        "\nDone. Undo anytime: ams init --uninstall; status: ams init --show"
    );
    Ok(())
}
