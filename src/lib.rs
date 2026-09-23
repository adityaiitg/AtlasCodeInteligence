pub mod cli;
pub mod dedup;
pub mod embedder;
pub mod error;
pub mod mcp;
pub mod security;
pub mod simd;
pub mod store;
pub mod types;
pub mod vector_index;

pub use embedder::{Embedder, MockEmbedder, Model2Vec, Model2VecEmbedder};
pub use error::{Error, McpError, SecurityError, StoreError};
pub use store::KnowledgeStore;
pub use types::{EntryKind, KnowledgeEntry, KnowledgeHistoryEntry, SearchHit, SearchResult};
