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
cargo fmt --check
cargo test
cargo build --release
```

The test suite consists of **6 unit tests** and **3 integration tests** (all passing):

- Unit tests (`src/main.rs`):
  - `exposes_search_save_and_delete_tools`: verifies MCP tool schemas and definitions.
  - `ignores_initialized_notification`: verifies notifications produce no response.
  - `vector_storage_round_trips_and_cosine_scores_match`: confirms byte serialization and cosine math.
  - `deduplication_updates_existing_entry_in_place`: confirms in-place updates when matching `dedup_key`.
  - `saving_and_replacing_updates_full_text_index`: verifies FTS5 updates on replace and isolation of unindexed scope tokens.
  - `entries_and_fts_index_survive_reopening_database`: tests persistence across database connections.
- End-to-end integration tests over stdio pipes (`tests/mcp_integration.rs`):
  - `test_mcp_binary_full_lifecycle`: exercises binary spawn, `initialize`, `tools/list`, save, exact keyword search, unindexed scope search isolation, semantic paraphrase search, in-place replace, server kill/restart persistence, delete, and post-delete verification.
  - `test_mcp_automatic_deduplication`: tests automatic in-place deduplication when saving with identical title and project scope, preserving entry ID and creation timestamp.
  - `test_mcp_offline_lexical_only_mode`: tests `ATLAS_CODE_INTELIGENCE_OFFLINE=1` running strictly without network/embeddings using SQLite FTS5 BM25 retrieval.

## 7. Current Git state

Recent commits:

```text
8e19de6 Fix FTS project scope indexing, candidate rank filtering, and document live verification
704f29c Initial AtlasCodeInteligence MCP server
```

Recent improvements:
- `src/embedder.rs`: Added offline and lexical-only mode support via `ATLAS_CODE_INTELIGENCE_OFFLINE` / `ATLAS_CODE_INTELIGENCE_LEXICAL_ONLY`, graceful fallback with clear diagnostics, and optional enforcement via `ATLAS_CODE_INTELIGENCE_REQUIRE_SEMANTIC`.
- `src/store.rs`: Added `dedup_key` schema migration with partial unique index, automatic deduplication in `save()`, `generate_dedup_key()` canonical slugifier, and unit tests.
- `src/mcp.rs`: Exposed `dedup_key` in MCP tool schema, supported automatic deduplication and reporting, added positive similarity thresholding (`sim > 0.0`), and added `"lexical_only"` ranking label when offline.
- `tests/mcp_integration.rs`: Added comprehensive stdio JSON-RPC integration test suite covering full lifecycle, deduplication, and offline mode.
- `README.md`: Documented offline/lexical configurations, deduplication keys, and build/test workflows.

## 8. Known limitations

- The embedding model is loaded lazily on first semantic search/save and held in process memory.
- Search compares the query vector with every eligible stored vector in memory; this is fast (<1ms) for small-to-medium stores but will benefit from an ANN/vector index when exceeding thousands of entries.
- Replacement in place intentionally discards prior revisions (no audit log table yet).
- No authentication, encryption, remote sync, multi-user tenancy, or conflict resolution exists (local single-user MCP design).
- The database path is local to the host running the MCP process.

## 9. Recommended next steps

1. Commit the offline mode, deduplication, integration tests, and updated documentation.
2. Decide repository visibility, license (e.g. MIT, Apache-2.0), and whether to publish to a GitHub remote.
3. Add optional revision history if auditability becomes more important than replacement simplicity.
4. Consider an approximate nearest-neighbor index or SQLite vector extension when the store grows beyond a few thousand entries.
