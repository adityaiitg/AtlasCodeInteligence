use serde_json::json;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Store error: {0}")]
    Store(#[from] StoreError),

    #[error("Embedding error: {0}")]
    Embed(#[from] EmbedError),

    #[error("Security error: {0}")]
    Security(#[from] SecurityError),

    #[error("MCP protocol error: {0}")]
    Mcp(#[from] McpError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

#[derive(Error, Debug)]
pub enum StoreError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Entry not found: {0}")]
    NotFound(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Database migration error: {0}")]
    Migration(String),

    #[error("Invalid operation: {0}")]
    InvalidOperation(String),
}

#[derive(Error, Debug)]
pub enum EmbedError {
    #[error("Model loading error: {0}")]
    ModelLoad(String),

    #[error("Tokenization error: {0}")]
    Tokenization(String),

    #[error("Model lock poisoned: {0}")]
    LockPoisoned(String),

    #[error("Model weights error: {0}")]
    Weights(String),
}

#[derive(Error, Debug)]
pub enum SecurityError {
    #[error("Field '{field}' exceeds maximum allowed length of {max} bytes (actual: {actual})")]
    InputTooLong {
        field: String,
        max: usize,
        actual: usize,
    },

    #[error("Field '{field}' cannot be empty")]
    EmptyField { field: String },

    #[error("Too many tags: provided {actual}, maximum allowed is {max}")]
    TooManyTags { actual: usize, max: usize },

    #[error("Invalid tag '{tag}': {reason}")]
    InvalidTag { tag: String, reason: String },

    #[error("Invalid database path: {0}")]
    InvalidPath(String),

    #[error("Input line exceeds maximum limit of {0} bytes")]
    LineTooLong(usize),
}

#[derive(Error, Debug)]
pub enum McpError {
    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Method not found: {0}")]
    MethodNotFound(String),

    #[error("Invalid params: {0}")]
    InvalidParams(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl McpError {
    pub fn code(&self) -> i64 {
        match self {
            Self::Parse(_) => -32700,
            Self::InvalidRequest(_) => -32600,
            Self::MethodNotFound(_) => -32601,
            Self::InvalidParams(_) => -32602,
            Self::Internal(_) => -32603,
        }
    }

    pub fn to_json_rpc(&self, id: &serde_json::Value) -> serde_json::Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": self.code(),
                "message": self.to_string()
            }
        })
    }
}
