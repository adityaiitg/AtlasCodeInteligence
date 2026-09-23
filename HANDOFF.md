# AtlasCodeInteligence: Detailed Handoff

**Date:** 2026-09-23  
**Repository:** `/Volumes/T7/Personal_MAC_DATA/Personal/github/AtlasCodeInteligence`  
**Status:** Local implementation complete; GitHub publishing intentionally deferred.

## 1. Product intent

AtlasCodeInteligence is a local-first Rust MCP server for retaining engineering knowledge that is useful across coding-agent sessions and projects.

The intended learning loop is:

1. An agent searches before repeating a workflow or debugging a failure.
2. The agent performs the work and confirms what actually worked.
3. The agent explicitly saves the validated workflow or resolution through MCP.
4. Future agents retrieve the entry using exact error text, keywords, or a semantic paraphrase.
5. If the knowledge is corrected, the agent saves it with the existing entry ID so the old searchable content is replaced.

The server does not observe agent reasoning, shell history, or tool calls automatically. Capture is explicit through `save_knowledge` so only confirmed knowledge enters the store.

## 2. Decisions and scope

- Repository name and spelling: `AtlasCodeInteligence`.
- Implementation language: Rust, edition 2021.
- Integration: MCP JSON-RPC over stdio.
- Storage: one SQLite database shared locally across projects.
- Default database: platform user-data directory plus `AtlasCodeInteligence/knowledge.db`.
- Override: set `ATLAS_CODE_INTELIGENCE_DB` to an explicit SQLite path.
- Retrieval: SQLite FTS5 keyword search plus dense vector similarity, combined using reciprocal rank fusion (RRF).
- Embedding model: `minishlab/potion-code-16M-v2`, matching CodeAtlas fast mode.
- Update policy: replacement in place, with stable ID and original creation timestamp.
- Entry history: no revision table is implemented in v1; replacement removes the previous searchable text and vector.
- Authentication and multi-user access: out of scope for the local v1 server.
- GitHub remote: not created or pushed because publishing was deferred by the user.

## 3. Repository contents

### `Cargo.toml`

Defines the `atlascode-inteligence-mcp` binary and dependencies for SQLite, FTS5, serialization, UUIDs, timestamps, Hugging Face model access, SafeTensors, and tokenization.

### `src/main.rs`

- Resolves the database path.
- Creates the parent data directory when needed.
- Opens the SQLite store and initializes the lazy Model2Vec embedder.
- Reads one JSON-RPC request per stdin line.
- Writes protocol responses only to stdout.
- Sends startup diagnostics to stderr so MCP stdout remains valid JSON-RPC.

### `src/embedder.rs`

- Downloads `model.safetensors`, `tokenizer.json`, and optional `config.json` from Hugging Face on first use.
- Uses a mutex-protected lazy model so startup does not require an immediate model download.
- Supports the Model2Vec token averaging and normalization behavior used by CodeAtlas fast mode.
- Produces local `Vec<f32>` embeddings for both saved entries and search queries.

### `src/store.rs`

Owns the SQLite schema and persistence behavior:

- `knowledge` table stores entry fields, serialized tags, optional project scope, vector BLOB, creation time, and update time.
- `knowledge_fts` is an FTS5 virtual table using Porter stemming and Unicode tokenization.
- The entry ID is `UNINDEXED` in FTS because it is metadata, not content.
- Project scope is also `UNINDEXED`; it filters candidates but does not become searchable text.
- Replacement deletes the old FTS row, upserts the knowledge row, inserts the new FTS row, and refreshes the embedding in one transaction.
- SQLite uses WAL mode for disk databases and enables foreign keys.

### `src/mcp.rs`

Implements MCP protocol handling and tool contracts. Search uses the filtered candidate set, keyword ranks, semantic ranks, and RRF scoring.

### `README.md`

Contains build instructions, MCP configuration, database configuration, and tool descriptions.

## 4. MCP interface

The server reports protocol version `2024-11-05` and server name `atlascode-inteligence`.

### `search_knowledge`

Required argument:

```json
{"query":"text, question, error message, or exact keyword"}
```

Optional arguments:

- `limit`: integer from 1 to 50; defaults to 5.
- `tags`: array of strings; all supplied tags must match.
- `project_scope`: exact project/repository filter.

The response includes each entry plus `score`, `keyword_rank`, and `semantic_rank`. The ranking label is `reciprocal_rank_fusion`.

### `save_knowledge`

Required arguments:

- `title`
- `kind`: `workflow` or `resolved_failure`
- `context`: when to use the workflow, or the observed failure context
- `content`: reusable steps, root cause, and resolution
- `verification`: evidence that the result worked

Optional arguments:

- `entry_id`: existing ID to replace; omit to create a UUID entry.
- `tags`: array of strings.
- `project_scope`: repository or project name.

The embedding input combines title, context, content, verification, and tags. Replacing a missing ID returns an error rather than silently creating a new entry.

### `delete_knowledge`

Required argument:

```json
{"entry_id":"uuid"}
```

The operation removes both the primary row and FTS row and returns whether a row was deleted.

## 5. Search behavior

For a search request:

1. Apply project and tag filters to the stored entries.
2. Run FTS5 keyword search over title, kind, context, content, verification, and tags.
3. Restrict lexical ranks to the eligible filtered IDs.
4. Embed the query using Model2Vec.
5. Compute cosine similarity against stored vectors.
6. Rank semantic candidates by descending similarity.
7. Assign each result an RRF contribution of `1 / (60 + rank)` for each signal in which it appears.
8. Sort by combined score, then newest update time, and truncate to `limit`.

This gives exact error/configuration terms a lexical path while allowing paraphrased questions to match semantically.

## 6. Verification completed

Commands run successfully:

```sh
cargo fmt
cargo test
cargo build
```

The Rust test suite currently has five passing tests:

- MCP tool definitions expose search, save, and delete.
- MCP initialized notifications produce no response.
- Vector byte serialization and cosine similarity behave as expected.
- Replacing an entry preserves its ID and refreshes/removes old FTS content.
- Entries and FTS rows survive closing and reopening a SQLite database.

A live binary smoke test also passed MCP `initialize` and `tools/list`, confirming JSON-RPC stdout, stderr-only diagnostics, and the advertised tool schemas.

A broader live save/search/replace/restart/delete script initially found that project scope text was unintentionally searchable. The implementation was corrected by making the FTS project scope column `UNINDEXED`, and a regression test now covers the same boundary.

The full live script has been rerun against a temporary database and verified:
- `initialize` handshake succeeded.
- `save_knowledge` stored the entry with generated UUID and valid timestamps.
- Exact keyword search retrieved the entry with `keyword_rank: 1`.
- Project scope search confirmed `keyword_rank: None` (not indexed in FTS).
- Semantic paraphrase search retrieved the entry with semantic rank.
- In-place replacement preserved entry ID and creation timestamp while updating content.
- Process restart confirmed data persistence across server restarts.
- Keyword search on replaced content succeeded; keyword search on obsolete content returned zero matches.
- `delete_knowledge` removed the entry, and subsequent search confirmed complete removal.

## 7. Current Git state

Initial commit:

```text
704f29c Initial AtlasCodeInteligence MCP server
```

The uncommitted follow-up changes:
- `src/mcp.rs`: restrict lexical ranks to eligible filtered candidate IDs.
- `src/store.rs`: mark `project_scope` as `UNINDEXED` in `knowledge_fts`.
- `HANDOFF.md`: updated documentation of live testing and verification.

## 8. Known limitations

- First semantic save/search requires network access to download the Hugging Face model.
- There is no explicit lexical-only fallback if model download fails.
- The model is loaded lazily and held in process memory after first use.
- Search currently compares the query vector with every eligible stored vector; this is appropriate for a small local store but will need an ANN/vector index for larger collections.
- Replacement intentionally discards prior revisions.
- No automatic deduplication or canonical workflow key exists.
- No authentication, encryption, remote sync, tenancy, or conflict resolution exists.
- The database path is local to the host running the MCP process.

## 9. Recommended next steps

1. Rerun a live temporary-database flow: save, exact keyword search, paraphrase search, replace, restart, search again, delete, and confirm removal.
2. Commit `src/mcp.rs`, `src/store.rs`, and `HANDOFF.md`.
3. Decide repository visibility, license, and whether to publish a GitHub remote.
4. Add a lexical-only mode for offline operation and clearer model-download errors.
5. Add a stable deduplication key such as project plus normalized title/error signature.
6. Add optional revision history if auditability becomes more important than replacement simplicity.
7. Add an integration test that launches the binary and exercises JSON-RPC over pipes.
8. Consider an approximate nearest-neighbor index or SQLite vector extension when the store grows beyond a few thousand entries.
