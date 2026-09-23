# AtlasCodeInteligence

A local-first, high-performance Rust MCP server for persisting and retrieving engineering workflows and verified problem resolutions across autonomous agent sessions.

Agents consult saved knowledge before repeating tasks or diagnosing complex bugs, and verify solutions before saving or merging entries.

---

## Architecture & Capabilities

- **Two-Phase Hybrid Retrieval**:
  - **Phase 1 (In-Memory Candidate Generation)**: Contiguous flat vector cache (`VectorIndex`) evaluated with **ARM NEON SIMD** (or portable unrolled SIMD) dot products combined with SQLite FTS5 column-weighted BM25 (`title: 10.0, tags: 6.0, context: 4.0, verification: 2.0`). RRF ($k=20$) fuses ranks with a $0.35$ minimum semantic floor.
  - **Phase 2 (Point Hydration)**: Only winning candidate entries are hydrated from SQLite via point queries, eliminating full table scans and achieving sub-2ms latency.
- **Automated Secret & Credential Redaction**:
  - Automatically identifies and sanitizes sensitive data before embedding and storage: AWS keys, GitHub/GitLab PATs, OpenAI/Anthropic/Google API keys, JWTs, Bearer tokens, PEM private keys, and database passwords in connection URIs.
- **Smart Deduplication & Accumulative Merge**:
  - Scrubs dynamic volatile tokens (UUIDs, hex addresses, ISO timestamps, ephemeral ports, temp paths) when computing deduplication keys.
  - Detects semantic near-duplicates ($\text{sim} \ge 0.92$) within the same `project_scope`.
  - Non-destructive smart merge: unions tag sets, appends novel verification evidence, and updates content in-place.
- **Audit History & Rollback**:
  - Schema migrations tracked via `PRAGMA user_version`.
  - `knowledge_history` audit table logs every modification with previous content, version number, and operation ("update" / "merge").
  - Point-in-time rollback capability to restore superseded solutions.
- **High-Concurrency SQLite Tuning**:
  - WAL journal mode, `busy_timeout(5000ms)`, `synchronous = NORMAL`, `mmap_size = 256MB`, and `TransactionBehavior::Immediate` to eliminate SQLITE_BUSY write deadlocks between concurrent agents.
- **Agent UX & Token Efficiency**:
  - Dense Markdown responses in `content[0].text` reduce LLM context token usage by over 30%.
  - `compact: true` option in `search_knowledge` yields concise summaries for up to 65% token savings.
  - Standard JSON structured payloads are preserved in `structuredContent` for programmatic tool callers.
- **MCP Resources & Prompts**:
  - Resources: `knowledge://recent`, `knowledge://stats`, `knowledge://entries/{id}`.
  - Prompts: `troubleshoot_issue`, `document_solution`.
- **Operational CLI Subcommands**:
  - `serve` (default stdio MCP server), `stats`, `export`, `import`, and `reindex`.

---

## Build and Test

```sh
# Run unit and integration tests in debug mode
cargo test

# Run all tests in release mode
cargo test --release

# Run CLI stats
cargo run --release -- stats

# Start the MCP server over stdio
cargo run --release -- serve
```

---

## Configuration & Environment Variables

| Variable | Description | Default |
|---|---|---|
| `ATLAS_CODE_INTELIGENCE_DB` | Explicit path to SQLite database | `~/Library/Application Support/AtlasCodeInteligence/knowledge.db` |
| `ATLAS_CODE_INTELIGENCE_OFFLINE` | Set to `1` or `true` for offline lexical-only mode | `false` |
| `ATLAS_CODE_INTELIGENCE_LEXICAL_ONLY` | Alias for offline mode (FTS5 BM25 search only) | `false` |
| `ATLAS_CODE_INTELIGENCE_REQUIRE_SEMANTIC` | Fail fast if model fails to download rather than falling back | `false` |
| `ATLAS_CODE_INTELIGENCE_MODEL_PATH` | Directory containing custom `model.safetensors` and `tokenizer.json` | Hugging Face cache |
| `RUST_LOG` | Tracing log level output to stderr (`error`, `warn`, `info`, `debug`) | `info` |

---

## CLI Usage

```sh
# Display database statistics and metrics
atlascode-inteligence-mcp stats

# Export entries to JSON or JSONL backup
atlascode-inteligence-mcp export --output backup.json
atlascode-inteligence-mcp export --output backup.jsonl --jsonl --project payments-backend

# Import entries from a backup
atlascode-inteligence-mcp import --input backup.json --merge

# Reindex full-text search and embeddings
atlascode-inteligence-mcp reindex
```

---

## MCP Client Configuration

Add to your editor or agent's MCP settings (`claude_desktop_config.json`, Cursor, Antigravity, etc.):

```json
{
  "mcpServers": {
    "atlascode-inteligence": {
      "command": "atlascode-inteligence-mcp",
      "args": []
    }
  }
}
```

---

## MCP Tools Reference

- **`search_knowledge`**:
  - `query` *(string, required)*: Natural language question, error code (`E0382`, `TS2322`), or keywords.
  - `limit` *(integer, 1..50, default 5)*: Maximum results.
  - `tags` *(array of strings)*: Tag filters.
  - `project_scope` *(string)*: Scope/repository isolation.
  - `compact` *(boolean, default false)*: Dense snippet mode for context window conservation.
- **`save_knowledge`**:
  - `title` *(string, required)*: Summary of the workflow or problem resolved.
  - `kind` *(enum, required)*: `"workflow"` or `"resolved_failure"`.
  - `context` *(string, required)*: Trigger conditions, symptoms, and environment.
  - `content` *(string, required)*: Step-by-step resolution or code modifications.
  - `verification` *(string, required)*: Concrete evidence the resolution worked (tests, commands).
  - `tags` *(array of strings)*: Descriptive categories.
  - `project_scope` *(string)*: Target repository or system.
  - `entry_id` *(string, optional)*: Explicit ID to update in place.
  - `dedup_key` *(string, optional)*: Custom deduplication slug.
- **`delete_knowledge`**:
  - `entry_id` *(string, required)*: The unique ID to delete.
