use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Workflow,
    ResolvedFailure,
}

impl EntryKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Workflow => "workflow",
            Self::ResolvedFailure => "resolved_failure",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub context: String,
    pub content: String,
    pub verification: String,
    pub tags: Vec<String>,
    pub project_scope: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    #[serde(flatten)]
    pub entry: KnowledgeEntry,
    pub score: f64,
    pub keyword_rank: Option<usize>,
    pub semantic_rank: Option<usize>,
}

pub struct KnowledgeStore {
    connection: Connection,
}

impl KnowledgeStore {
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path)
            .with_context(|| format!("Opening knowledge database at {}", path.display()))?;
        if path != Path::new(":memory:") {
            connection.query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0))?;
        }
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute_batch(
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
                updated_at TEXT NOT NULL
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_fts USING fts5(
                id UNINDEXED, title, kind, context, content, verification, tags, project_scope UNINDEXED,
                tokenize='porter unicode61'
            );",
        )?;
        Ok(Self { connection })
    }

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
    ) -> Result<KnowledgeEntry> {
        let id = entry_id
            .map(str::to_owned)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let timestamp = chrono::Utc::now().to_rfc3339();
        let existing_created_at: Option<String> = self
            .connection
            .query_row(
                "SELECT created_at FROM knowledge WHERE id = ?1",
                [&id],
                |row| row.get(0),
            )
            .optional()?;
        if entry_id.is_some() && existing_created_at.is_none() {
            anyhow::bail!("Knowledge entry '{id}' does not exist");
        }
        let created_at = existing_created_at.unwrap_or_else(|| timestamp.clone());
        let tags_json = serde_json::to_string(tags)?;
        let embedding_bytes = vector_to_bytes(embedding);
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute("DELETE FROM knowledge_fts WHERE id = ?1", [&id])?;
        transaction.execute(
            "INSERT INTO knowledge (id, title, kind, context, content, verification, tags, project_scope, embedding, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET title=excluded.title, kind=excluded.kind, context=excluded.context,
                content=excluded.content, verification=excluded.verification, tags=excluded.tags,
                project_scope=excluded.project_scope, embedding=excluded.embedding, updated_at=excluded.updated_at",
            params![id, title, kind.as_str(), context, content, verification, tags_json, project_scope, embedding_bytes, created_at, timestamp],
        )?;
        transaction.execute(
            "INSERT INTO knowledge_fts (id, title, kind, context, content, verification, tags, project_scope)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, title, kind.as_str(), context, content, verification, tags.join(" "), project_scope.unwrap_or("")],
        )?;
        transaction.commit()?;
        self.get(&id)?.context("Saved entry disappeared")
    }

    pub fn delete(&self, id: &str) -> Result<bool> {
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute("DELETE FROM knowledge_fts WHERE id = ?1", [id])?;
        let changed = transaction.execute("DELETE FROM knowledge WHERE id = ?1", [id])?;
        transaction.commit()?;
        Ok(changed > 0)
    }

    pub fn get(&self, id: &str) -> Result<Option<KnowledgeEntry>> {
        self.connection
            .query_row(
                "SELECT id, title, kind, context, content, verification, tags, project_scope, created_at, updated_at
                 FROM knowledge WHERE id = ?1",
                [id],
                row_to_entry,
            )
            .optional()
            .context("Reading knowledge entry")
    }

    pub fn all(
        &self,
        project: Option<&str>,
        tags: &[String],
    ) -> Result<Vec<(KnowledgeEntry, Vec<f32>)>> {
        let mut statement = self.connection.prepare(
            "SELECT id, title, kind, context, content, verification, tags, project_scope, embedding, created_at, updated_at
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
                };
                let vector = bytes_to_vector(&row.get::<_, Vec<u8>>(8)?);
                Ok((entry, vector))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(entries
            .into_iter()
            .filter(|(entry, _)| {
                project.map_or(true, |scope| entry.project_scope.as_deref() == Some(scope))
                    && tags.iter().all(|tag| {
                        entry
                            .tags
                            .iter()
                            .any(|stored| stored.eq_ignore_ascii_case(tag))
                    })
            })
            .collect())
    }

    pub fn keyword_search(&self, query: &str, limit: usize) -> Result<Vec<(String, f64)>> {
        let terms = query
            .split(|character: char| !character.is_alphanumeric() && character != '_')
            .filter(|term| !term.is_empty())
            .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
            .collect::<Vec<_>>();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let fts_query = terms.join(" OR ");
        let mut statement = self.connection.prepare(
            "SELECT id, bm25(knowledge_fts) AS rank FROM knowledge_fts
             WHERE knowledge_fts MATCH ?1 ORDER BY rank LIMIT ?2",
        )?;
        let rows = statement.query_map(params![fts_query, limit as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("Running keyword search")
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
    })
}

pub fn vector_to_bytes(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

pub fn bytes_to_vector(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().expect("four bytes")))
        .collect()
}

pub fn cosine_similarity(left: &[f32], right: &[f32]) -> f64 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    let (dot, left_norm, right_norm) = left.iter().zip(right).fold(
        (0.0f64, 0.0f64, 0.0f64),
        |(dot, left_norm, right_norm), (a, b)| {
            let a = *a as f64;
            let b = *b as f64;
            (dot + a * b, left_norm + a * a, right_norm + b * b)
        },
    );
    if left_norm == 0.0 || right_norm == 0.0 {
        0.0
    } else {
        dot / (left_norm.sqrt() * right_norm.sqrt())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_and_replacing_updates_full_text_index() {
        let store = KnowledgeStore::open(Path::new(":memory:")).unwrap();
        let original = store
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
            )
            .unwrap();
        assert_eq!(
            store.keyword_search("migration", 10).unwrap()[0].0,
            original.id
        );
        assert!(store
            .keyword_search("projectscopeuniquetoken", 10)
            .unwrap()
            .is_empty());

        let updated = store
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
            )
            .unwrap();

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
    fn vector_storage_round_trips_and_cosine_scores_match() {
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
