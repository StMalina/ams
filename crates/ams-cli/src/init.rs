//! `ams init` — register the AMS workflow with the user's coding agents.
//!
//! Three mechanisms, one per agent:
//!   import — Claude Code: slim `~/.claude/AMS.md` + one `@AMS.md` line in CLAUDE.md
//!   block  — marker-guarded inline block in the agent's global instructions file
//!            (Codex, Gemini, Copilot CLI, Windsurf, OpenCode, OpenClaw, Pi,
//!             Antigravity — the latter shares Gemini's ~/.gemini/GEMINI.md)
//!   file   — a dedicated file ams fully owns inside the agent's rules directory
//!            (Cline, Roo Code, Kilo Code, Copilot in VS Code)
//!
//! Selection: `--agents claude,codex,...` explicit; `--agents all`; default is
//! an interactive checkbox picker over /dev/tty (works under `curl | sh`),
//! falling back to a line prompt on exotic terminals and to auto-detection
//! (config dir exists) when no terminal is available.
//!
//! Every edit of a shared file is backup (`.bak`) + temp-file + rename; owned
//! files are simply written/removed. Idempotent: a second run changes nothing.
//!
//! Picking Claude Code also installs the Claude Code plugin (guards + skill)
//! through the `claude plugin` CLI when it is on PATH — fail-soft, opt out
//! with AMS_NO_PLUGIN=1.

use anyhow::{bail, Context, Result};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
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
    Copilot,
    CopilotVscode,
    Windsurf,
    Cline,
    Roo,
    Kilo,
    Opencode,
    Openclaw,
    Pi,
    Antigravity,
}

#[derive(Clone, Copy, PartialEq)]
enum Mech {
    Import,
    Block,
    OwnFile,
}

const ALL_AGENTS: [Agent; 13] = [
    Agent::Claude,
    Agent::Codex,
    Agent::Gemini,
    Agent::Copilot,
    Agent::CopilotVscode,
    Agent::Windsurf,
    Agent::Cline,
    Agent::Roo,
    Agent::Kilo,
    Agent::Opencode,
    Agent::Openclaw,
    Agent::Pi,
    Agent::Antigravity,
];

fn home() -> Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

fn xdg_config() -> Result<PathBuf> {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return Ok(PathBuf::from(x));
        }
    }
    Ok(home()?.join(".config"))
}

/// User dir of the first VS Code variant found (Code, Insiders, VSCodium);
/// defaults to plain Code when none is installed yet.
fn vscode_user_dir() -> Result<PathBuf> {
    let base = if cfg!(target_os = "macos") {
        home()?.join("Library/Application Support")
    } else {
        xdg_config()?
    };
    for variant in ["Code", "Code - Insiders", "VSCodium"] {
        let user = base.join(variant).join("User");
        if user.is_dir() {
            return Ok(user);
        }
    }
    Ok(base.join("Code/User"))
}

/// Cline keeps global rules in ~/Documents/Cline/Rules; some Linux/WSL
/// installs use ~/Cline/Rules instead — prefer whichever already exists.
fn cline_rules_dir() -> Result<PathBuf> {
    let alt = home()?.join("Cline/Rules");
    if alt.is_dir() {
        return Ok(alt);
    }
    Ok(home()?.join("Documents/Cline/Rules"))
}

impl Agent {
    fn name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Gemini => "gemini",
            Agent::Copilot => "copilot",
            Agent::CopilotVscode => "copilot-vscode",
            Agent::Windsurf => "windsurf",
            Agent::Cline => "cline",
            Agent::Roo => "roo",
            Agent::Kilo => "kilo",
            Agent::Opencode => "opencode",
            Agent::Openclaw => "openclaw",
            Agent::Pi => "pi",
            Agent::Antigravity => "antigravity",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex CLI",
            Agent::Gemini => "Gemini CLI",
            Agent::Copilot => "GitHub Copilot CLI",
            Agent::CopilotVscode => "GitHub Copilot (VS Code)",
            Agent::Windsurf => "Windsurf",
            Agent::Cline => "Cline",
            Agent::Roo => "Roo Code",
            Agent::Kilo => "Kilo Code",
            Agent::Opencode => "OpenCode",
            Agent::Openclaw => "OpenClaw",
            Agent::Pi => "Pi",
            Agent::Antigravity => "Google Antigravity",
        }
    }

    fn mech(self) -> Mech {
        match self {
            Agent::Claude => Mech::Import,
            Agent::CopilotVscode | Agent::Cline | Agent::Roo | Agent::Kilo => Mech::OwnFile,
            _ => Mech::Block,
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
        Ok(match self {
            Agent::Claude => home()?.join(".claude"),
            Agent::Codex => home()?.join(".codex"),
            Agent::Gemini => home()?.join(".gemini"),
            Agent::Copilot => home()?.join(".copilot"),
            Agent::CopilotVscode => vscode_user_dir()?,
            Agent::Windsurf => home()?.join(".codeium/windsurf"),
            Agent::Cline => cline_rules_dir()?,
            Agent::Roo => home()?.join(".roo"),
            Agent::Kilo => home()?.join(".kilocode"),
            Agent::Opencode => xdg_config()?.join("opencode"),
            Agent::Openclaw => home()?.join(".openclaw"),
            Agent::Pi => home()?.join(".pi"),
            Agent::Antigravity => home()?.join(".gemini/antigravity"),
        })
    }

    /// The global instructions file this agent loads (the one we edit or own).
    fn memory_file(self) -> Result<PathBuf> {
        let dir = self.config_dir()?;
        Ok(match self {
            Agent::Claude => dir.join("CLAUDE.md"),
            Agent::Codex => dir.join("AGENTS.md"),
            Agent::Gemini => dir.join("GEMINI.md"),
            Agent::Copilot => dir.join("copilot-instructions.md"),
            Agent::CopilotVscode => dir.join("prompts/ams.instructions.md"),
            Agent::Windsurf => dir.join("memories/global_rules.md"),
            Agent::Cline => dir.join("ams.md"),
            Agent::Roo => dir.join("rules/ams.md"),
            Agent::Kilo => dir.join("rules/ams.md"),
            Agent::Opencode => dir.join("AGENTS.md"),
            Agent::Openclaw => dir.join("workspace/AGENTS.md"),
            Agent::Pi => dir.join("agent/AGENTS.md"),
            // Antigravity reads the same global file as Gemini CLI.
            Agent::Antigravity => home()?.join(".gemini/GEMINI.md"),
        })
    }

    /// Content of the dedicated file for OwnFile agents.
    fn own_file_content(self) -> String {
        match self {
            // VS Code instructions files need frontmatter to apply globally.
            Agent::CopilotVscode => {
                format!("---\napplyTo: '**'\ndescription: AMS — code navigation via signatures\n---\n\n{AMS_MD}")
            }
            _ => AMS_MD.to_string(),
        }
    }

    fn detected(self) -> bool {
        match self {
            Agent::Cline => {
                let ok = |p: &str| home().map(|h| h.join(p).is_dir()).unwrap_or(false);
                ok("Documents/Cline") || ok("Cline")
            }
            _ => self.config_dir().map(|d| d.is_dir()).unwrap_or(false),
        }
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

fn install_own_file(agent: Agent) -> Result<()> {
    let file = agent.memory_file()?;
    let content = agent.own_file_content();
    if fs::read_to_string(&file).unwrap_or_default() == content {
        println!("[ok] {} up to date", file.display());
        return Ok(());
    }
    atomic_write(&file, &content)?;
    println!("[ok] {} written", file.display());
    Ok(())
}

fn uninstall_agent(agent: Agent) -> Result<()> {
    match agent.mech() {
        Mech::OwnFile => {
            let file = agent.memory_file()?;
            if file.exists() {
                fs::remove_file(&file)?;
                println!("[ok] {} removed", file.display());
            }
            return Ok(());
        }
        Mech::Import | Mech::Block => {}
    }
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
    let registered = match agent.mech() {
        Mech::Import => {
            let ams_md = Agent::Claude.config_dir()?.join("AMS.md");
            let slim = fs::read_to_string(&ams_md).unwrap_or_default();
            has_import(&content) && slim == AMS_MD
        }
        Mech::OwnFile => content == agent.own_file_content(),
        Mech::Block => find_block(&content, &file)
            .map(|b| {
                b.map(|(s, e)| content[s..e].contains(AMS_MD.trim()))
                    .unwrap_or(false)
            })
            .unwrap_or(false),
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
            .ok_or_else(|| {
                let known: Vec<&str> = ALL_AGENTS.iter().map(|a| a.name()).collect();
                anyhow::anyhow!("unknown agent '{part}' (known: {}, all, auto)", known.join(", "))
            })?;
        if !out.contains(&agent) {
            out.push(agent);
        }
    }
    Ok(out)
}

/// Interactive pick over /dev/tty (survives `curl | sh`); None when no tty.
/// Checkbox UI on real terminals, line prompt as fallback.
fn tty_select(detected: &[Agent]) -> Option<Vec<Agent>> {
    #[cfg(unix)]
    if let Some(picked) = checkbox_select(detected) {
        return Some(picked);
    }
    line_select(detected)
}

#[cfg(unix)]
mod raw_tty {
    use std::os::unix::io::RawFd;

    /// Puts the fd into no-echo/no-canonical mode; restores (and re-shows the
    /// cursor) on drop, even on panic or early return.
    pub struct RawMode {
        fd: RawFd,
        orig: libc::termios,
    }

    impl RawMode {
        pub fn enable(fd: RawFd) -> Option<Self> {
            let mut orig: libc::termios = unsafe { std::mem::zeroed() };
            if unsafe { libc::tcgetattr(fd, &mut orig) } != 0 {
                return None;
            }
            let mut raw = orig;
            raw.c_lflag &= !(libc::ICANON | libc::ECHO);
            raw.c_cc[libc::VMIN] = 1;
            raw.c_cc[libc::VTIME] = 0;
            if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
                return None;
            }
            Some(RawMode { fd, orig })
        }
    }

    impl Drop for RawMode {
        fn drop(&mut self) {
            const SHOW_CURSOR: &[u8] = b"\x1b[?25h";
            unsafe {
                libc::write(
                    self.fd,
                    SHOW_CURSOR.as_ptr() as *const libc::c_void,
                    SHOW_CURSOR.len(),
                );
                libc::tcsetattr(self.fd, libc::TCSANOW, &self.orig);
            }
        }
    }
}

/// Arrow/space checkbox picker. None = raw mode unavailable, use the fallback.
#[cfg(unix)]
fn checkbox_select(detected: &[Agent]) -> Option<Vec<Agent>> {
    use std::os::unix::io::AsRawFd;

    let mut tty = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    let _raw = raw_tty::RawMode::enable(tty.as_raw_fd())?;

    let mut checked: Vec<bool> = ALL_AGENTS.iter().map(|a| detected.contains(a)).collect();
    let mut cursor = 0usize;
    let n = ALL_AGENTS.len();

    let _ = write!(
        tty,
        "\r\nRegister the AMS workflow for which agents?\r\n\
         \x1b[2m  ↑/↓ move · space toggle · a all · n none · enter confirm · q skip\x1b[0m\r\n\x1b[?25l"
    );
    let draw = |tty: &mut fs::File, checked: &[bool], cursor: usize, first: bool| {
        let mut out = String::new();
        if !first {
            out.push_str(&format!("\x1b[{n}A"));
        }
        for (i, a) in ALL_AGENTS.iter().enumerate() {
            out.push_str(&format!(
                "\r\x1b[2K{} [{}] {}{}\r\n",
                if i == cursor { ">" } else { " " },
                if checked[i] { "x" } else { " " },
                a.label(),
                if a.detected() { " \x1b[2m(detected)\x1b[0m" } else { "" },
            ));
        }
        let _ = tty.write_all(out.as_bytes());
        let _ = tty.flush();
    };
    draw(&mut tty, &checked, cursor, true);

    let mut byte = [0u8; 1];
    loop {
        if tty.read_exact(&mut byte).is_err() {
            return Some(Vec::new());
        }
        match byte[0] {
            b' ' => checked[cursor] = !checked[cursor],
            b'a' => checked.iter_mut().for_each(|c| *c = true),
            b'n' => checked.iter_mut().for_each(|c| *c = false),
            b'j' => cursor = (cursor + 1) % n,
            b'k' => cursor = (cursor + n - 1) % n,
            b'\r' | b'\n' => break,
            b'q' | 0x03 => {
                // q / Ctrl-C: register nothing.
                checked.iter_mut().for_each(|c| *c = false);
                break;
            }
            0x1b => {
                // ESC [ A/B — arrow keys.
                let mut seq = [0u8; 2];
                if tty.read_exact(&mut seq).is_ok() && seq[0] == b'[' {
                    match seq[1] {
                        b'A' => cursor = (cursor + n - 1) % n,
                        b'B' => cursor = (cursor + 1) % n,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        draw(&mut tty, &checked, cursor, false);
    }
    let _ = write!(tty, "\r\n");

    Some(
        ALL_AGENTS
            .iter()
            .zip(&checked)
            .filter(|(_, c)| **c)
            .map(|(a, _)| *a)
            .collect(),
    )
}

/// Plain line prompt — for terminals where raw mode fails.
fn line_select(detected: &[Agent]) -> Option<Vec<Agent>> {
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
            "  - {:<15} {} {}",
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

/// Claude Code plugin (guards + skill) — managed through the official
/// `claude plugin` CLI so updates keep flowing through the marketplace.
/// Both commands are idempotent; everything here is fail-soft: no claude
/// binary or a failed install degrades to a printed hint, never an error.
fn claude_plugin(install: bool) {
    if std::env::var("AMS_NO_PLUGIN").ok().as_deref() == Some("1") {
        return;
    }
    let run = |args: &[&str]| {
        std::process::Command::new("claude")
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
    };
    if install {
        let hint = || {
            println!(
                "note: Claude Code plugin (guards + skill) not installed — from Claude Code run:\n      /plugin marketplace add StMalina/ams\n      /plugin install ams@ams"
            );
        };
        // A failed add is fine (e.g. a marketplace named `ams` already
        // exists, pointing elsewhere) — install below settles it.
        match run(&["plugin", "marketplace", "add", "StMalina/ams"]) {
            Err(_) => hint(), // no claude CLI on PATH
            Ok(_) => match run(&["plugin", "install", "ams@ams"]) {
                Ok(out) if out.status.success() => {
                    println!("[ok] Claude Code plugin installed (guards + skill)")
                }
                _ => hint(),
            },
        }
    } else if let Ok(out) = run(&["plugin", "uninstall", "ams@ams"]) {
        if out.status.success() {
            let _ = run(&["plugin", "marketplace", "remove", "ams"]);
            println!("[ok] Claude Code plugin uninstalled");
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
        for a in &targets {
            uninstall_agent(*a)?;
        }
        if targets.contains(&Agent::Claude) {
            claude_plugin(false);
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
        println!("nothing registered; run `ams init --agents claude,codex,...` anytime (see --help)");
        return Ok(());
    }

    for a in &targets {
        match a.mech() {
            Mech::Import => install_claude()?,
            Mech::Block => install_block(*a)?,
            Mech::OwnFile => install_own_file(*a)?,
        }
    }
    // Picking Claude Code means the whole Claude Code setup — the plugin's
    // guards and skill are what make agents actually use ams.
    if targets.contains(&Agent::Claude) {
        claude_plugin(true);
    }
    println!(
        "\nDone. Undo anytime: ams init --uninstall; status: ams init --show"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_agents_names_and_aliases() {
        let picked = parse_agents("claude, roo,kilo").unwrap();
        assert_eq!(picked, vec![Agent::Claude, Agent::Roo, Agent::Kilo]);
        assert_eq!(parse_agents("all").unwrap().len(), ALL_AGENTS.len());
        assert!(parse_agents("cursor").is_err());
    }

    #[test]
    fn own_file_content_has_frontmatter_only_for_vscode() {
        assert!(Agent::CopilotVscode.own_file_content().starts_with("---\napplyTo: '**'"));
        assert_eq!(Agent::Roo.own_file_content(), AMS_MD);
    }

    #[test]
    fn antigravity_shares_gemini_file() {
        std::env::set_var("HOME", "/tmp/ams-test-home");
        assert_eq!(
            Agent::Antigravity.memory_file().unwrap(),
            Agent::Gemini.memory_file().unwrap()
        );
    }
}
