# AtlasCodeInteligence

A local-first Rust MCP server for remembering reusable engineering workflows and verified fixes. Agents search saved knowledge before repeating a task and explicitly save a workflow or resolution after confirming it works.

## Features

- Hybrid retrieval combines SQLite FTS5 keyword ranking with Model2Vec cosine similarity and reciprocal rank fusion.
- Semantic vectors use the same `minishlab/potion-code-16M-v2` model as CodeAtlas fast embeddings.
- SQLite stores entries, vectors, and the full-text index in one local database.
- Existing entries can be replaced by ID; updates refresh the searchable content and embedding.
- Search, save, and delete are exposed as MCP tools over stdio.

## Build and run

```sh
cargo build --release
cargo run --release
```

On the first semantic search or save, the server downloads the model from Hugging Face and caches it locally. After that, model inference runs locally. The database defaults to the platform's user data directory under `AtlasCodeInteligence/knowledge.db`.

Set `ATLAS_CODE_INTELIGENCE_DB` to use a specific SQLite database path.

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

- `search_knowledge`: Search by natural language, error text, or exact keywords. Optional `tags` and `project_scope` filters are supported.
- `save_knowledge`: Store a workflow or a resolved failure with its context, reusable steps or fix, and verification. Supply `entry_id` to replace an existing entry.
- `delete_knowledge`: Remove an entry by ID.

The server does not inspect agent reasoning or tool history. The MCP client should call `save_knowledge` after confirming a reusable workflow or fix, and should search before saving a correction to an existing entry.
