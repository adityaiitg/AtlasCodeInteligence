use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "atlascode-inteligence-mcp",
    about = "A local-first Model Context Protocol (MCP) server for reusable engineering knowledge",
    version
)]
pub struct Cli {
    #[arg(short, long, env = "ATLAS_CODE_INTELIGENCE_DB", global = true)]
    pub db: Option<PathBuf>,

    #[arg(
        long,
        env = "ATLAS_CODE_INTELIGENCE_OFFLINE",
        default_missing_value = "true",
        num_args = 0..=1,
        value_parser = clap::builder::BoolishValueParser::new(),
        global = true
    )]
    pub offline: bool,

    #[arg(
        long,
        env = "ATLAS_CODE_INTELIGENCE_REQUIRE_SEMANTIC",
        default_missing_value = "true",
        num_args = 0..=1,
        value_parser = clap::builder::BoolishValueParser::new(),
        global = true
    )]
    pub require_semantic: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Start the standard MCP server over stdio (default)
    Serve(ServeArgs),

    /// Display statistics and metrics about the knowledge database
    Stats(StatsArgs),

    /// Export knowledge entries to a JSON or JSONL file
    Export(ExportArgs),

    /// Import knowledge entries from a JSON or JSONL backup
    Import(ImportArgs),

    /// Re-index SQLite FTS5 and embeddings
    Reindex(ReindexArgs),
}

#[derive(Args, Debug, Default)]
pub struct ServeArgs {}

#[derive(Args, Debug)]
pub struct StatsArgs {}

#[derive(Args, Debug)]
pub struct ExportArgs {
    /// Destination file path (e.g. backup.json or backup.jsonl)
    #[arg(short, long)]
    pub output: PathBuf,

    /// Optional project scope filter
    #[arg(short, long)]
    pub project: Option<String>,

    /// Export as line-delimited JSONL instead of single JSON array
    #[arg(long)]
    pub jsonl: bool,
}

#[derive(Args, Debug)]
pub struct ImportArgs {
    /// Source backup file path
    #[arg(short, long)]
    pub input: PathBuf,

    /// Merge incoming records into existing entries when dedup keys match
    #[arg(short, long)]
    pub merge: bool,
}

#[derive(Args, Debug)]
pub struct ReindexArgs {}
