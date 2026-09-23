# AtlasCodeInteligence

A local-first Rust MCP server for remembering reusable engineering workflows and verified fixes. Agents search saved knowledge before repeating a task and explicitly save a workflow or resolution after confirming it works.

## Features

- Hybrid retrieval combines SQLite FTS5 keyword ranking with Model2Vec cosine similarity and reciprocal rank fusion.
- Semantic vectors use the same `minishlab/potion-code-16M-v2` model as CodeAtlas fast embeddings.
- SQLite stores entries, vectors, deduplication keys, and the full-text index in one local database.
- Existing entries can be replaced by ID or automatically updated in place via stable deduplication keys (`dedup_key`).
- Lexical-only mode supports fully offline operation without requiring model downloads or internet access.
- Search, save, and delete are exposed as MCP tools over stdio.

## Build and test

```sh
cargo build --release
cargo test
cargo run --release
```

On the first semantic search or save, the server downloads the model from Hugging Face and caches it locally. After that, model inference runs locally. The database defaults to the platform's user data directory under `AtlasCodeInteligence/knowledge.db`.

### Environment configuration

- `ATLAS_CODE_INTELIGENCE_DB`: Set an explicit SQLite database file path.
- `ATLAS_CODE_INTELIGENCE_OFFLINE`: Set to `1` or `true` to run strictly offline in lexical-only mode.
- `ATLAS_CODE_INTELIGENCE_LEXICAL_ONLY`: Set to `1` or `true` to disable semantic embedding downloads and use FTS5 keyword search only.
- `ATLAS_CODE_INTELIGENCE_REQUIRE_SEMANTIC`: Set to `1` or `true` to require semantic model loading instead of falling back to lexical-only mode on download failure.

## MCP configuration

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

## Tools

- `search_knowledge`: Search by natural language, error text, or exact keywords. Optional `tags` and `project_scope` filters are supported. In hybrid mode, reciprocal rank fusion (RRF) combines keyword and semantic rankings. In lexical-only mode, BM25 ranks are used.
- `save_knowledge`: Store a workflow or a resolved failure with its context, reusable steps or fix, and verification. Supply `entry_id` to replace an existing entry, or supply an optional `dedup_key` (defaults to `{project_scope}:{slugified_title}`) to automatically update existing entries in place.
- `delete_knowledge`: Remove an entry by ID.

The server does not inspect agent reasoning or tool history. The MCP client should call `save_knowledge` after confirming a reusable workflow or fix, and should search before saving a correction to an existing entry.
