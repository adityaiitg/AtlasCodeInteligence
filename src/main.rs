use anyhow::{Context, Result};
use clap::Parser;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use atlascode_inteligence::cli::{Cli, Commands, ExportArgs, ImportArgs};
use atlascode_inteligence::embedder::{Embedder, Model2Vec};
use atlascode_inteligence::mcp;
use atlascode_inteligence::security::{read_bounded_line, validate_database_path, InputLimits};
use atlascode_inteligence::store::KnowledgeStore;
use atlascode_inteligence::types::{EntryKind, KnowledgeEntry};

fn main() -> Result<()> {
    // Initialize tracing on stderr
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let db_path = resolve_database_path(&cli)?;

    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Creating data directory {}", parent.display()))?;
    }

    let store = KnowledgeStore::open(&db_path)?;

    match cli.command {
        Some(Commands::Stats(_)) => run_stats(&store, &db_path),
        Some(Commands::Export(args)) => run_export(&store, &args),
        Some(Commands::Import(args)) => run_import(&store, &args),
        Some(Commands::Reindex(_)) => run_reindex(&store),
        Some(Commands::Serve(_)) | None => run_server(store, db_path),
    }
}

fn run_server(store: KnowledgeStore, db_path: PathBuf) -> Result<()> {
    let embedder = Model2Vec::new();
    eprintln!(
        "AtlasCodeInteligence MCP ready; database: {}",
        db_path.display()
    );

    let limits = InputLimits::default();
    let stdin = io::stdin();
    let mut stdin_lock = stdin.lock();
    let mut line_buf = String::with_capacity(1024);

    loop {
        match read_bounded_line(&mut stdin_lock, &mut line_buf, limits.max_line_len) {
            Ok(Some(_)) => {
                let trimmed = line_buf.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let response = match serde_json::from_str::<serde_json::Value>(trimmed) {
                    Ok(request) => mcp::handle_request(&request, &store, &embedder),
                    Err(error) => Some(mcp::error_response(
                        &serde_json::Value::Null,
                        -32700,
                        &format!("Parse error: {error}"),
                    )),
                };

                if let Some(response) = response {
                    println!("{}", response);
                    let _ = io::stdout().flush();
                }
            }
            Ok(None) => break, // EOF reached
            Err(error) => {
                eprintln!("Failed to read MCP input line: {error}");
                break;
            }
        }
    }

    Ok(())
}

fn run_stats(store: &KnowledgeStore, db_path: &Path) -> Result<()> {
    let stats = store.stats()?;
    println!("AtlasCodeInteligence Database Statistics");
    println!("========================================");
    println!("Database path:      {}", db_path.display());
    println!("Total entries:      {}", stats.total_entries);
    println!("Audit revisions:    {}", stats.total_history_revisions);
    println!(
        "Database size:      {:.2} KB",
        stats.db_size_bytes as f64 / 1024.0
    );
    println!("Semantic vector active: {}", stats.semantic_active);
    println!("\nProject Breakdown:");
    for (project, count) in &stats.projects {
        println!("  - {project:<20} : {count} entries");
    }
    Ok(())
}

fn run_export(store: &KnowledgeStore, args: &ExportArgs) -> Result<()> {
    let entries = store.all(args.project.as_deref(), &[])?;
    let file = File::create(&args.output)
        .with_context(|| format!("Creating export file {}", args.output.display()))?;
    let mut writer = BufWriter::new(file);

    if args.jsonl {
        for (entry, _) in &entries {
            serde_json::to_writer(&mut writer, entry)?;
            writer.write_all(b"\n")?;
        }
    } else {
        let just_entries: Vec<&KnowledgeEntry> = entries.iter().map(|(e, _)| e).collect();
        serde_json::to_writer_pretty(&mut writer, &just_entries)?;
    }

    writer.flush()?;
    println!(
        "Exported {} knowledge entries to {}",
        entries.len(),
        args.output.display()
    );
    Ok(())
}

fn run_import(store: &KnowledgeStore, args: &ImportArgs) -> Result<()> {
    let file = File::open(&args.input)
        .with_context(|| format!("Opening import file {}", args.input.display()))?;
    let reader = BufReader::new(file);

    let entries: Vec<KnowledgeEntry> =
        if args.input.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            use std::io::BufRead;
            let mut list = Vec::new();
            for line in reader.lines() {
                let l = line?;
                if !l.trim().is_empty() {
                    let entry: KnowledgeEntry = serde_json::from_str(&l)?;
                    list.push(entry);
                }
            }
            list
        } else {
            serde_json::from_reader(reader)?
        };

    let embedder = Model2Vec::new();
    let mut imported = 0;
    for entry in &entries {
        let kind = EntryKind::parse(&entry.kind).unwrap_or(EntryKind::Workflow);
        let embedded_text = format!(
            "{}\n{}\n{}\n{}\n{}",
            entry.title,
            entry.context,
            entry.content,
            entry.verification,
            entry.tags.join(" ")
        );
        let vector = embedder.embed(&embedded_text)?.unwrap_or_default();
        store.save(
            if args.merge { None } else { Some(&entry.id) },
            &entry.title,
            &kind,
            &entry.context,
            &entry.content,
            &entry.verification,
            &entry.tags,
            entry.project_scope.as_deref(),
            &vector,
            entry.dedup_key.as_deref(),
        )?;
        imported += 1;
    }

    println!(
        "Imported {} knowledge entries from {}",
        imported,
        args.input.display()
    );
    Ok(())
}

fn run_reindex(store: &KnowledgeStore) -> Result<()> {
    let embedder = Model2Vec::new();
    let entries = store.all(None, &[])?;
    println!("Re-indexing {} knowledge entries...", entries.len());

    for (entry, _) in &entries {
        let kind = EntryKind::parse(&entry.kind).unwrap_or(EntryKind::Workflow);
        let embedded_text = format!(
            "{}\n{}\n{}\n{}\n{}",
            entry.title,
            entry.context,
            entry.content,
            entry.verification,
            entry.tags.join(" ")
        );
        let vector = embedder.embed(&embedded_text)?.unwrap_or_default();
        store.save(
            Some(&entry.id),
            &entry.title,
            &kind,
            &entry.context,
            &entry.content,
            &entry.verification,
            &entry.tags,
            entry.project_scope.as_deref(),
            &vector,
            entry.dedup_key.as_deref(),
        )?;
    }

    println!("Re-indexing completed successfully.");
    Ok(())
}

fn resolve_database_path(cli: &Cli) -> Result<PathBuf> {
    let path = if let Some(ref path) = cli.db {
        path.clone()
    } else if let Some(path) = std::env::var_os("ATLAS_CODE_INTELIGENCE_DB") {
        PathBuf::from(path)
    } else {
        let base = dirs::data_dir().context("Could not determine user data directory")?;
        base.join("AtlasCodeInteligence").join("knowledge.db")
    };

    validate_database_path(&path).map_err(|e| anyhow::anyhow!(e))
}
