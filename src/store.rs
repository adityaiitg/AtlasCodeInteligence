use crate::security::{escape_fts5_query, FtsOptions};
use crate::simd::bytes_to_vector;
use crate::types::{
    EntryKind, HitTelemetry, KnowledgeEntry, KnowledgeHistoryEntry, SearchHit, SearchResult,
    SearchTelemetry, StatsResult,
};
use crate::vector_index::VectorIndex;
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use uuid::Uuid;

pub struct KnowledgeStore {
    connection: Mutex<Connection>,
    db_path: PathBuf,
    vector_index: Arc<Mutex<VectorIndex>>,
}

impl KnowledgeStore {
    pub fn open(path: &Path) -> Result<Self> {
        let mut connection = Connection::open(path)
            .with_context(|| format!("Opening knowledge database at {}", path.display()))?;

        // 1. High performance SQLite PRAGMAs
        if path != Path::new(":memory:") {
            let _ = connection.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()));
            let _ = connection.query_row("PRAGMA synchronous=NORMAL", [], |_| Ok(()));
            let _ = connection.query_row("PRAGMA mmap_size=268435456", [], |_| Ok(()));
        }
        let _ = connection.busy_timeout(std::time::Duration::from_millis(5000));
        let _ = connection.pragma_update(None, "foreign_keys", "ON");
        let _ = connection.pragma_update(None, "cache_size", -64000);
        let _ = connection.pragma_update(None, "temp_store", "MEMORY");

        // 2. Schema migrations via user_version
        Self::apply_migrations(&mut connection)?;

        // 3. Load vector index in memory for fast candidate scoring
        let mut vector_index = VectorIndex::new();
        Self::load_vector_index(&connection, &mut vector_index)?;

        Ok(Self {
            connection: Mutex::new(connection),
            db_path: path.to_path_buf(),
            vector_index: Arc::new(Mutex::new(vector_index)),
        })
    }

    fn apply_migrations(conn: &mut Connection) -> Result<()> {
        let user_version: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;

        if user_version < 1 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS knowledge (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    context TEXT NOT NULL,
                    content TEXT NOT NULL,
                    verification TEXT NOT NULL,
                    tags TEXT NOT NULL,
                    project_scope TEXT,
                    embedding BLOB NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    dedup_key TEXT
                );
                CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_fts USING fts5(
                    id UNINDEXED, title, kind, context, content, verification, tags, project_scope UNINDEXED,
                    tokenize='porter unicode61'
                );
                CREATE UNIQUE INDEX IF NOT EXISTS idx_knowledge_dedup_key ON knowledge(dedup_key) WHERE dedup_key IS NOT NULL;
                CREATE INDEX IF NOT EXISTS idx_knowledge_project ON knowledge(project_scope);
                CREATE INDEX IF NOT EXISTS idx_knowledge_updated ON knowledge(updated_at DESC);
                PRAGMA user_version = 1;",
            )?;
        }

        if user_version < 2 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS knowledge_history (
                    history_id INTEGER PRIMARY KEY AUTOINCREMENT,
                    entry_id TEXT NOT NULL,
                    version INTEGER NOT NULL,
                    operation TEXT NOT NULL,
                    title TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    context TEXT NOT NULL,
                    content TEXT NOT NULL,
                    verification TEXT NOT NULL,
                    tags TEXT NOT NULL,
                    project_scope TEXT,
                    timestamp TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_history_entry_id ON knowledge_history(entry_id, version DESC);
                CREATE INDEX IF NOT EXISTS idx_knowledge_project_updated ON knowledge(project_scope, updated_at DESC);
                PRAGMA user_version = 2;",
            )?;
        }

        Ok(())
    }

    fn load_vector_index(conn: &Connection, index: &mut VectorIndex) -> Result<()> {
        let mut stmt = conn.prepare(
            "SELECT id, project_scope, tags, embedding FROM knowledge WHERE length(embedding) > 0",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let scope: Option<String> = row.get(1)?;
            let tags_raw: String = row.get(2)?;
            let tags: Vec<String> = serde_json::from_str(&tags_raw).unwrap_or_default();
            let blob: Vec<u8> = row.get(3)?;
            let vector = bytes_to_vector(&blob);
            index.insert_or_update(&id, scope.as_deref(), &tags, &vector);
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn save(
        &self,
        entry_id: Option<&str>,
        title: &str,
        kind: &EntryKind,
        context: &str,
        content: &str,
        verification: &str,
        tags: &[String],
        project_scope: Option<&str>,
        embedding: &[f32],
        dedup_key: Option<&str>,
    ) -> Result<(KnowledgeEntry, bool)> {
        let (id, deduplicated) = if let Some(explicit_id) = entry_id {
            (explicit_id.to_owned(), false)
        } else if let Some(key) = dedup_key {
            if let Some(existing) = self.get_by_dedup_key(key)? {
                (existing.id, true)
            } else {
                (Uuid::new_v4().to_string(), false)
            }
        } else {
            (Uuid::new_v4().to_string(), false)
        };

        let timestamp = chrono::Utc::now().to_rfc3339();
        let mut conn = self
            .connection
            .lock()
            .map_err(|e| anyhow::anyhow!("DB lock error: {e}"))?;

        // Read existing entry if present to record history
        let existing: Option<KnowledgeEntry> = conn
            .query_row(
                "SELECT id, title, kind, context, content, verification, tags, project_scope, created_at, updated_at, dedup_key
                 FROM knowledge WHERE id = ?1",
                [&id],
                row_to_entry,
            )
            .optional()?;

        if entry_id.is_some() && existing.is_none() {
            anyhow::bail!("Knowledge entry '{id}' does not exist");
        }

        let created_at = existing
            .as_ref()
            .map(|e| e.created_at.clone())
            .unwrap_or_else(|| timestamp.clone());

        let tags_json = serde_json::to_string(tags)?;
        let embedding_bytes = vector_to_bytes(embedding);

        // Immediate transaction prevents write contention deadlocks
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        // If updating existing entry, record in knowledge_history
        if let Some(prev) = &existing {
            let next_version: u32 = tx
                .query_row(
                    "SELECT COALESCE(MAX(version), 0) + 1 FROM knowledge_history WHERE entry_id = ?1",
                    [&id],
                    |row| row.get(0),
                )
                .unwrap_or(1);

            let op = if deduplicated { "merge" } else { "update" };
            let prev_tags_json = serde_json::to_string(&prev.tags)?;
            tx.execute(
                "INSERT INTO knowledge_history (entry_id, version, operation, title, kind, context, content, verification, tags, project_scope, timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    prev.id,
                    next_version,
                    op,
                    prev.title,
                    prev.kind,
                    prev.context,
                    prev.content,
                    prev.verification,
                    prev_tags_json,
                    prev.project_scope,
                    timestamp,
                ],
            )?;
        }

        tx.execute("DELETE FROM knowledge_fts WHERE id = ?1", [&id])?;
        tx.execute(
            "INSERT INTO knowledge (id, title, kind, context, content, verification, tags, project_scope, embedding, created_at, updated_at, dedup_key)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(id) DO UPDATE SET title=excluded.title, kind=excluded.kind, context=excluded.context,
                content=excluded.content, verification=excluded.verification, tags=excluded.tags,
                project_scope=excluded.project_scope, embedding=excluded.embedding, updated_at=excluded.updated_at,
                dedup_key=excluded.dedup_key",
            params![id, title, kind.as_str(), context, content, verification, tags_json, project_scope, embedding_bytes, created_at, timestamp, dedup_key],
        )?;

        // Populate FTS5 index
        tx.execute(
            "INSERT INTO knowledge_fts (id, title, kind, context, content, verification, tags, project_scope)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, title, kind.as_str(), context, content, verification, tags.join(" "), project_scope.unwrap_or("")],
        )?;

        tx.commit()?;

        // Update in-memory vector index
        if let Ok(mut vi) = self.vector_index.lock() {
            vi.insert_or_update(&id, project_scope, tags, embedding);
        }

        let entry = KnowledgeEntry {
            id,
            title: title.to_string(),
            kind: kind.as_str().to_string(),
            context: context.to_string(),
            content: content.to_string(),
            verification: verification.to_string(),
            tags: tags.to_vec(),
            project_scope: project_scope.map(str::to_owned),
            created_at,
            updated_at: timestamp,
            dedup_key: dedup_key.map(str::to_owned),
        };

        Ok((entry, deduplicated))
    }

    pub fn delete(&self, id: &str) -> Result<bool> {
        let mut conn = self
            .connection
            .lock()
            .map_err(|e| anyhow::anyhow!("DB lock error: {e}"))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM knowledge_fts WHERE id = ?1", [id])?;
        let changed = tx.execute("DELETE FROM knowledge WHERE id = ?1", [id])?;
        tx.commit()?;

        if let Ok(mut vi) = self.vector_index.lock() {
            vi.remove(id);
        }

        Ok(changed > 0)
    }

    pub fn get(&self, id: &str) -> Result<Option<KnowledgeEntry>> {
        let conn = self
            .connection
            .lock()
            .map_err(|e| anyhow::anyhow!("DB lock error: {e}"))?;
        conn.query_row(
            "SELECT id, title, kind, context, content, verification, tags, project_scope, created_at, updated_at, dedup_key
             FROM knowledge WHERE id = ?1",
            [id],
            row_to_entry,
        )
        .optional()
        .context("Reading knowledge entry")
    }

    pub fn get_by_dedup_key(&self, dedup_key: &str) -> Result<Option<KnowledgeEntry>> {
        let conn = self
            .connection
            .lock()
            .map_err(|e| anyhow::anyhow!("DB lock error: {e}"))?;
        conn.query_row(
            "SELECT id, title, kind, context, content, verification, tags, project_scope, created_at, updated_at, dedup_key
             FROM knowledge WHERE dedup_key = ?1",
            [dedup_key],
            row_to_entry,
        )
        .optional()
        .context("Reading knowledge entry by dedup_key")
    }

    pub fn get_history(&self, entry_id: &str) -> Result<Vec<KnowledgeHistoryEntry>> {
        let conn = self
            .connection
            .lock()
            .map_err(|e| anyhow::anyhow!("DB lock error: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT history_id, entry_id, version, operation, title, kind, context, content, verification, tags, project_scope, timestamp
             FROM knowledge_history WHERE entry_id = ?1 ORDER BY version DESC",
        )?;
        let rows = stmt.query_map([entry_id], |row| {
            let tags_raw: String = row.get(9)?;
            let tags: Vec<String> = serde_json::from_str(&tags_raw).unwrap_or_default();
            Ok(KnowledgeHistoryEntry {
                history_id: row.get(0)?,
                entry_id: row.get(1)?,
                version: row.get(2)?,
                operation: row.get(3)?,
                title: row.get(4)?,
                kind: row.get(5)?,
                context: row.get(6)?,
                content: row.get(7)?,
                verification: row.get(8)?,
                tags,
                project_scope: row.get(10)?,
                timestamp: row.get(11)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("Reading knowledge history")
    }

    pub fn all(
        &self,
        project: Option<&str>,
        tags: &[String],
    ) -> Result<Vec<(KnowledgeEntry, Vec<f32>)>> {
        let conn = self
            .connection
            .lock()
            .map_err(|e| anyhow::anyhow!("DB lock error: {e}"))?;

        let mut statement = conn.prepare(
            "SELECT id, title, kind, context, content, verification, tags, project_scope, embedding, created_at, updated_at, dedup_key
             FROM knowledge ORDER BY updated_at DESC",
        )?;
        let entries = statement
            .query_map([], |row| {
                let entry = KnowledgeEntry {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    kind: row.get(2)?,
                    context: row.get(3)?,
                    content: row.get(4)?,
                    verification: row.get(5)?,
                    tags: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                    project_scope: row.get(7)?,
                    created_at: row.get(9)?,
                    updated_at: row.get(10)?,
                    dedup_key: row.get(11)?,
                };
                let vector = bytes_to_vector(&row.get::<_, Vec<u8>>(8)?);
                Ok((entry, vector))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(entries
            .into_iter()
            .filter(|(entry, _)| {
                project.is_none_or(|scope| entry.project_scope.as_deref() == Some(scope))
                    && tags.iter().all(|tag| {
                        entry
                            .tags
                            .iter()
                            .any(|stored| stored.eq_ignore_ascii_case(tag))
                    })
            })
            .collect())
    }

    /// Column-weighted BM25 keyword search using SQLite FTS5.
    /// Weights: title: 10.0, tags: 6.0, context: 4.0, verification: 2.0, content: 1.0, kind: 1.0.
    pub fn keyword_search(&self, query: &str, limit: usize) -> Result<Vec<(String, f64)>> {
        let fts_opts = FtsOptions::default();
        let Some(fts_query) = escape_fts5_query(query, &fts_opts) else {
            return Ok(Vec::new());
        };

        let conn = self
            .connection
            .lock()
            .map_err(|e| anyhow::anyhow!("DB lock error: {e}"))?;

        // 8 column weights for: (id, title, kind, context, content, verification, tags, project_scope)
        let mut statement = conn.prepare(
            "SELECT id, bm25(knowledge_fts, 0.0, 10.0, 1.0, 4.0, 1.0, 2.0, 6.0, 0.0) AS rank
             FROM knowledge_fts
             WHERE knowledge_fts MATCH ?1 ORDER BY rank LIMIT ?2",
        )?;

        let rows = statement.query_map(params![fts_query, limit as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("Running keyword search")
    }

    /// High performance two-phase hybrid search combining vector dot products with weighted BM25 and RRF.
    pub fn hybrid_search(
        &self,
        query: &str,
        maybe_query_vector: Option<&[f32]>,
        project: Option<&str>,
        tags: &[String],
        limit: usize,
    ) -> Result<SearchResult> {
        let total_start = Instant::now();

        // 1. Lexical retrieval
        let lex_start = Instant::now();
        let candidate_limit = (limit * 10).max(50);
        let lexical_candidates = self.keyword_search(query, candidate_limit)?;
        let lex_time = lex_start.elapsed().as_secs_f64() * 1000.0;

        let lexical_ranks: HashMap<String, usize> = lexical_candidates
            .into_iter()
            .enumerate()
            .map(|(rank, (id, _))| (id, rank + 1))
            .collect();

        // 2. Semantic retrieval from in-memory contiguous vector index
        let sem_start = Instant::now();
        let semantic_results = if let Some(query_vec) = maybe_query_vector {
            if let Ok(vi) = self.vector_index.lock() {
                // Minimum score threshold of 0.35 to reject low-similarity noise
                vi.search(query_vec, project, tags, 0.35, candidate_limit)
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        let sem_time = sem_start.elapsed().as_secs_f64() * 1000.0;

        let semantic_ranks: HashMap<String, (usize, f32)> = semantic_results
            .into_iter()
            .enumerate()
            .map(|(rank, (id, score))| (id, (rank + 1, score)))
            .collect();

        // 3. Fusion scoring: Reciprocal Rank Fusion with k=20
        let rrf_k = 20.0;
        struct CandidateScore {
            score: f64,
            keyword_rank: Option<usize>,
            semantic_rank: Option<usize>,
            vector_sim: Option<f32>,
        }

        let mut scored_candidate_ids: HashMap<String, CandidateScore> = HashMap::new();

        for (id, lex_rank) in &lexical_ranks {
            let sem_info = semantic_ranks.get(id);
            let sem_rank = sem_info.map(|(r, _)| *r);
            let sem_sim = sem_info.map(|(_, s)| *s);

            let rrf_score = (1.0 / (rrf_k + *lex_rank as f64))
                + sem_rank.map_or(0.0, |r| 1.0 / (rrf_k + r as f64));

            scored_candidate_ids.insert(
                id.clone(),
                CandidateScore {
                    score: rrf_score,
                    keyword_rank: Some(*lex_rank),
                    semantic_rank: sem_rank,
                    vector_sim: sem_sim,
                },
            );
        }

        for (id, (sem_rank, sem_sim)) in &semantic_ranks {
            if scored_candidate_ids.contains_key(id) {
                continue;
            }
            let rrf_score = 1.0 / (rrf_k + *sem_rank as f64);
            scored_candidate_ids.insert(
                id.clone(),
                CandidateScore {
                    score: rrf_score,
                    keyword_rank: None,
                    semantic_rank: Some(*sem_rank),
                    vector_sim: Some(*sem_sim),
                },
            );
        }

        if scored_candidate_ids.is_empty() {
            return Ok(SearchResult {
                results: Vec::new(),
                count: 0,
                ranking: if maybe_query_vector.is_some() {
                    "reciprocal_rank_fusion".to_string()
                } else {
                    "lexical_only".to_string()
                },
                telemetry: Some(SearchTelemetry {
                    lexical_ms: lex_time,
                    semantic_ms: sem_time,
                    total_ms: total_start.elapsed().as_secs_f64() * 1000.0,
                    candidate_count: 0,
                }),
            });
        }

        struct CandidateEntry {
            id: String,
            score: f64,
            keyword_rank: Option<usize>,
            semantic_rank: Option<usize>,
            vector_sim: Option<f32>,
        }

        // Rank and select top-K IDs
        let mut sorted_candidates: Vec<CandidateEntry> = scored_candidate_ids
            .into_iter()
            .map(|(id, cs)| CandidateEntry {
                id,
                score: cs.score,
                keyword_rank: cs.keyword_rank,
                semantic_rank: cs.semantic_rank,
                vector_sim: cs.vector_sim,
            })
            .collect();

        sorted_candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
        if sorted_candidates.len() > limit {
            sorted_candidates.truncate(limit);
        }

        // Phase 2: Point hydration - fetch ONLY the winning entries from SQLite
        let mut hits = Vec::with_capacity(sorted_candidates.len());
        for candidate in sorted_candidates {
            if let Some(entry) = self.get(&candidate.id)? {
                // Apply scope/tag filters if entry was retrieved purely through lexical search
                if let Some(req_scope) = project {
                    if entry.project_scope.as_deref() != Some(req_scope) {
                        continue;
                    }
                }
                if !tags.is_empty() {
                    let has_all_tags = tags
                        .iter()
                        .all(|req_tag| entry.tags.iter().any(|t| t.eq_ignore_ascii_case(req_tag)));
                    if !has_all_tags {
                        continue;
                    }
                }

                hits.push(SearchHit {
                    entry,
                    score: candidate.score,
                    keyword_rank: candidate.keyword_rank,
                    semantic_rank: candidate.semantic_rank,
                    telemetry: Some(HitTelemetry {
                        vector_similarity: candidate.vector_sim,
                        bm25_rank: candidate.keyword_rank,
                    }),
                });
            }
        }

        let total_time = total_start.elapsed().as_secs_f64() * 1000.0;
        let ranking = if maybe_query_vector.is_some() {
            "reciprocal_rank_fusion".to_string()
        } else {
            "lexical_only".to_string()
        };

        Ok(SearchResult {
            count: hits.len(),
            results: hits,
            ranking,
            telemetry: Some(SearchTelemetry {
                lexical_ms: lex_time,
                semantic_ms: sem_time,
                total_ms: total_time,
                candidate_count: lexical_ranks.len() + semantic_ranks.len(),
            }),
        })
    }

    /// Roll back an entry to a historical revision.
    pub fn rollback_entry(&self, entry_id: &str, target_version: u32) -> Result<KnowledgeEntry> {
        let conn = self
            .connection
            .lock()
            .map_err(|e| anyhow::anyhow!("DB lock error: {e}"))?;

        let hist: KnowledgeHistoryEntry = conn
            .query_row(
                "SELECT history_id, entry_id, version, operation, title, kind, context, content, verification, tags, project_scope, timestamp
                 FROM knowledge_history WHERE entry_id = ?1 AND version = ?2",
                params![entry_id, target_version],
                |row| {
                    let tags_raw: String = row.get(9)?;
                    let tags: Vec<String> = serde_json::from_str(&tags_raw).unwrap_or_default();
                    Ok(KnowledgeHistoryEntry {
                        history_id: row.get(0)?,
                        entry_id: row.get(1)?,
                        version: row.get(2)?,
                        operation: row.get(3)?,
                        title: row.get(4)?,
                        kind: row.get(5)?,
                        context: row.get(6)?,
                        content: row.get(7)?,
                        verification: row.get(8)?,
                        tags,
                        project_scope: row.get(10)?,
                        timestamp: row.get(11)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("History version {target_version} not found for entry '{entry_id}'"))?;

        drop(conn);

        let kind = EntryKind::parse(&hist.kind).unwrap_or(EntryKind::Workflow);
        let (restored, _) = self.save(
            Some(entry_id),
            &hist.title,
            &kind,
            &hist.context,
            &hist.content,
            &hist.verification,
            &hist.tags,
            hist.project_scope.as_deref(),
            &[],
            None,
        )?;

        Ok(restored)
    }

    /// Diagnostics and statistics.
    pub fn stats(&self) -> Result<StatsResult> {
        let conn = self
            .connection
            .lock()
            .map_err(|e| anyhow::anyhow!("DB lock error: {e}"))?;

        let total_entries: usize =
            conn.query_row("SELECT COUNT(*) FROM knowledge", [], |r| r.get(0))?;

        let total_history_revisions: usize = conn
            .query_row("SELECT COUNT(*) FROM knowledge_history", [], |r| r.get(0))
            .unwrap_or(0);

        let mut stmt = conn.prepare(
            "SELECT COALESCE(project_scope, 'global'), COUNT(*) FROM knowledge GROUP BY project_scope ORDER BY COUNT(*) DESC",
        )?;
        let projects = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let db_size_bytes = std::fs::metadata(&self.db_path)
            .map(|m| m.len())
            .unwrap_or(0);

        let semantic_active = self
            .vector_index
            .lock()
            .map(|vi| !vi.is_empty())
            .unwrap_or(false);

        Ok(StatsResult {
            total_entries,
            total_history_revisions,
            projects,
            db_size_bytes,
            semantic_active,
        })
    }
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<KnowledgeEntry> {
    Ok(KnowledgeEntry {
        id: row.get(0)?,
        title: row.get(1)?,
        kind: row.get(2)?,
        context: row.get(3)?,
        content: row.get(4)?,
        verification: row.get(5)?,
        tags: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
        project_scope: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        dedup_key: row.get(10)?,
    })
}

pub fn vector_to_bytes(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migrations_and_history_audit() {
        let store = KnowledgeStore::open(Path::new(":memory:")).unwrap();

        // 1. Initial save
        let (first, dedup1) = store
            .save(
                None,
                "Configuring Postgres connection pool",
                &EntryKind::Workflow,
                "Production web service",
                "Set max_connections=50",
                "Zero timeout errors",
                &["postgres".to_string()],
                Some("backend"),
                &[1.0, 0.0],
                Some("backend:postgres-pool"),
            )
            .unwrap();
        assert!(!dedup1);

        // 2. Update with deduplication key -> must record revision 1 in history
        let (second, dedup2) = store
            .save(
                None,
                "Configuring Postgres connection pool",
                &EntryKind::Workflow,
                "Production web service high load",
                "Set max_connections=100 and idle_timeout=30s",
                "Connection errors eliminated under 10k rps load test",
                &["postgres".to_string(), "tuning".to_string()],
                Some("backend"),
                &[0.0, 1.0],
                Some("backend:postgres-pool"),
            )
            .unwrap();
        assert!(dedup2);
        assert_eq!(first.id, second.id);

        let history = store.get_history(&first.id).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].version, 1);
        assert_eq!(history[0].content, "Set max_connections=50");

        // 3. Rollback
        let restored = store.rollback_entry(&first.id, 1).unwrap();
        assert_eq!(restored.content, "Set max_connections=50");
    }

    #[test]
    fn saving_and_replacing_updates_full_text_index() {
        let store = KnowledgeStore::open(Path::new(":memory:")).unwrap();
        let (original, dedup1) = store
            .save(
                None,
                "Recover a failed database migration",
                &EntryKind::ResolvedFailure,
                "The migration reports a lock timeout",
                "Stop the competing job and retry the migration",
                "The migration completed and the service became healthy",
                &["database".to_owned()],
                Some("projectscopeuniquetoken"),
                &[1.0, 0.0],
                None,
            )
            .unwrap();
        assert!(!dedup1);
        assert_eq!(
            store.keyword_search("migration", 10).unwrap()[0].0,
            original.id
        );
        assert!(store
            .keyword_search("projectscopeuniquetoken", 10)
            .unwrap()
            .is_empty());

        let (updated, dedup2) = store
            .save(
                Some(&original.id),
                "Recover a failed database migration",
                &EntryKind::Workflow,
                "Use the documented maintenance window",
                "Pause ingestion, apply the migration, then resume ingestion",
                "Migration and ingestion health checks both pass",
                &["database".to_owned()],
                Some("projectscopeuniquetoken"),
                &[0.0, 1.0],
                None,
            )
            .unwrap();
        assert!(!dedup2);
        assert_eq!(updated.id, original.id);
        assert_eq!(updated.created_at, original.created_at);
        assert!(store.keyword_search("competing", 10).unwrap().is_empty());
        assert_eq!(
            store.keyword_search("ingestion", 10).unwrap()[0].0,
            original.id
        );
        assert_eq!(
            store
                .all(Some("projectscopeuniquetoken"), &["database".to_owned()])
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn deduplication_updates_existing_entry_in_place() {
        let store = KnowledgeStore::open(Path::new(":memory:")).unwrap();
        let key = crate::dedup::generate_dedup_key(
            Some("infra-repo"),
            "Deploy Helm Chart with custom values",
        );
        assert_eq!(key, "infra-repo:deploy-helm-chart-with-custom-values");

        let (first, dedup1) = store
            .save(
                None,
                "Deploy Helm Chart with custom values",
                &EntryKind::Workflow,
                "When updating production clusters",
                "helm upgrade --install ...",
                "Helm release status shows deployed",
                &["helm".to_owned()],
                Some("infra-repo"),
                &[1.0, 0.0],
                Some(&key),
            )
            .unwrap();
        assert!(!dedup1);

        let (second, dedup2) = store
            .save(
                None,
                "Deploy Helm Chart with custom values",
                &EntryKind::Workflow,
                "Updated procedure for production clusters",
                "helm upgrade --install --wait ...",
                "All pods are in Running state",
                &["helm".to_owned()],
                Some("infra-repo"),
                &[0.0, 1.0],
                Some(&key),
            )
            .unwrap();
        assert!(dedup2);
        assert_eq!(first.id, second.id);
        assert_eq!(first.created_at, second.created_at);
        assert_eq!(store.all(None, &[]).unwrap().len(), 1);
        assert_eq!(second.content, "helm upgrade --install --wait ...");
    }

    #[test]
    fn vector_storage_round_trips_and_cosine_scores_match() {
        use crate::simd::{bytes_to_vector, cosine_similarity, vector_to_bytes};
        let vector = vec![0.25, -0.5, 0.75];
        assert_eq!(bytes_to_vector(&vector_to_bytes(&vector)), vector);
        assert!((cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!((cosine_similarity(&[1.0, 0.0], &[0.0, 1.0])).abs() < 1e-6);
    }

    #[test]
    fn entries_and_fts_index_survive_reopening_database() {
        let path = std::env::temp_dir().join(format!("atlascode-test-{}.db", Uuid::new_v4()));
        {
            let store = KnowledgeStore::open(&path).unwrap();
            store
                .save(
                    None,
                    "Rebuild the local search index",
                    &EntryKind::Workflow,
                    "After a database restore",
                    "Run the index rebuild command",
                    "The search command returns the restored records",
                    &["indexing".to_owned()],
                    None,
                    &[0.5, 0.5],
                    None,
                )
                .unwrap();
        }
        {
            let store = KnowledgeStore::open(&path).unwrap();
            assert_eq!(store.keyword_search("restore", 5).unwrap().len(), 1);
            assert_eq!(store.all(None, &[]).unwrap().len(), 1);
        }
        std::fs::remove_file(&path).unwrap();
    }
}
