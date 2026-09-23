use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Workflow,
    ResolvedFailure,
}

impl EntryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Workflow => "workflow",
            Self::ResolvedFailure => "resolved_failure",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "workflow" => Ok(Self::Workflow),
            "resolved_failure" => Ok(Self::ResolvedFailure),
            other => Err(format!("Unknown knowledge kind '{other}'")),
        }
    }
}

impl std::fmt::Display for EntryKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dedup_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeHistoryEntry {
    pub history_id: i64,
    pub entry_id: String,
    pub version: u32,
    pub operation: String,
    pub title: String,
    pub kind: String,
    pub context: String,
    pub content: String,
    pub verification: String,
    pub tags: Vec<String>,
    pub project_scope: Option<String>,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HitTelemetry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector_similarity: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bm25_rank: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    #[serde(flatten)]
    pub entry: KnowledgeEntry,
    pub score: f64,
    pub keyword_rank: Option<usize>,
    pub semantic_rank: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<HitTelemetry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchTelemetry {
    pub lexical_ms: f64,
    pub semantic_ms: f64,
    pub total_ms: f64,
    pub candidate_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub results: Vec<SearchHit>,
    pub count: usize,
    pub ranking: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<SearchTelemetry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveKnowledgeResult {
    pub saved: bool,
    pub deduplicated: bool,
    pub entry: KnowledgeEntry,
    #[serde(default)]
    pub secrets_redacted: usize,
    #[serde(default)]
    pub merged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsResult {
    pub total_entries: usize,
    pub total_history_revisions: usize,
    pub projects: Vec<(String, usize)>,
    pub db_size_bytes: u64,
    pub semantic_active: bool,
}
