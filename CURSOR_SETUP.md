# Using AtlasCodeInteligence with Cursor

This guide provides step-by-step instructions to install, configure, and use **AtlasCodeInteligence** as a Model Context Protocol (MCP) server inside **Cursor**.

---

## 1. Installation

You can install the `atlascode-inteligence-mcp` binary through any of the following methods.

### Option A: From crates.io (Recommended once published)
```sh
cargo install atlascode-inteligence
```

### Option B: Directly from GitHub
```sh
cargo install --git https://github.com/adityaiitg/AtlasCodeInteligence.git
```

### Option C: From Local Source
```sh
cd /path/to/AtlasCodeInteligence
cargo install --path .
```

Verify that the executable is installed and available on your PATH:
```sh
atlascode-inteligence-mcp --version
# Output: atlascode-inteligence-mcp 0.1.0
```

> **Note for macOS/Linux:** Cargo installs binaries to `~/.cargo/bin`. Ensure that `~/.cargo/bin` is in your system `PATH`. If unsure, run:
> ```sh
> which atlascode-inteligence-mcp
> ```

---

## 2. Configuring Cursor

### Method 1: Via Cursor Settings UI (Global)

1. Open **Cursor**.
2. Open Cursor Settings:
   - macOS: `Cmd + ,` (or click the gear icon in the top right).
   - Windows/Linux: `Ctrl + ,`.
3. Navigate to **Features** in the sidebar, then scroll down to **MCP Servers**.
4. Click **+ Add New MCP Server**.
5. Fill in the modal fields:
   - **Name:** `atlascode-inteligence`
   - **Type:** `command` (stdio)
   - **Command:** `atlascode-inteligence-mcp` (or full path, e.g. `/Users/yourusername/.cargo/bin/atlascode-inteligence-mcp`)
6. Click **Add**.
7. You will see a green status dot next to `atlascode-inteligence` showing 3 tools enabled:
   - `search_knowledge`
   - `save_knowledge`
   - `delete_knowledge`

---

### Method 2: Via Project-Level Configuration (`.cursor/mcp.json`)

To automatically enable AtlasCodeInteligence for anyone opening a specific repository in Cursor, create or edit `.cursor/mcp.json` in the root of your workspace:

```json
{
  "mcpServers": {
    "atlascode-inteligence": {
      "command": "atlascode-inteligence-mcp",
      "args": []
    }
  }
}
```

#### Optional Environment Variables in `.cursor/mcp.json`
If you want to isolate databases per project or operate strictly offline, pass environment variables:

```json
{
  "mcpServers": {
    "atlascode-inteligence": {
      "command": "atlascode-inteligence-mcp",
      "args": [],
      "env": {
        "ATLAS_CODE_INTELIGENCE_DB": "/path/to/custom_knowledge.db",
        "ATLAS_CODE_INTELIGENCE_OFFLINE": "0",
        "RUST_LOG": "info"
      }
    }
  }
}
```

---

## 3. Cursor Rules for Automatic Tool Usage

To make Cursor's AI agent (in Chat and Composer) automatically consult and update your knowledge base, create a rule file in your project at `.cursor/rules/atlascode.mdc` (or add to `.cursorrules`):

```markdown
---
description: Closed-loop engineering memory with AtlasCodeInteligence
globs: *
alwaysApply: true
---

# AtlasCodeInteligence Protocol

You have access to the local AtlasCodeInteligence MCP tools. Follow this closed-loop workflow:

1. **SEARCH FIRST**:
   - Before investigating non-trivial compiler errors, database migrations, framework quirks, or complex deployment steps, call `search_knowledge`.
   - Provide concise queries with error codes (e.g. `E0382`, `TS2322`, `SQLSTATE 55P03`) or library names.
   - If a known resolution is found, apply it immediately.

2. **VERIFY BEFORE SAVING**:
   - Never save unverified assumptions or speculative fixes.
   - Only save after you run the test, build the binary, or confirm the command executed with exit code 0.

3. **SAVE & MERGE**:
   - Call `save_knowledge` after verifying a fix.
   - Set `kind` to `"resolved_failure"` for bugs/errors, or `"workflow"` for reusable procedures.
   - Set `context` to the symptom/error trace, `content` to the exact solution, and `verification` to the evidence.
   - AtlasCodeInteligence automatically redacts secrets and merges near-duplicates.
```

---

## 4. Example Interactions in Cursor

### Searching for Knowledge
In Cursor Chat (`Cmd + L`) or Composer (`Cmd + I`):
> *"Search our Atlas knowledge base: how do we resolve PostgreSQL lock timeouts during active record migrations?"*

Cursor will call `search_knowledge` and display the matching entry, root cause, resolution steps, and verification commands.

### Saving a Verified Solution
After fixing an issue in Cursor:
> *"We verified the fix passes all unit tests. Please save this resolution into AtlasCodeInteligence under project 'payments-backend' with tags ['postgres', 'migration']."*

Cursor will call `save_knowledge`, scrubbing any credentials automatically and storing the solution with full audit history.

---

## 5. Troubleshooting & Health Check

### Test the Server from Terminal
Run the built-in CLI stats command to verify database access:
```sh
atlascode-inteligence-mcp stats
```
Output:
```
AtlasCodeInteligence Database Statistics
========================================
Database path:      /Users/.../Library/Application Support/AtlasCodeInteligence/knowledge.db
Total entries:      12
Audit revisions:    4
Database size:      164.00 KB
Semantic vector active: true
```

### Cursor Shows Red Status Dot
If Cursor displays an error or red status dot:
1. Verify PATH: In your terminal run `which atlascode-inteligence-mcp`. In Cursor's MCP configuration, specify the **absolute path** (`/Users/.../.cargo/bin/atlascode-inteligence-mcp`) instead of just `atlascode-inteligence-mcp`.
2. Check logs: Start Cursor from the terminal (`cursor .`) to view stderr diagnostic logs emitted by the MCP server.
