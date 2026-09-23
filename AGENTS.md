# AtlasCodeInteligence Agent Operational Guide

This document defines how autonomous AI agents (Claude, Gemini, Antigravity, Cursor, Codex) should interact with the **AtlasCodeInteligence** Model Context Protocol (MCP) server.

---

## 1. Core Operating Philosophy: Closed-Loop Engineering

AtlasCodeInteligence is a local-first persistent memory engine for engineering solutions and reusable workflows. Rather than resolving compiler errors, framework quirks, or complex deployment steps from scratch repeatedly, agents should participate in a continuous closed loop:

```
    ┌───────────────────────────┐
    │  New Task / Error Arrives │
    └─────────────┬─────────────┘
                  │
                  ▼
    ┌───────────────────────────┐
    │ 1. SEARCH KNOWLEDGE BASE  │  ◄── `search_knowledge`
    └─────────────┬─────────────┘
                  │
        ┌─────────┴─────────┐
        │                   │
    [Found]            [Not Found]
        │                   │
        ▼                   ▼
┌──────────────┐    ┌─────────────────┐
│ Apply Known  │    │ Diagnose, Fix,  │
│  Resolution  │    │   and VERIFY    │
└───────┬──────┘    └───────┬─────────┘
        │                   │
        │                   ▼
        │           ┌─────────────────┐
        │           │ 2. SAVE & MERGE │  ◄── `save_knowledge`
        │           └───────┬─────────┘
        │                   │
        ▼                   ▼
    ┌───────────────────────────┐
    │ Complete Task with Proof  │
    └───────────────────────────┘
```

---

## 2. When and How to Search (`search_knowledge`)

### When to Call:
- **Immediately upon encountering an error**: Before analyzing compiler errors, failed tests, DB lock timeouts, or network failures.
- **Before running risky operations**: Helm deployments, database migrations, package upgrades, TLS certificate updates.
- **When starting work in a specific repository**: Query with `project_scope: "<repo-name>"` to retrieve known quirks and workflows.

### Query Best Practices:
1. **Include specific signatures**: Include exact compiler codes (`E0382`, `TS2322`, `SQLSTATE 55P03`) or function names.
2. **Strip volatile ephemeral values**: Omit timestamp strings, local temp directories, and ephemeral port numbers from the query string (the server automatically normalizes them, but cleaner queries produce higher BM25 confidence).
3. **Use `compact: true` when scanning broadly**: If you anticipate many matches or need to keep your context window lean, pass `"compact": true` to receive high-density summaries.

```json
{
  "name": "search_knowledge",
  "arguments": {
    "query": "PostgreSQL lock timeout during active record migration",
    "project_scope": "payments-backend",
    "limit": 3
  }
}
```

---

## 3. When and How to Save (`save_knowledge`)

### The Golden Rule of Saving:
> **Never save an unverified assumption.** Only save after you have run the command, executed the test suite, or confirmed the service is healthy.

### Required Fields Quality Checklist:
- **`title`**: Clear, imperative statement of the action or resolution.
  - *Good:* `Resolve PostgreSQL lock timeout during migration`
  - *Bad:* `Fixed bug`
- **`kind`**:
  - `"workflow"`: For reusable standard procedures (deploying, release tagging, generating protos).
  - `"resolved_failure"`: For bug fixes, environment quirks, and compiler errors.
- **`context`**: The symptoms, error stack, operating system, and reproduction trigger.
- **`content`**: Exact step-by-step resolution, code modifications, or terminal commands.
- **`verification`**: The exact command executed and evidence showing it succeeded (test pass count, HTTP 200, log line).
- **`tags`**: 2 to 5 relevant lowercase tags (e.g. `["postgres", "migration", "database"]`).
- **`project_scope`**: Repository name or subsystem (e.g. `"payments-backend"`).

### Automated Deduplication & Security:
- **Near-Duplicate Detection**: If a similar entry already exists in the same scope, AtlasCodeInteligence automatically merges your tags and appends your verification evidence rather than cluttering the database with duplicate rows.
- **Automated Secret Redaction**: Private keys, AWS tokens, GitHub/GitLab PATs, OpenAI/Anthropic keys, and DB passwords in connection URIs are automatically redacted before embedding and disk storage.

---

## 4. MCP Tools & Resources Quick Reference

| Tool / Resource | Purpose |
|---|---|
| `search_knowledge` | Two-phase hybrid BM25 + Vector search with Reciprocal Rank Fusion ($k=20$). |
| `save_knowledge` | Persist workflows or resolved failures with secret scrubbing and audit history. |
| `delete_knowledge` | Remove outdated or superseded knowledge entries by ID. |
| `knowledge://recent` | Resource providing the 20 most recently updated entries in Markdown. |
| `knowledge://stats` | Operational telemetry: total entries, revision count, project breakdown. |
