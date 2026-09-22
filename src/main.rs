mod embedder;
mod mcp;
mod store;

use anyhow::{Context, Result};
use std::io::{self, BufRead};
use std::path::PathBuf;

fn main() -> Result<()> {
    let db_path = database_path()?;
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Creating data directory {}", parent.display()))?;
    }

    let store = store::KnowledgeStore::open(&db_path)?;
    let embedder = embedder::Model2Vec::new();
    eprintln!(
        "AtlasCodeInteligence MCP ready; database: {}",
        db_path.display()
    );

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                eprintln!("Failed to read MCP input: {error}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(request) => mcp::handle_request(&request, &store, &embedder),
            Err(error) => Some(mcp::error_response(
                &serde_json::Value::Null,
                -32700,
                &format!("Parse error: {error}"),
            )),
        };
        if let Some(response) = response {
            println!("{}", response);
        }
    }
    Ok(())
}

fn database_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("ATLAS_CODE_INTELIGENCE_DB") {
        return Ok(PathBuf::from(path));
    }
    let base = dirs::data_dir().context("Could not determine the user data directory")?;
    Ok(base.join("AtlasCodeInteligence").join("knowledge.db"))
}
