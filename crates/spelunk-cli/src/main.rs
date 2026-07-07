use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, bail};
use clap::{CommandFactory, Parser, Subcommand};
use spelunk_core::{chunk_files, walk_source_files};

/// spelunk: hybrid (lexical + semantic) code search.
///
/// `args_conflicts_with_subcommands` lets `spelunk "some query"` coexist with
/// `spelunk index`: a first token that matches a subcommand name is the
/// subcommand, anything else is a search query.
#[derive(Parser)]
#[command(
    name = "spelunk",
    version,
    about,
    args_conflicts_with_subcommands = true
)]
struct Cli {
    /// Search query, e.g. `spelunk "where is rate limiting implemented?"`
    query: Option<String>,

    /// Machine-readable JSON output.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Walk and chunk the repository (milestone 1: prints chunks, no
    /// persistent index yet).
    Index {
        /// Directory to index. Defaults to the current directory.
        path: Option<PathBuf>,
        /// Print every chunk as `path:start-end  kind  name`.
        #[arg(long)]
        print_chunks: bool,
    },
    /// Show the state of the index in `.spelunk/`.
    Status,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Index { path, print_chunks }) => {
            let root = path.unwrap_or_else(|| PathBuf::from("."));
            cmd_index(&root, print_chunks, cli.json)
        }
        Some(Command::Status) => {
            println!("no index found — the persistent index in .spelunk/ lands in milestone 2");
            Ok(())
        }
        None => match cli.query {
            Some(_) => {
                bail!("search lands in milestone 2; try `spelunk index --print-chunks` for now")
            }
            None => {
                Cli::command().print_help()?;
                Ok(())
            }
        },
    }
}

fn cmd_index(root: &std::path::Path, print_chunks: bool, json: bool) -> anyhow::Result<()> {
    let started = Instant::now();
    let files =
        walk_source_files(root).with_context(|| format!("failed to walk {}", root.display()))?;
    let outcome = chunk_files(&files);

    if json {
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        serde_json::to_writer_pretty(&mut out, &outcome.chunks)?;
        out.write_all(b"\n")?;
        return Ok(());
    }

    for skipped in &outcome.skipped {
        eprintln!("warning: skipped {}: {}", skipped.rel_path, skipped.reason);
    }

    if print_chunks {
        for chunk in &outcome.chunks {
            println!(
                "{}:{}-{}  {:<10}  {}",
                chunk.path,
                chunk.start_line,
                chunk.end_line,
                format!("{:?}", chunk.kind).to_lowercase(),
                chunk.name.as_deref().unwrap_or("-"),
            );
        }
        println!();
    }

    let mut per_language: BTreeMap<&str, usize> = BTreeMap::new();
    for file in &files {
        *per_language.entry(file.config.language.name()).or_default() += 1;
    }
    let languages = per_language
        .iter()
        .map(|(lang, n)| format!("{lang}: {n}"))
        .collect::<Vec<_>>()
        .join(", ");

    println!(
        "chunked {} files ({}) into {} chunks in {:.0?}{}",
        files.len(),
        if languages.is_empty() {
            "none"
        } else {
            languages.as_str()
        },
        outcome.chunks.len(),
        started.elapsed(),
        if outcome.skipped.is_empty() {
            String::new()
        } else {
            format!(", skipped {}", outcome.skipped.len())
        },
    );
    Ok(())
}
