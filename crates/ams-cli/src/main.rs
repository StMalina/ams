mod format;
mod init;

use ams_core::model::SymbolKind;
use ams_core::Index;
use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "ams",
    version,
    about = "AI Module Signatures — compact code index for AI agents.\n\
             Read signatures instead of files; open sources only at the exact line spans."
)]
struct Cli {
    /// Emit JSON instead of compact text
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Index a project (creates .ams/index.db at the given root)
    Build {
        /// Project root (default: current directory)
        path: Option<PathBuf>,
        /// Reparse everything, ignoring mtime/hash fast paths
        #[arg(long)]
        force: bool,
    },
    /// Show signatures of a file or directory, with @start-end line spans
    Describe {
        /// File or directory paths
        paths: Vec<String>,
        /// Only exported symbols
        #[arg(long)]
        exported: bool,
    },
    /// One-line-per-file overview of the project or a directory
    Tree {
        dir: Option<String>,
        /// Aggregate by directory at this depth (0 = flat file list)
        #[arg(long)]
        depth: Option<usize>,
        /// Show only the top files by reverse-dependency count
        #[arg(long)]
        hubs: bool,
    },
    /// Full-text search over names, signatures, and docs ("find by meaning")
    Search {
        /// Words to search for (AND-ed)
        terms: Vec<String>,
    },
    /// Find symbol definitions by (sub)name
    Find {
        name: String,
        /// Filter by kind: fn|method|class|struct|enum|trait|interface|const|type|mod
        #[arg(long)]
        kind: Option<String>,
        /// Only exported symbols
        #[arg(long)]
        exported: bool,
    },
    /// Find usages (call sites) of a symbol name
    Refs {
        name: String,
        /// Only usages under this directory (narrows common names)
        #[arg(long = "in", value_name = "DIR")]
        in_dir: Option<String>,
    },
    /// Dependencies and reverse dependencies of a file
    Related {
        file: String,
    },
    /// Register the AMS workflow in Claude Code (~/.claude/AMS.md + @AMS.md import)
    Init {
        /// Print current registration status without changing anything
        #[arg(long)]
        show: bool,
        /// Remove the AMS.md file and the @AMS.md import from CLAUDE.md
        #[arg(long)]
        uninstall: bool,
    },
    /// Token savings so far: per-command output size vs covered source size
    Gain,
    /// Attach a doc note to a symbol: ams annotate src/auth.ts:AuthService.login "..."
    Annotate {
        /// Target as <file>:<Symbol.path>
        target: String,
        doc: String,
    },
}

fn main() {
    // Die quietly on `ams ... | head` instead of panicking on broken pipe.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    if let Err(e) = run() {
        eprintln!("ams: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    if let Cmd::Init { show, uninstall } = &cli.cmd {
        return init::run(*show, *uninstall);
    }

    if let Cmd::Build { path, force } = &cli.cmd {
        let root = path.clone().unwrap_or(std::env::current_dir()?);
        let mut idx = Index::create(&root)?;
        if *force {
            idx.clear_files()?;
        }
        let stats = idx.sync()?;
        if cli.json {
            println!(
                "{}",
                serde_json::json!({
                    "root": idx.root, "files": stats.total,
                    "parsed": stats.parsed, "removed": stats.removed
                })
            );
        } else {
            println!(
                "indexed {} files (parsed {}, removed {}) -> {}/.ams/index.db",
                stats.total,
                stats.parsed,
                stats.removed,
                idx.root.display()
            );
        }
        return Ok(());
    }

    // Every query self-heals: cheap stat-walk, reparse only what changed.
    let mut idx = Index::open_existing(&std::env::current_dir()?)?;
    idx.sync()?;

    match cli.cmd {
        Cmd::Build { .. } | Cmd::Init { .. } => unreachable!(),
        Cmd::Describe { paths, exported } => {
            if paths.is_empty() {
                return Err(anyhow!("usage: ams describe <file|dir>..."));
            }
            let mut descriptions = Vec::new();
            for p in &paths {
                let rel = idx.rel_path(p)?;
                let abs = idx.root.join(&rel);
                if abs.is_dir() {
                    for f in idx.files_under(if rel.is_empty() { None } else { Some(&rel) })? {
                        descriptions.push(idx.describe(&f)?);
                    }
                } else {
                    descriptions.push(idx.describe(&rel)?);
                }
            }
            let out = if cli.json {
                format!("{}\n", serde_json::to_string_pretty(&descriptions)?)
            } else {
                descriptions
                    .iter()
                    .map(|d| format::describe(d, exported))
                    .collect()
            };
            print!("{out}");
            let paths: Vec<&str> = descriptions.iter().map(|d| d.path.as_str()).collect();
            let _ = idx.log_stat("describe", out.len(), &paths);
        }
        Cmd::Tree { dir, depth, hubs } => {
            let prefix = dir.map(|d| idx.rel_path(&d)).transpose()?;
            let prefix = prefix.as_deref().filter(|s| !s.is_empty());
            let mut entries = idx.tree(prefix)?;
            let mut out = String::new();
            if hubs {
                entries.sort_by(|a, b| b.used_by_count.cmp(&a.used_by_count));
                entries.truncate(20);
                if cli.json {
                    out = format!("{}\n", serde_json::to_string_pretty(&entries)?);
                } else {
                    out = format::tree(&entries);
                }
            } else {
                // Big projects: a flat 5000-line listing defeats the purpose —
                // roll up by top-level directory unless told otherwise.
                let depth = match depth {
                    Some(d) => d,
                    None if entries.len() > 300 => {
                        if !cli.json {
                            out.push_str(&format!(
                                "{} files — rolled up by directory (use --depth 0 for the flat list, --hubs for top files)\n",
                                entries.len()
                            ));
                        }
                        1
                    }
                    None => 0,
                };
                if cli.json {
                    let json = if depth == 0 {
                        serde_json::to_string_pretty(&entries)?
                    } else {
                        serde_json::to_string_pretty(&format::rollup(&entries, prefix, depth))?
                    };
                    out.push_str(&json);
                    out.push('\n');
                } else if depth == 0 {
                    out.push_str(&format::tree(&entries));
                } else {
                    out.push_str(&format::tree_rollup(&entries, prefix, depth));
                }
            }
            print!("{out}");
            let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
            let _ = idx.log_stat("tree", out.len(), &paths);
        }
        Cmd::Search { terms } => {
            let hits = idx.search(&terms.join(" "))?;
            let out = if cli.json {
                format!("{}\n", serde_json::to_string_pretty(&hits)?)
            } else {
                format::find(&hits, &terms.join(" "))
            };
            print!("{out}");
            let paths: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
            let _ = idx.log_stat("search", out.len(), &paths);
        }
        Cmd::Find {
            name,
            kind,
            exported,
        } => {
            let kind = kind
                .map(|k| {
                    SymbolKind::from_str(&k).ok_or_else(|| anyhow!("unknown kind: {k}"))
                })
                .transpose()?;
            let hits = idx.find(&name, kind, exported)?;
            let out = if cli.json {
                format!("{}\n", serde_json::to_string_pretty(&hits)?)
            } else {
                format::find(&hits, &name)
            };
            print!("{out}");
            let paths: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
            let _ = idx.log_stat("find", out.len(), &paths);
        }
        Cmd::Refs { name, in_dir } => {
            let hits = idx.refs(&name, in_dir.as_deref())?;
            let out = if cli.json {
                format!("{}\n", serde_json::to_string_pretty(&hits)?)
            } else {
                format::refs(&hits, &name)
            };
            print!("{out}");
            let paths: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
            let _ = idx.log_stat("refs", out.len(), &paths);
        }
        Cmd::Related { file } => {
            let rel = idx.rel_path(&file)?;
            let info = idx.related(&rel)?;
            let out = if cli.json {
                format!("{}\n", serde_json::to_string_pretty(&info)?)
            } else {
                format::related(&info)
            };
            print!("{out}");
            let mut paths: Vec<&str> = vec![info.path.as_str()];
            paths.extend(info.internal_deps.iter().map(String::as_str));
            paths.extend(info.used_by.iter().map(String::as_str));
            let _ = idx.log_stat("related", out.len(), &paths);
        }
        Cmd::Gain => {
            let rows = idx.gain()?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                print!("{}", format::gain(&rows));
            }
        }
        Cmd::Annotate { target, doc } => {
            let (file, symbol_path) = target
                .rsplit_once(':')
                .ok_or_else(|| anyhow!("target must be <file>:<Symbol.path>"))?;
            let rel = idx.rel_path(file)?;
            idx.annotate(&rel, symbol_path, &doc)?;
            println!("annotated {rel}:{symbol_path}");
        }
    }
    Ok(())
}
