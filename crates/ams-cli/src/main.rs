mod format;

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
    },
    /// Dependencies and reverse dependencies of a file
    Related {
        file: String,
    },
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
        Cmd::Build { .. } => unreachable!(),
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
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&descriptions)?);
            } else {
                for d in &descriptions {
                    print!("{}", format::describe(d, exported));
                }
            }
        }
        Cmd::Tree { dir } => {
            let prefix = dir.map(|d| idx.rel_path(&d)).transpose()?;
            let entries = idx.tree(prefix.as_deref().filter(|s| !s.is_empty()))?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else {
                print!("{}", format::tree(&entries));
            }
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
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&hits)?);
            } else {
                print!("{}", format::find(&hits, &name));
            }
        }
        Cmd::Refs { name } => {
            let hits = idx.refs(&name)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&hits)?);
            } else {
                print!("{}", format::refs(&hits, &name));
            }
        }
        Cmd::Related { file } => {
            let rel = idx.rel_path(&file)?;
            let info = idx.related(&rel)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&info)?);
            } else {
                print!("{}", format::related(&info));
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
