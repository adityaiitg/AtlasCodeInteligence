use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

struct McpProcess {
    child: Child,
    reader: BufReader<std::process::ChildStdout>,
}

impl McpProcess {
    fn spawn(db_path: &std::path::Path) -> Self {
        Self::spawn_with_env(db_path, &[])
    }

    fn spawn_with_env(db_path: &std::path::Path, extra_env: &[(&str, &str)]) -> Self {
        let bin_path = env!("CARGO_BIN_EXE_atlascode-inteligence-mcp");
        let mut cmd = Command::new(bin_path);
        cmd.env("ATLAS_CODE_INTELIGENCE_DB", db_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .spawn()
            .expect("Failed to spawn atlascode-inteligence-mcp");
        let stdout = child.stdout.take().expect("Child stdout missing");
        let reader = BufReader::new(stdout);
        Self { child, reader }
    }

    fn send_request(&mut self, request: Value) -> Value {
        let stdin = self.child.stdin.as_mut().expect("Child stdin missing");
        let mut line = serde_json::to_string(&request).expect("Serialize request");
        line.push('\n');
        stdin.write_all(line.as_bytes()).expect("Write to stdin");
        stdin.flush().expect("Flush stdin");

        let mut response_line = String::new();
        self.reader
            .read_line(&mut response_line)
            .expect("Read response line");
        serde_json::from_str(&response_line).expect("Parse response JSON")
    }

    fn close(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn test_mcp_binary_full_lifecycle() {
    let temp_dir = std::env::temp_dir();
    let db_path = temp_dir.join(format!("atlascode-integ-test-{}.db", uuid::Uuid::new_v4()));

    // Phase 1: Initialize, list tools, save, search, replace
    let mut server = McpProcess::spawn(&db_path);

    // 1. Initialize
    let init_resp = server.send_request(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "integration-tester", "version": "1.0" }
        }
    }));
    assert_eq!(init_resp["id"], 1);
    assert_eq!(
        init_resp["result"]["serverInfo"]["name"],
        "atlascode-inteligence"
    );
    assert_eq!(init_resp["result"]["protocolVersion"], "2024-11-05");

    // 2. Tools list
    let list_resp = server.send_request(json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    }));
    let tool_names = list_resp["result"]["tools"]
        .as_array()
        .expect("Tools array")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        tool_names,
        vec!["search_knowledge", "save_knowledge", "delete_knowledge"]
    );

    // 3. Save knowledge
    let save_resp = server.send_request(json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "save_knowledge",
            "arguments": {
                "title": "Resolve PostgreSQL lock timeout during migration",
                "kind": "resolved_failure",
                "context": "Active record migration fails with lock_timeout after 5 seconds",
                "content": "Terminated long running analytical queries holding exclusive table lock and set lock_timeout=10s",
                "verification": "Migration 2026092301 completed in 420ms without lock timeout",
                "tags": ["postgres", "migration", "database"],
                "project_scope": "payments-backend"
            }
        }
    }));
    assert!(!save_resp["result"]
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false));
    let entry_id = save_resp["result"]["structuredContent"]["entry"]["id"]
        .as_str()
        .expect("Entry ID string")
        .to_owned();

    // 4. Keyword search
    let search_resp = server.send_request(json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "search_knowledge",
            "arguments": {
                "query": "analytical queries holding exclusive table lock"
            }
        }
    }));
    assert_eq!(search_resp["result"]["structuredContent"]["count"], 1);
    assert_eq!(
        search_resp["result"]["structuredContent"]["results"][0]["keyword_rank"],
        1
    );

    // 5. Project scope keyword search isolation (unindexed in FTS)
    let scope_search_resp = server.send_request(json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "search_knowledge",
            "arguments": {
                "query": "payments-backend"
            }
        }
    }));
    for res in scope_search_resp["result"]["structuredContent"]["results"]
        .as_array()
        .unwrap()
    {
        assert!(
            res.get("keyword_rank").is_none() || res["keyword_rank"].is_null(),
            "Project scope should not be matched via keyword search"
        );
    }

    // 6. Semantic search
    let semantic_resp = server.send_request(json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "tools/call",
        "params": {
            "name": "search_knowledge",
            "arguments": {
                "query": "database migration stuck on table lock timeout"
            }
        }
    }));
    assert!(
        semantic_resp["result"]["structuredContent"]["count"]
            .as_u64()
            .unwrap()
            >= 1
    );

    // 7. In-place replace
    let replace_resp = server.send_request(json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": {
            "name": "save_knowledge",
            "arguments": {
                "entry_id": entry_id,
                "title": "Resolve PostgreSQL lock timeout during migration",
                "kind": "workflow",
                "context": "Standard procedure for schema updates on busy production tables",
                "content": "Gracefully drain incoming traffic, run migration with zero downtime flags, monitor pg_stat_activity",
                "verification": "Executed across staging and prod clusters with zero dropped connections",
                "tags": ["postgres", "migration", "database", "zero-downtime"],
                "project_scope": "payments-backend"
            }
        }
    }));
    assert_eq!(
        replace_resp["result"]["structuredContent"]["entry"]["id"],
        entry_id
    );

    // Close phase 1 server
    server.close();

    // Phase 2: Restart server against existing DB, verify persistence, update, and deletion
    let mut server2 = McpProcess::spawn(&db_path);

    // 8. Search for obsolete content (should not match keywords)
    let obsolete_search = server2.send_request(json!({
        "jsonrpc": "2.0",
        "id": 8,
        "method": "tools/call",
        "params": {
            "name": "search_knowledge",
            "arguments": {
                "query": "analytical queries"
            }
        }
    }));
    let obsolete_results = obsolete_search["result"]["structuredContent"]["results"]
        .as_array()
        .unwrap();
    for res in obsolete_results {
        assert!(
            res.get("keyword_rank").is_none() || res["keyword_rank"].is_null(),
            "Obsolete content should not be matched via keyword search"
        );
    }

    // 9. Search for updated content
    let updated_search = server2.send_request(json!({
        "jsonrpc": "2.0",
        "id": 9,
        "method": "tools/call",
        "params": {
            "name": "search_knowledge",
            "arguments": {
                "query": "drain incoming traffic"
            }
        }
    }));
    assert_eq!(updated_search["result"]["structuredContent"]["count"], 1);
    assert_eq!(
        updated_search["result"]["structuredContent"]["results"][0]["keyword_rank"],
        1
    );

    // 10. Delete entry
    let delete_resp = server2.send_request(json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "tools/call",
        "params": {
            "name": "delete_knowledge",
            "arguments": {
                "entry_id": entry_id
            }
        }
    }));
    assert_eq!(delete_resp["result"]["structuredContent"]["deleted"], true);

    // 11. Search after deletion
    let post_delete_search = server2.send_request(json!({
        "jsonrpc": "2.0",
        "id": 11,
        "method": "tools/call",
        "params": {
            "name": "search_knowledge",
            "arguments": {
                "query": "drain incoming traffic"
            }
        }
    }));
    assert_eq!(
        post_delete_search["result"]["structuredContent"]["count"],
        0
    );

    server2.close();
    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn test_mcp_automatic_deduplication() {
    let temp_dir = std::env::temp_dir();
    let db_path = temp_dir.join(format!("atlascode-dedup-test-{}.db", uuid::Uuid::new_v4()));

    let mut server = McpProcess::spawn(&db_path);

    // 1. Initial save without entry_id
    let save1 = server.send_request(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "save_knowledge",
            "arguments": {
                "title": "Configure Kafka consumer group offset reset",
                "kind": "workflow",
                "context": "When consumer group falls behind retention window",
                "content": "kafka-consumer-groups.sh --reset-offsets --to-latest",
                "verification": "Lag drops to zero and consumer resumes processing",
                "tags": ["kafka", "streaming"],
                "project_scope": "event-pipeline"
            }
        }
    }));
    let entry1 = &save1["result"]["structuredContent"]["entry"];
    let id1 = entry1["id"].as_str().unwrap().to_owned();
    let created_at1 = entry1["created_at"].as_str().unwrap().to_owned();
    assert_eq!(save1["result"]["structuredContent"]["deduplicated"], false);

    // 2. Second save with identical title and project_scope (still omitting entry_id)
    let save2 = server.send_request(json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "save_knowledge",
            "arguments": {
                "title": "Configure Kafka consumer group offset reset",
                "kind": "workflow",
                "context": "Updated procedure: always verify active members before resetting",
                "content": "kafka-consumer-groups.sh --describe --group pipeline; kafka-consumer-groups.sh --reset-offsets --to-latest --execute",
                "verification": "Lag verified at 0 and offsets committed successfully",
                "tags": ["kafka", "streaming", "ops"],
                "project_scope": "event-pipeline"
            }
        }
    }));
    let entry2 = &save2["result"]["structuredContent"]["entry"];
    let id2 = entry2["id"].as_str().unwrap().to_owned();
    let created_at2 = entry2["created_at"].as_str().unwrap().to_owned();

    // Must update in place with deduplicated: true
    assert_eq!(save2["result"]["structuredContent"]["deduplicated"], true);
    assert_eq!(id1, id2, "Deduplication must preserve the entry ID");
    assert_eq!(
        created_at1, created_at2,
        "Deduplication must preserve created_at"
    );

    // 3. Search verifies only 1 entry exists
    let search = server.send_request(json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "search_knowledge",
            "arguments": { "query": "kafka-consumer-groups" }
        }
    }));
    assert_eq!(search["result"]["structuredContent"]["count"], 1);

    server.close();
    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn test_mcp_offline_lexical_only_mode() {
    let temp_dir = std::env::temp_dir();
    let db_path = temp_dir.join(format!(
        "atlascode-offline-test-{}.db",
        uuid::Uuid::new_v4()
    ));

    // Spawn server with ATLAS_CODE_INTELIGENCE_OFFLINE=1
    let mut server =
        McpProcess::spawn_with_env(&db_path, &[("ATLAS_CODE_INTELIGENCE_OFFLINE", "1")]);

    // Save knowledge in offline mode
    let save_resp = server.send_request(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "save_knowledge",
            "arguments": {
                "title": "Fix Nginx 502 Bad Gateway with upstream sock permission",
                "kind": "resolved_failure",
                "context": "Nginx returns 502 after systemd service restart",
                "content": "chmod 666 /var/run/app.sock and update umask in systemd unit",
                "verification": "curl -I localhost returns 200 OK",
                "tags": ["nginx", "linux", "systemd"]
            }
        }
    }));
    assert!(!save_resp["result"]
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false));

    // Keyword search in offline mode
    let search_resp = server.send_request(json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "search_knowledge",
            "arguments": { "query": "app.sock permission" }
        }
    }));
    assert_eq!(search_resp["result"]["structuredContent"]["count"], 1);
    assert_eq!(
        search_resp["result"]["structuredContent"]["ranking"],
        "lexical_only"
    );
    let hit = &search_resp["result"]["structuredContent"]["results"][0];
    assert_eq!(hit["keyword_rank"], 1);
    assert!(hit.get("semantic_rank").is_none() || hit["semantic_rank"].is_null());

    server.close();
    let _ = std::fs::remove_file(&db_path);
}
