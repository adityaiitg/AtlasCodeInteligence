use crate::embedder::Model2Vec;
use crate::store::{cosine_similarity, EntryKind, KnowledgeStore, SearchHit};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

const SERVER_NAME: &str = "atlascode-inteligence";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const PROTOCOL_VERSION: &str = "2024-11-05";

pub fn handle_request(
    request: &Value,
    store: &KnowledgeStore,
    embedder: &Model2Vec,
) -> Option<Value> {
    let id = request.get("id");
    let method = request.get("method").and_then(Value::as_str);
    let Some(method) = method else {
        return id.map(|id| error_response(id, -32600, "Invalid request: missing method"));
    };
    let Some(request_id) = id else {
        return None;
    };
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    match method {
        "initialize" => Some(success_response(
            request_id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION }
            }),
        )),
        "notifications/initialized" | "notifications/cancelled" => None,
        "ping" => Some(success_response(request_id, json!({}))),
        "tools/list" => Some(success_response(
            request_id,
            json!({ "tools": tool_definitions() }),
        )),
        "tools/call" => Some(call_tool(request_id, &params, store, embedder)),
        _ => id.map(|id| error_response(id, -32601, &format!("Method not found: {method}"))),
    }
}

pub fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn success_response(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "search_knowledge",
            "description": "Search saved workflows and resolved engineering failures using keyword and semantic retrieval. Search before repeating a task.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Natural language question, error text, or exact keyword" },
                    "limit": { "type": "integer", "default": 5, "minimum": 1, "maximum": 50 },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "project_scope": { "type": "string" }
                },
                "required": ["query"]
            }
        },
        {
            "name": "save_knowledge",
            "description": "Save a confirmed reusable workflow or a resolved failure after verifying the fix. Search first; provide entry_id to replace a stale or corrected entry in place.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "entry_id": { "type": "string", "description": "Existing entry ID to replace; omit to create a new entry" },
                    "title": { "type": "string" },
                    "kind": { "type": "string", "enum": ["workflow", "resolved_failure"] },
                    "context": { "type": "string", "description": "When to use the workflow, or the observed failure and its context" },
                    "content": { "type": "string", "description": "Reusable steps, or root cause and resolution" },
                    "verification": { "type": "string", "description": "Evidence the workflow or fix worked" },
                    "tags": { "type": "array", "items": { "type": "string" }, "default": [] },
                    "project_scope": { "type": "string", "description": "Optional repository or project name" }
                },
                "required": ["title", "kind", "context", "content", "verification"]
            }
        },
        {
            "name": "delete_knowledge",
            "description": "Delete a saved knowledge entry by ID.",
            "inputSchema": {
                "type": "object",
                "properties": { "entry_id": { "type": "string" } },
                "required": ["entry_id"]
            }
        }
    ])
}

fn call_tool(id: &Value, params: &Value, store: &KnowledgeStore, embedder: &Model2Vec) -> Value {
    let tool_name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let result = match tool_name {
        "search_knowledge" => search_knowledge(&args, store, embedder),
        "save_knowledge" => save_knowledge(&args, store, embedder),
        "delete_knowledge" => delete_knowledge(&args, store),
        _ => return error_response(id, -32602, &format!("Unknown tool: {tool_name}")),
    };
    match result {
        Ok(value) => success_response(
            id,
            json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default() }],
                "structuredContent": value
            }),
        ),
        Err(error) => success_response(
            id,
            json!({
                "isError": true,
                "content": [{ "type": "text", "text": error.to_string() }]
            }),
        ),
    }
}

fn search_knowledge(
    args: &Value,
    store: &KnowledgeStore,
    embedder: &Model2Vec,
) -> anyhow::Result<Value> {
    let query = required_string(args, "query")?;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(5)
        .clamp(1, 50) as usize;
    let project = args.get("project_scope").and_then(Value::as_str);
    let tags = string_array(args.get("tags"))?;
    let entries = store.all(project, &tags)?;
    if entries.is_empty() {
        return Ok(json!({ "results": [], "count": 0 }));
    }

    let eligible_ids = entries
        .iter()
        .map(|(entry, _)| entry.id.as_str())
        .collect::<HashSet<_>>();
    let lexical = store.keyword_search(query, i64::MAX as usize)?;
    let lexical_ranks = lexical
        .into_iter()
        .filter(|(id, _)| eligible_ids.contains(id.as_str()))
        .enumerate()
        .map(|(index, (id, _))| (id, index + 1))
        .collect::<HashMap<_, _>>();
    let query_vector = embedder.embed(query)?;
    let mut semantic = entries
        .iter()
        .map(|(entry, vector)| (entry.id.clone(), cosine_similarity(&query_vector, vector)))
        .collect::<Vec<_>>();
    semantic.sort_by(|left, right| right.1.total_cmp(&left.1));
    let semantic_ranks = semantic
        .iter()
        .enumerate()
        .map(|(index, (id, _))| (id.clone(), index + 1))
        .collect::<HashMap<_, _>>();

    let mut hits = entries
        .into_iter()
        .map(|(entry, _)| {
            let keyword_rank = lexical_ranks.get(&entry.id).copied();
            let semantic_rank = semantic_ranks.get(&entry.id).copied();
            let score = keyword_rank.map_or(0.0, |rank| 1.0 / (60 + rank) as f64)
                + semantic_rank.map_or(0.0, |rank| 1.0 / (60 + rank) as f64);
            SearchHit {
                entry,
                score,
                keyword_rank,
                semantic_rank,
            }
        })
        .filter(|hit| hit.keyword_rank.is_some() || hit.semantic_rank.is_some())
        .collect::<Vec<_>>();
    hits.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| right.entry.updated_at.cmp(&left.entry.updated_at))
    });
    hits.truncate(limit);
    Ok(json!({ "results": hits, "count": hits.len(), "ranking": "reciprocal_rank_fusion" }))
}

fn save_knowledge(
    args: &Value,
    store: &KnowledgeStore,
    embedder: &Model2Vec,
) -> anyhow::Result<Value> {
    let title = required_string(args, "title")?;
    let kind = match required_string(args, "kind")? {
        "workflow" => EntryKind::Workflow,
        "resolved_failure" => EntryKind::ResolvedFailure,
        other => anyhow::bail!("Unsupported knowledge kind '{other}'"),
    };
    let context = required_string(args, "context")?;
    let content = required_string(args, "content")?;
    let verification = required_string(args, "verification")?;
    let tags = string_array(args.get("tags"))?;
    let project_scope = args.get("project_scope").and_then(Value::as_str);
    let entry_id = args.get("entry_id").and_then(Value::as_str);
    let embedded_text = format!(
        "{title}\n{context}\n{content}\n{verification}\n{}",
        tags.join(" ")
    );
    let vector = embedder.embed(&embedded_text)?;
    let entry = store.save(
        entry_id,
        title,
        &kind,
        context,
        content,
        verification,
        &tags,
        project_scope,
        &vector,
    )?;
    Ok(json!({ "saved": true, "entry": entry }))
}

fn delete_knowledge(args: &Value, store: &KnowledgeStore) -> anyhow::Result<Value> {
    let entry_id = required_string(args, "entry_id")?;
    Ok(json!({ "deleted": store.delete(entry_id)?, "entry_id": entry_id }))
}

fn required_string<'a>(args: &'a Value, key: &str) -> anyhow::Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("Missing required string argument '{key}'"))
}

fn string_array(value: Option<&Value>) -> anyhow::Result<Vec<String>> {
    match value {
        None => Ok(Vec::new()),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| anyhow::anyhow!("Expected string values in array"))
            })
            .collect(),
        Some(_) => anyhow::bail!("Expected an array of strings"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_search_save_and_delete_tools() {
        let definitions = tool_definitions();
        let names = definitions
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_owned))
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            ["search_knowledge", "save_knowledge", "delete_knowledge"]
        );
    }

    #[test]
    fn ignores_initialized_notification() {
        let request = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        let store = KnowledgeStore::open(std::path::Path::new(":memory:")).unwrap();
        let embedder = Model2Vec::new();
        assert!(handle_request(&request, &store, &embedder).is_none());
    }
}
