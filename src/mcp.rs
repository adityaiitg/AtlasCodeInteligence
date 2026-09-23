use crate::dedup::{find_near_duplicate, generate_dedup_key, smart_merge_entry};
use crate::embedder::Embedder;
use crate::security::{redact_secrets, validate_string_bound, validate_tags, InputLimits};
use crate::store::KnowledgeStore;
use crate::types::{EntryKind, SearchResult};
use serde_json::{json, Value};

const SERVER_NAME: &str = "atlascode-inteligence";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const PROTOCOL_VERSION: &str = "2024-11-05";

pub fn handle_request(
    request: &Value,
    store: &KnowledgeStore,
    embedder: &dyn Embedder,
) -> Option<Value> {
    // 1. Support batch requests
    if let Some(batch) = request.as_array() {
        let mut responses = Vec::new();
        for req in batch {
            if let Some(resp) = handle_single_request(req, store, embedder) {
                responses.push(resp);
            }
        }
        return if responses.is_empty() {
            None
        } else {
            Some(Value::Array(responses))
        };
    }

    handle_single_request(request, store, embedder)
}

fn handle_single_request(
    request: &Value,
    store: &KnowledgeStore,
    embedder: &dyn Embedder,
) -> Option<Value> {
    let id = request.get("id");

    // Optional protocol version validation
    if let Some(ver) = request.get("jsonrpc").and_then(Value::as_str) {
        if ver != "2.0" {
            return id.map(|id| error_response(id, -32600, "Invalid JSON-RPC protocol version"));
        }
    }

    let method = request.get("method").and_then(Value::as_str);
    let Some(method) = method else {
        return id.map(|id| error_response(id, -32600, "Invalid request: missing method"));
    };

    // Notifications (no id)
    let Some(request_id) = id else {
        match method {
            "notifications/initialized" | "notifications/cancelled" => {}
            _ => tracing::debug!("Ignored unhandled notification: {method}"),
        }
        return None;
    };

    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));

    match method {
        "initialize" => Some(success_response(
            request_id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {},
                    "resources": {},
                    "prompts": {}
                },
                "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION }
            }),
        )),
        "notifications/initialized" | "notifications/cancelled" => None,
        "ping" => Some(success_response(request_id, json!({}))),

        // Tools
        "tools/list" => Some(success_response(
            request_id,
            json!({ "tools": tool_definitions() }),
        )),
        "tools/call" => Some(call_tool(request_id, &params, store, embedder)),

        // Resources
        "resources/list" => Some(success_response(
            request_id,
            json!({ "resources": resource_definitions() }),
        )),
        "resources/read" => Some(read_resource(request_id, &params, store)),

        // Prompts
        "prompts/list" => Some(success_response(
            request_id,
            json!({ "prompts": prompt_definitions() }),
        )),
        "prompts/get" => Some(get_prompt(request_id, &params)),

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
            "description": "Search saved workflows and resolved engineering failures using hybrid BM25 and semantic vector retrieval. ALWAYS call before attempting complex debugging or executing risky migrations.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Natural language question, compiler error code (e.g. E0382, TS2322), or exact keywords" },
                    "limit": { "type": "integer", "default": 5, "minimum": 1, "maximum": 50, "description": "Maximum number of entries to return" },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "Optional tag filters (e.g. ['postgres', 'docker'])" },
                    "project_scope": { "type": "string", "description": "Optional repository or project name for scope isolation" },
                    "compact": { "type": "boolean", "default": false, "description": "If true, returns a concise summary of results to conserve context window tokens" }
                },
                "required": ["query"]
            }
        },
        {
            "name": "save_knowledge",
            "description": "Save a reusable engineering workflow or verified failure resolution. Searches first for duplicates; merges new verification evidence and tags automatically.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Clear, descriptive title summarizing the workflow or problem resolved" },
                    "kind": { "type": "string", "enum": ["workflow", "resolved_failure"], "description": "Entry category: 'workflow' for reusable procedures, 'resolved_failure' for bug/error fixes" },
                    "context": { "type": "string", "description": "Environment, trigger conditions, symptoms, or observed error trace" },
                    "content": { "type": "string", "description": "Step-by-step resolution, code modifications, or command run to fix the problem" },
                    "verification": { "type": "string", "description": "Concrete evidence demonstrating the fix worked (test results, command output, metrics)" },
                    "tags": { "type": "array", "items": { "type": "string" }, "default": [], "description": "Descriptive tags (e.g. ['database', 'postgres', 'migration'])" },
                    "project_scope": { "type": "string", "description": "Optional repository or project name" },
                    "entry_id": { "type": "string", "description": "Existing entry ID to explicitly update or replace in place" },
                    "dedup_key": { "type": "string", "description": "Stable deduplication slug or error code. If omitted, derived from project_scope and title." }
                },
                "required": ["title", "kind", "context", "content", "verification"]
            }
        },
        {
            "name": "delete_knowledge",
            "description": "Delete a saved knowledge entry by its unique ID.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "entry_id": { "type": "string", "description": "The unique ID of the entry to delete" }
                },
                "required": ["entry_id"]
            }
        }
    ])
}

fn resource_definitions() -> Value {
    json!([
        {
            "uri": "knowledge://recent",
            "name": "Recently Updated Knowledge",
            "description": "The 20 most recently saved or updated workflows and failure resolutions",
            "mimeType": "text/markdown"
        },
        {
            "uri": "knowledge://stats",
            "name": "Knowledge Base Statistics",
            "description": "Operational metrics: entry counts, project breakdown, and database size",
            "mimeType": "application/json"
        }
    ])
}

fn prompt_definitions() -> Value {
    json!([
        {
            "name": "troubleshoot_issue",
            "description": "Search for relevant engineering knowledge and guide troubleshooting an error",
            "arguments": [
                { "name": "error_message", "description": "The error message or failure description", "required": true }
            ]
        },
        {
            "name": "document_solution",
            "description": "Template for verifying and documenting a resolved engineering problem into AtlasCodeInteligence",
            "arguments": [
                { "name": "title", "description": "The title of the solution", "required": true }
            ]
        }
    ])
}

fn call_tool(id: &Value, params: &Value, store: &KnowledgeStore, embedder: &dyn Embedder) -> Value {
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
        Ok((markdown_text, structured_value)) => success_response(
            id,
            json!({
                "content": [{ "type": "text", "text": markdown_text }],
                "structuredContent": structured_value
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
    embedder: &dyn Embedder,
) -> anyhow::Result<(String, Value)> {
    let limits = InputLimits::default();
    let query_raw = required_string(args, "query")?;
    let query = validate_string_bound("query", query_raw, limits.max_query_len)?;

    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(5)
        .clamp(1, 50) as usize;

    let compact = args
        .get("compact")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let project = args.get("project_scope").and_then(Value::as_str);
    let raw_tags = string_array(args.get("tags"))?;
    let tags = validate_tags(&raw_tags, &limits)?;

    let maybe_query_vec = embedder.embed(query)?;
    let search_result =
        store.hybrid_search(query, maybe_query_vec.as_deref(), project, &tags, limit)?;

    // Format rich markdown for content[0].text
    let markdown = format_search_markdown(&search_result, query, compact);

    // Structure content as typed JSON matching existing contract
    let structured = serde_json::to_value(&search_result)?;

    Ok((markdown, structured))
}

fn save_knowledge(
    args: &Value,
    store: &KnowledgeStore,
    embedder: &dyn Embedder,
) -> anyhow::Result<(String, Value)> {
    let limits = InputLimits::default();

    let raw_title = required_string(args, "title")?;
    let raw_context = required_string(args, "context")?;
    let raw_content = required_string(args, "content")?;
    let raw_verification = required_string(args, "verification")?;

    let title_bounded = validate_string_bound("title", raw_title, limits.max_title_len)?;
    let context_bounded = validate_string_bound("context", raw_context, limits.max_context_len)?;
    let content_bounded = validate_string_bound("content", raw_content, limits.max_content_len)?;
    let verification_bounded = validate_string_bound(
        "verification",
        raw_verification,
        limits.max_verification_len,
    )?;

    let raw_tags = string_array(args.get("tags"))?;
    let tags = validate_tags(&raw_tags, &limits)?;

    let project_scope = args.get("project_scope").and_then(Value::as_str);
    let entry_id = args.get("entry_id").and_then(Value::as_str);
    let explicit_dedup_key = args.get("dedup_key").and_then(Value::as_str);

    let kind_str = required_string(args, "kind")?;
    let kind = match kind_str {
        "workflow" => EntryKind::Workflow,
        "resolved_failure" => EntryKind::ResolvedFailure,
        other => anyhow::bail!("Unsupported knowledge kind '{other}'"),
    };

    // Automated Secret Redaction prior to embedding & persisting
    let (clean_title, r1) = redact_secrets(title_bounded);
    let (clean_context, r2) = redact_secrets(context_bounded);
    let (clean_content, r3) = redact_secrets(content_bounded);
    let (clean_verification, r4) = redact_secrets(verification_bounded);
    let total_secrets_redacted = r1 + r2 + r3 + r4;

    // Deduplication Key Computation
    let canonical_key = generate_dedup_key(project_scope, &clean_title);
    let effective_key = explicit_dedup_key.unwrap_or(&canonical_key);

    // Compute embedding vector
    let embedded_text = format!(
        "{clean_title}\n{clean_context}\n{clean_content}\n{clean_verification}\n{}",
        tags.join(" ")
    );
    let vector = embedder.embed(&embedded_text)?.unwrap_or_default();

    // Check for near-duplicate or dedup match for smart merge
    let mut deduplicated = false;
    let mut merged = false;

    // Check existing by dedup key first
    let existing_by_key = store.get_by_dedup_key(effective_key)?;
    let target_id = if let Some(existing) = &existing_by_key {
        deduplicated = true;
        merged = true;
        let merged_entry = smart_merge_entry(
            existing,
            &clean_title,
            kind.as_str(),
            &clean_context,
            &clean_content,
            &clean_verification,
            &tags,
            project_scope,
            Some(effective_key),
        );
        Some(merged_entry)
    } else if entry_id.is_none() && !vector.is_empty() {
        // Check near duplicate by vector similarity >= 0.92
        let all_entries = store.all(project_scope, &[])?;
        if let Some(near_dup) = find_near_duplicate(&vector, &all_entries, project_scope, 0.92) {
            deduplicated = true;
            merged = true;
            let merged_entry = smart_merge_entry(
                near_dup,
                &clean_title,
                kind.as_str(),
                &clean_context,
                &clean_content,
                &clean_verification,
                &tags,
                project_scope,
                Some(effective_key),
            );
            Some(merged_entry)
        } else {
            None
        }
    } else {
        None
    };

    let (entry, store_dedup) = if let Some(m) = target_id {
        let (saved_entry, _) = store.save(
            Some(&m.id),
            &m.title,
            &kind,
            &m.context,
            &m.content,
            &m.verification,
            &m.tags,
            m.project_scope.as_deref(),
            &vector,
            m.dedup_key.as_deref(),
        )?;
        (saved_entry, true)
    } else {
        store.save(
            entry_id,
            &clean_title,
            &kind,
            &clean_context,
            &clean_content,
            &clean_verification,
            &tags,
            project_scope,
            &vector,
            Some(effective_key),
        )?
    };

    let effective_dedup = deduplicated || store_dedup;

    let markdown = format!(
        "### Knowledge Entry Saved Successfully\n\n\
         - **ID:** `{}`\n\
         - **Title:** {}\n\
         - **Kind:** `{}`\n\
         - **Scope:** `{}`\n\
         - **Status:** {}\n\
         - **Tags:** {}\n\
         {}\n\
         **Resolution Summary:**\n\
         ```\n\
         {}\n\
         ```",
        entry.id,
        entry.title,
        entry.kind,
        entry.project_scope.as_deref().unwrap_or("global"),
        if effective_dedup {
            "Updated in-place (deduplicated/merged)"
        } else {
            "Created new entry"
        },
        if entry.tags.is_empty() {
            "none".to_string()
        } else {
            entry.tags.join(", ")
        },
        if total_secrets_redacted > 0 {
            format!(
                "- **Security:** Redacted {total_secrets_redacted} credentials before persisting"
            )
        } else {
            String::new()
        },
        entry.content.chars().take(300).collect::<String>()
    );

    let structured = json!({
        "saved": true,
        "deduplicated": effective_dedup,
        "merged": merged,
        "secrets_redacted": total_secrets_redacted,
        "entry": entry
    });

    Ok((markdown, structured))
}

fn delete_knowledge(args: &Value, store: &KnowledgeStore) -> anyhow::Result<(String, Value)> {
    let entry_id = required_string(args, "entry_id")?;
    let deleted = store.delete(entry_id)?;

    let markdown = if deleted {
        format!("Successfully deleted knowledge entry `{entry_id}`.")
    } else {
        format!("No knowledge entry found with ID `{entry_id}`.")
    };

    let structured = json!({
        "deleted": deleted,
        "entry_id": entry_id
    });

    Ok((markdown, structured))
}

fn read_resource(id: &Value, params: &Value, store: &KnowledgeStore) -> Value {
    let uri = params.get("uri").and_then(Value::as_str).unwrap_or("");
    if uri == "knowledge://recent" {
        match store.all(None, &[]) {
            Ok(mut entries) => {
                entries.truncate(20);
                let mut md = String::from("# Recent Knowledge Entries\n\n");
                for (e, _) in entries {
                    md.push_str(&format!(
                        "- **{}** (`{}`) [{}]\n  - Context: {}\n  - Updated: {}\n\n",
                        e.title,
                        e.kind,
                        e.project_scope.as_deref().unwrap_or("global"),
                        e.context.chars().take(120).collect::<String>(),
                        e.updated_at
                    ));
                }
                success_response(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "text/markdown",
                            "text": md
                        }]
                    }),
                )
            }
            Err(e) => error_response(id, -32603, &e.to_string()),
        }
    } else if uri == "knowledge://stats" {
        match store.stats() {
            Ok(stats) => success_response(
                id,
                json!({
                    "contents": [{
                        "uri": uri,
                        "mimeType": "application/json",
                        "text": serde_json::to_string_pretty(&stats).unwrap_or_default()
                    }]
                }),
            ),
            Err(e) => error_response(id, -32603, &e.to_string()),
        }
    } else if let Some(entry_id) = uri.strip_prefix("knowledge://entries/") {
        match store.get(entry_id) {
            Ok(Some(entry)) => {
                let md = format!(
                    "# {}\n\n- **Kind:** {}\n- **Scope:** {}\n- **Tags:** {}\n\n## Context\n{}\n\n## Resolution\n{}\n\n## Verification\n{}\n",
                    entry.title,
                    entry.kind,
                    entry.project_scope.as_deref().unwrap_or("global"),
                    entry.tags.join(", "),
                    entry.context,
                    entry.content,
                    entry.verification
                );
                success_response(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "text/markdown",
                            "text": md
                        }]
                    }),
                )
            }
            Ok(None) => error_response(id, -32602, "Entry not found"),
            Err(e) => error_response(id, -32603, &e.to_string()),
        }
    } else {
        error_response(id, -32602, &format!("Unknown resource URI: {uri}"))
    }
}

fn get_prompt(id: &Value, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments");

    match name {
        "troubleshoot_issue" => {
            let error_msg = args
                .and_then(|a| a.get("error_message"))
                .and_then(Value::as_str)
                .unwrap_or("");
            success_response(
                id,
                json!({
                    "description": "Troubleshoot an issue using saved knowledge",
                    "messages": [
                        {
                            "role": "user",
                            "content": {
                                "type": "text",
                                "text": format!(
                                    "I encountered the following issue:\n\n```\n{error_msg}\n```\n\nPlease check AtlasCodeInteligence using `search_knowledge` to see if we have a known resolution. If found, apply the resolution; if not, diagnose and after verifying the fix, save the verified solution with `save_knowledge`."
                                )
                            }
                        }
                    ]
                }),
            )
        }
        "document_solution" => {
            let title = args
                .and_then(|a| a.get("title"))
                .and_then(Value::as_str)
                .unwrap_or("");
            success_response(
                id,
                json!({
                    "description": "Document a verified resolution into AtlasCodeInteligence",
                    "messages": [
                        {
                            "role": "user",
                            "content": {
                                "type": "text",
                                "text": format!(
                                    "Please document the solution for '{title}'.\n1. Formulate clear title and tags\n2. Include root cause under 'context'\n3. Include step-by-step resolution under 'content'\n4. Include verification command and evidence under 'verification'\n5. Save via `save_knowledge`."
                                )
                            }
                        }
                    ]
                }),
            )
        }
        _ => error_response(id, -32602, &format!("Unknown prompt: {name}")),
    }
}

fn format_search_markdown(search_result: &SearchResult, query: &str, compact: bool) -> String {
    if search_result.results.is_empty() {
        return format!("No knowledge entries found matching query: \"{query}\".");
    }

    let mut md = format!(
        "### Found {} Knowledge {} for \"{}\" (ranking: {})\n\n",
        search_result.count,
        if search_result.count == 1 {
            "entry"
        } else {
            "entries"
        },
        query,
        search_result.ranking
    );

    if compact {
        for (i, hit) in search_result.results.iter().enumerate() {
            md.push_str(&format!(
                "{}. **{}** (`{}`) - ID: `{}`\n   - **Scope:** `{}` | **Tags:** `{}`\n   - **Context:** {}\n   - **Fix:** {}\n\n",
                i + 1,
                hit.entry.title,
                hit.entry.kind,
                hit.entry.id,
                hit.entry.project_scope.as_deref().unwrap_or("global"),
                hit.entry.tags.join(", "),
                hit.entry.context.chars().take(100).collect::<String>(),
                hit.entry.content.chars().take(120).collect::<String>()
            ));
        }
        return md;
    }

    for (i, hit) in search_result.results.iter().enumerate() {
        md.push_str(&format!(
            "---\n#### {}. {} `[{}]`\n- **ID:** `{}`\n- **Scope:** `{}` | **Tags:** {}\n- **Context:**\n  > {}\n- **Resolution:**\n```\n{}\n```\n- **Verification Evidence:**\n  > {}\n",
            i + 1,
            hit.entry.title,
            hit.entry.kind,
            hit.entry.id,
            hit.entry.project_scope.as_deref().unwrap_or("global"),
            if hit.entry.tags.is_empty() {
                "none".to_string()
            } else {
                hit.entry.tags.join(", ")
            },
            hit.entry.context.trim(),
            hit.entry.content.trim(),
            hit.entry.verification.trim()
        ));

        if let Some(telemetry) = &hit.telemetry {
            let mut score_parts = Vec::new();
            if let Some(rank) = telemetry.bm25_rank {
                score_parts.push(format!("BM25 #{rank}"));
            }
            if let Some(sim) = telemetry.vector_similarity {
                score_parts.push(format!("Vector Sim: {sim:.3}"));
            }
            if !score_parts.is_empty() {
                md.push_str(&format!(
                    "- **Match Signals:** {}\n",
                    score_parts.join(" | ")
                ));
            }
        }
        md.push('\n');
    }

    if let Some(tel) = &search_result.telemetry {
        md.push_str(&format!(
            "*(Retrieved in {:.1}ms: lexical {:.1}ms, semantic {:.1}ms)*\n",
            tel.total_ms, tel.lexical_ms, tel.semantic_ms
        ));
    }

    md
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
    use crate::embedder::MockEmbedder;

    #[test]
    fn test_batch_jsonrpc_request() {
        let store = KnowledgeStore::open(std::path::Path::new(":memory:")).unwrap();
        let embedder = MockEmbedder::new(32);

        let batch = json!([
            { "jsonrpc": "2.0", "id": 1, "method": "ping" },
            { "jsonrpc": "2.0", "id": 2, "method": "tools/list" }
        ]);

        let response = handle_request(&batch, &store, &embedder).unwrap();
        let arr = response.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["id"], 1);
        assert_eq!(arr[1]["id"], 2);
    }

    #[test]
    fn test_resources_and_prompts_list() {
        let store = KnowledgeStore::open(std::path::Path::new(":memory:")).unwrap();
        let embedder = MockEmbedder::new(32);

        let res_req = json!({ "jsonrpc": "2.0", "id": 1, "method": "resources/list" });
        let res_resp = handle_request(&res_req, &store, &embedder).unwrap();
        assert!(res_resp["result"]["resources"].as_array().unwrap().len() >= 2);

        let prompt_req = json!({ "jsonrpc": "2.0", "id": 2, "method": "prompts/list" });
        let prompt_resp = handle_request(&prompt_req, &store, &embedder).unwrap();
        assert!(prompt_resp["result"]["prompts"].as_array().unwrap().len() >= 2);
    }

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
        let embedder = MockEmbedder::new(32);
        assert!(handle_request(&request, &store, &embedder).is_none());
    }
}
