## User (checkpoint)

if we think about flok and how it currently lacks MCP support, does it have too many tools? I will work on first-class MCP support shortly, will that make any of the existing tools obsolete?

## Assistant

<thinking>
The user is asking about flok's current tool system and whether adding MCP (Model Context Protocol) support would make any existing tools obsolete. Let me explore the codebase to understand the current tool architecture.
</thinking>

Great question! Let me explore the current tool landscape in flok to give you a well-informed answer.

**Tool Call: Explore tool registry and definitions**
Status: Completed

Now I have a comprehensive understanding. Here's the full summary:

---

## Flok Tool System — Comprehensive Summary

### Architecture Overview

The tool system lives in `flok/crates/flok-core/src/tool/` and follows a clean **trait-based plugin** pattern. Every tool implements the `Tool` trait, tools are collected in a `ToolRegistry`, and the `SessionEngine` dispatches tool calls from LLM responses through a multi-phase execution pipeline.

---

### 1. The `Tool` Trait

Defined in `mod.rs`, the core trait that every tool implements:

```flok/crates/flok-core/src/tool/mod.rs#L244-L268
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters_schema(&self) -> serde_json::Value;
    fn permission_level(&self) -> PermissionLevel { PermissionLevel::Safe }
    fn describe_invocation(&self, args: &serde_json::Value) -> String { ... }
    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput>;
}
```

Each tool declares:
- **name** — unique identifier (e.g., `"bash"`, `"read"`, `"edit"`)
- **description** — LLM-facing description of what the tool does
- **parameters_schema** — JSON Schema for the tool's input parameters
- **permission_level** — `Safe`, `Write`, or `Dangerous`
- **describe_invocation** — human-readable summary for permission prompts
- **execute** — the actual async implementation

---

### 2. The `ToolRegistry`

```flok/crates/flok-core/src/tool/registry.rs#L9-L14
pub struct ToolRegistry {
    tools: HashMap<&'static str, Arc<dyn Tool>>,
}
```

A simple `HashMap` keyed by tool name. Provides:
- `register(tool)` — add a tool
- `get(name)` — look up a tool by name
- `tool_definitions()` — generate `ToolDefinition` structs (name + description + schema) to send to the LLM in completion requests
- `names()` — list all registered tool names

---

### 3. Tool Registration (Wiring)

All tools are registered in `flok/crates/flok/src/main.rs` during startup. The registration order is:

1. **Core file/shell tools**: `ReadTool`, `WriteTool`, `EditTool`, `FastApplyTool`, `BashTool`, `GrepTool`, `SmartGrepTool`, `GlobTool`
2. **Network/interaction tools**: `WebfetchTool`, `QuestionTool`
3. **State/tracking tools**: `TodoWriteTool`, `SkillTool`, `AgentMemoryTool`
4. **Planning tools**: `PlanTool`, `PlanCreateTool`, `PlanUpdateTool`
5. **Review tool**: `CodeReviewTool`
6. **Team coordination tools**: `TeamCreateTool`, `TeamDeleteTool`, `TeamTaskTool`, `SendMessageTool`
7. **Sub-agent tool**: `TaskTool` (registered last — gets a **snapshot** of tools *before* itself to prevent recursive spawning)
8. **LSP tools** (conditional — only if workspace is supported): `LspDiagnosticsTool`, `LspGotoDefinitionTool`, `LspFindReferencesTool`, `LspSymbolsTool`

---

### 4. Tool Dispatch (Execution Pipeline)

The `SessionEngine::execute_tool_calls()` method in `engine.rs` handles dispatch in a sophisticated multi-phase pipeline:

**Phase 1 — Pre-validation (sequential)**:
For each tool call from the LLM:
1. **Lookup**: Resolve the tool by name from the registry. Unknown tools → error.
2. **Plan mode check**: If in plan mode, block non-`Safe` tools.
3. **Schema validation**: Validate arguments against the tool's JSON schema.
4. **Permission check**: 
   - `Safe` tools → auto-approved
   - `Write`/`Dangerous` tools → evaluated against the 3-layer rule system (defaults → config → session rules), possibly prompting the user via TUI
   - `bash` commands get additional `external_directory` permission checks if they reference paths outside the project

**Phase 2 — Execution (mixed sequential/parallel)**:
- **Safe tools** are batched and executed **concurrently** via `futures::future::join_all`
- **Write/Dangerous tools** are executed **sequentially**, with file state captured before/after for undo support
- Each batch of accumulated safe tools is flushed before the next write tool runs
- Every tool execution is wrapped in `catch_unwind` for panic safety

**Phase 3 — Post-processing**:
- Results are truncated if too long
- File diffs are captured for undo support
- Automatic verification may run after write tools (e.g., linting/testing)

---

### 5. Permission System

Three-tier, last-match-wins rule evaluation:

```flok/crates/flok-core/src/tool/mod.rs#L88-L94
pub struct PermissionManager {
    default_rules: Vec<PermissionRule>,      // Hardcoded defaults
    config_rules: RwLock<Vec<PermissionRule>>,  // From flok.toml
    session_rules: Mutex<Vec<PermissionRule>>,  // User "Always Allow" decisions
    request_tx: Option<mpsc::UnboundedSender<PermissionRequest>>,
}
```

Decision outcomes: `Allow`, `Ask` (prompt user), or `Deny`. User can respond with `Allow` (once), `Always` (adds session rule), or `Deny`.

---

### 6. Supporting Infrastructure

- **`ToolContext`** — passed to every tool: project root, session ID, agent name, cancellation token, LSP manager reference, compression config
- **`ToolOutput`** — return value with `content` (text for LLM), `is_error` flag, and optional metadata
- **`path_security.rs`** — shared path validation for write operations. Blocks: path traversal (`../`), symlink escapes, writes to `.flok/` internals, `flok.toml`, agent memory files, and plan files
- **`compression.rs`** — 4-stage output compression pipeline (filter → group → truncate → deduplicate) applied to large tool outputs (e.g., bash) to fit token budgets

---

### 7. Complete Tool Inventory

| # | Tool Name | Struct | Permission | Purpose |
|---|-----------|--------|------------|---------|
| 1 | `read` | `ReadTool` | Safe | Read file contents with line numbers. Supports offset/limit pagination. Also lists directory contents if given a directory path. |
| 2 | `write` | `WriteTool` | Write | Write/overwrite entire file contents. Creates parent directories. Blocked for `.flok/` internals and `flok.toml`. |
| 3 | `edit` | `EditTool` | Write | Exact string find-and-replace in a file. Requires a unique match (errors on 0 or >1 matches). Surgical single-edit tool. |
| 4 | `fast_apply` | `FastApplyTool` | Write | Smart code edit using snippets with `// ... existing code ...` ellipsis markers. Delegates to the `flok_apply` crate with multiple strategies (ellipsis-merge, full-file). Preferred for multi-line changes. |
| 5 | `bash` | `BashTool` | Dangerous | Execute shell commands in the project directory. Custom parser rejects shell operators (`|`, `&`, `;`, `>`, `<`, `$`, backticks) to prevent injection. Strips dangerous env vars (`LD_PRELOAD`, `DYLD_INSERT_LIBRARIES`, `NODE_OPTIONS`, etc.). Configurable timeout (default 120s). Output compression applied when configured. |
| 6 | `grep` | `GrepTool` | Safe | Regex search across files using system `grep -rn -E`. Supports path scoping and file glob filtering. Truncates results at 50KB. |
| 7 | `smart_grep` | `SmartGrepTool` | Safe | Symbol-aware code search with 4 query modes: `text` (raw regex), `symbol` (definition search), `reference` (usage search), `semantic` (kind-filtered). Uses tree-sitter for Rust and JavaScript AST-aware searching. Classifies matches as definitions, calls, or references. |
| 8 | `glob` | `GlobTool` | Safe | Find files matching glob patterns (e.g., `**/*.rs`). Returns paths sorted, truncated at 500 results. |
| 9 | `webfetch` | `WebfetchTool` | Write | Fetch URL content. SSRF protection blocks private IPs, loopback, metadata endpoints, `.internal` domains. Auto-upgrades HTTP→HTTPS. Strips HTML tags for LLM consumption. Handles redirects (max 5). 100KB response limit. |
| 10 | `question` | `QuestionTool` | Safe | Ask the user a question with selectable options via TUI dialog. Returns the user's chosen answer. Supports custom free-text input. |
| 11 | `todowrite` | `TodoWriteTool` | Safe | Manage a task list (replace entire list). Each item has content, status (`pending`/`in_progress`/`completed`/`cancelled`), and priority (`high`/`medium`/`low`). Shared state displayed in TUI sidebar. |
| 12 | `skill` | `SkillTool` | Safe | Load specialized instruction markdown files. Search order: project `.flok/skills/` → global `~/.config/flok/skills/` → built-in compiled skills (`code-review`, `spec-review`, `self-review-loop`, `handle-pr-feedback`). 20KB limit. |
| 13 | `agent_memory` | `AgentMemoryTool` | Safe | Read/write/append persistent per-agent memory stored in `.flok/memory/<agent>.md`. Memory persists across sessions. |
| 14 | `plan` | `PlanTool` | Safe | Write markdown plans to `.flok/plan.md`. Supports replace or append mode. Used in plan mode for documenting analysis. |
| 15 | `plan_create` | `PlanCreateTool` | Safe | Create typed, structured execution plans persisted as JSON in `.flok/plans/<plan_id>.json`. Plans have titled steps, dependencies, affected files, and agent type assignments. |
| 16 | `plan_update` | `PlanUpdateTool` | Safe | Update plan/step status (`draft`→`approved`→`executing`→`completed`/`failed`/`cancelled`). Supports checkpointing with workspace snapshot hashes. |
| 17 | `code_review` | `CodeReviewTool` | Safe | Run a structured code review on a git diff. Spawns parallel specialist reviewers (correctness, style, architecture, completeness) via a `ReviewEngine`. Returns prioritized findings with a binary APPROVE/REQUEST_CHANGES verdict. Can review against a base branch or a GitHub PR (via `gh pr diff`). |
| 18 | `task` | `TaskTool` | Safe | Spawn a sub-agent (child session) to handle work autonomously. Available agent types: `explore`, `general`. Supports explicit model/reasoning-effort overrides, background mode, and team integration. Sub-agents get a filtered tool set (no `task` tool to prevent recursion). Max 25 iterations per sub-agent. |
| 19 | `team_create` | `TeamCreateTool` | Safe | Create a named agent team for multi-agent coordination. Returns a team ID. |
| 20 | `team_delete` | `TeamDeleteTool` | Safe | Disband a team after work is complete. |
| 21 | `team_task` | `TeamTaskTool` | Safe | Manage tasks on a team's shared task board (CRUD operations: create, update, get, list). Tasks have subject, description, status, and owner. |
| 22 | `send_message` | `SendMessageTool` | Safe | Send a message to another agent in a team. Enables inter-agent communication for coordinated workflows. |
| 23 | `lsp_diagnostics` | `LspDiagnosticsTool` | Safe | Get LSP diagnostics (errors, warnings, hints) for a file or directory. Optional severity filter. *(Conditional — only registered when LSP is available.)* |
| 24 | `lsp_goto_definition` | `LspGotoDefinitionTool` | Safe | Find the definition location of a symbol at a given file position via LSP. *(Conditional.)* |
| 25 | `lsp_find_references` | `LspFindReferencesTool` | Safe | Find all references to a symbol at a given file position via LSP. *(Conditional.)* |
| 26 | `lsp_symbols` | `LspSymbolsTool` | Safe | List document symbols for a file or search workspace symbols via LSP. *(Conditional.)* |

---

### Key Design Patterns

1. **Safe tools run concurrently; write/dangerous tools run sequentially** — maximizes throughput while preserving file consistency and allowing undo capture.

2. **Panic safety** — every `tool.execute()` call is wrapped in `catch_unwind`, so a panicking tool doesn't crash the session.

3. **Path security is centralized** — `resolve_write_path()` in `path_security.rs` is used by `write`, `edit`, and `fast_apply` to enforce a single set of path rules (no traversal, no symlink escape, no internal file writes).

4. **Sub-agent recursion prevention** — the `TaskTool` is registered *last* and receives a snapshot of tools taken *before* itself, so sub-agents cannot spawn further sub-agents.

5. **Output compression** — configurable 4-stage pipeline applied to bash output to keep token usage manageable for long command outputs.

**Tool Call: Check for MCP-related code or specs**
Status: Completed

Here's a complete summary of everything MCP-related in the flok project:

---

## MCP in the Flok Project — Complete Summary

### 1. Spec: `flok/specs/009-mcp-integration/spec.md` ✅ Exists (Detailed)

This is a comprehensive **Draft** spec covering the full MCP integration design. Key highlights:

- **Transports**: Stdio (P1) and StreamableHTTP with SSE fallback (P2)
- **Config format** in `flok.toml`:
```flok/specs/009-mcp-integration/spec.md#L68-L76
  ```toml
  [mcp.filesystem]
  command = "npx"
  args = ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/dir"]
  timeout = 30  # seconds

  [mcp.remote-search]
  url = "https://mcp.example.com/search"
  headers = { Authorization = "Bearer $SEARCH_API_KEY" }
  ```
```
- **Key entities**: `McpServerConfig`, `McpTransport` (Stdio/Http), `McpServerStatus`, `McpManager`, `McpServer`, `McpToolDefinition`
- **Tool namespacing**: `{server_name}_{tool_name}` to prevent collisions
- **Lifecycle**: Non-blocking startup, graceful degradation, reconnect with exponential backoff, 10s total connection timeout
- **Features**: `tools/list`, `ToolListChanged` notification handling, prompts, resources, OAuth 2.0 (PKCE)
- **Alternatives considered**: Evaluating `rmcp` crate vs. building from scratch

### 2. Implementation Status: **Not Started** 🔴

There is **zero MCP implementation code** in the codebase:

- **No `crates/flok-mcp/` directory** exists
- **No MCP references in any `.rs` file** under `crates/`
- **No MCP configuration** in `FlokConfig` struct — the config struct at `flok/crates/flok-core/src/config.rs` has no `mcp` field
- **No `flok.toml`** exists at the project root (only `flok.example.toml` and `flok.all-gpt-5.4-high.toml`, neither of which contains MCP sections)

### 3. Implementation Roadmap (from `flok/1-to-10.md`)

MCP is **Workstream #6** ("MCP Platform"), explicitly marked as **"Not started"**:

```flok/1-to-10.md#L573-L578
## 6. MCP Platform

### Goal

Make external tools first-class through MCP instead of forcing every integration into the binary.
```

The planned implementation order is:
1. Add config model for MCP servers
2. Implement stdio JSON-RPC transport first
3. Add tool discovery on startup or lazy connect
4. Namespace MCP tools in the registry
5. Add reconnect and disabled/failed server state
6. Add prompt/resource support after tools are stable
7. Add HTTP/OAuth transport later

The crate extraction plan places `flok-mcp` as step 4 (last to extract) because it has "the most lifecycle/config coupling."

### 4. Architecture References

- **`flok/specs/001-core-architecture/spec.md`** lists `flok-mcp` as a planned workspace crate
- **`flok/.specify/memory/constitution.md`** states: *"MCP servers are the only external processes"*
- **`flok/AGENTS.md`** mentions mocking MCP servers in tests

### 5. Current Tool System (Integration Surface)

The current `ToolRegistry` at `flok/crates/flok-core/src/tool/registry.rs` is **simple and static**:

```flok/crates/flok-core/src/tool/registry.rs#L12-L16
pub struct ToolRegistry {
    tools: HashMap<&'static str, Arc<dyn Tool>>,
}
```

Key observations for MCP integration:
- Tool names are `&'static str` — MCP tools have dynamic names, so this will need to change to `String`
- Registration is via `&mut self` at startup — MCP needs dynamic registration/deregistration
- The `Tool` trait in `mod.rs` uses `&'static str` for `name()` and `description()` — MCP tools would need an adapter (as sketched in the spec's `McpToolWrapper`)
- No concept of tool namespacing exists yet

### 6. What Doesn't Exist Yet

| Component | Status |
|-----------|--------|
| `crates/flok-mcp/` crate | Does not exist |
| `McpManager` | Not implemented |
| `McpServerConfig` in config | Not in `FlokConfig` |
| Stdio transport | Not implemented |
| HTTP/SSE transport | Not implemented |
| JSON-RPC 2.0 client | Not implemented |
| Dynamic tool registration | Not supported (registry is static `&'static str` keyed) |
| Tool namespacing | Not implemented |
| `[mcp.*]` config parsing | Not implemented |
| MCP-related bus events | Not defined |
| Any MCP dependency (e.g., `rmcp`) | Not in `Cargo.toml` |

### Bottom Line

MCP has a **thorough spec** and a **clear implementation plan**, but there is **absolutely no runtime code** yet. The tool registry will need modifications (at minimum, moving from `&'static str` keys to `String`) to support dynamic MCP tools. The spec recommends starting with stdio transport and evaluating the `rmcp` crate before building from scratch.

## Assistant

<thinking>
Now I have a comprehensive understanding of both the current tool system and the MCP situation. Let me analyze whether any existing tools would become obsolete with MCP support.

Current tools (26 total):
1. **read** - File reading
2. **write** - File writing
3. **edit** - String find-and-replace editing
4. **fast_apply** - Smart code editing with ellipsis markers
5. **bash** - Shell command execution
6. **grep** - Regex file search
7. **smart_grep** - Symbol-aware code search (tree-sitter)


8. **glob** - File path matching
9. **webfetch** - URL fetching with SSRF protection
10. **question** - Ask user questions via TUI
11. **todowrite** - Task list management
12. **skill** - Load instruction markdown files
13. **agent_memory** - Persistent per-agent memory
14. **plan** - Write markdown plans
15. **plan_create** - Create structured execution plans
16. **plan_update** - Update plan status
17. **code_review** - Run structured code review
18. **task** - Spawn sub-agents
19. **team_create** - Create agent teams
20. **team_delete** - Delete teams
21. **team_task** - Manage team tasks
22. **send_message** - Inter-agent messaging
23. **lsp_diagnostics** - LSP diagnostics
24. **lsp_find_references** - LSP references
25. **lsp_goto_definition** - LSP go-to-def
26. **lsp_symbols** - LSP symbols

Now I'm thinking about whether MCP would make any of these redundant. MCP is fundamentally a protocol for connecting to external tool servers that expose tools, prompts, and resources — so it would replace hardcoded integrations rather than core functionality. Let me sort these by what MCP could actually replace versus what needs to stay built-in.

The core file and workspace operations like read, write, edit, fast_apply, bash, grep, and glob are too security-sensitive to move to MCP servers since they need flok's built-in protections against path traversal and injection attacks. The TUI interaction tools like question and todowrite are tightly coupled to the event system and can't be externalized. Agent coordination tools like task, team_create, team_delete, team_task, and send_message are deeply integrated with flok's session engine and bus, so they have to remain built-in. The knowledge and memory tools are also intertwined with flok's internal state management.

The LSP tools are interesting because they wrap an LSP client that's already running for flok's own use—theoretically an MCP server could do this, but they're really just convenience wrappers. Code review is another complex tool that spawns sub-reviewers and relies heavily on flok's session engine.

Looking at this holistically, no existing tools would actually become obsolete if I introduced MCP support. The real question is whether there are redundancies or consolidation opportunities among the current toolset, regardless of MCP.

I'm noticing some potential overlaps: grep, smart_grep, and glob are three search tools with different backends—smart_grep could theoretically subsume grep, but they serve different use cases. Write, edit, and fast_apply are three file modification tools with different semantics and purposes, though there might be room to consolidate write and edit. The planning tools are more concerning—plan writes markdown while plan_create and plan_update manage structured JSON plans, which feels genuinely redundant. Team coordination has four tools that could potentially be consolidated, and I should look at the todo tools as well.

The real issue here is that having too many tools actually hurts LLM performance—the more options available, the harder it is to make the right selection, and tool definitions consume token budget. I need to think through which tools are truly necessary versus which ones create unnecessary cognitive load.
</thinking>

This is a really insightful question. After digging through the entire tool system and the MCP spec, here's my analysis:

## Current Tool Count: 26 Tools

That's quite a lot. Let me break them into categories and assess each one:

### Core Workspace Tools (8) — **Must remain built-in, MCP won't replace these**

| Tool | Rationale |
|------|-----------|
| `read` | Core sandbox operation with security enforcement |
| `write` | Core sandbox operation with path traversal protection |
| `edit` | Core sandbox operation (surgical find-replace) |
| `fast_apply` | Core sandbox operation (smart patching via `flok_apply`) |
| `bash` | Core sandbox operation with injection protection, env-var stripping |
| `grep` | Fast search, relies on system `grep` |
| `smart_grep` | Tree-sitter AST-aware search, tightly coupled to flok internals |
| `glob` | Simple file-path matching |

These are the bread-and-butter. They enforce security invariants (path traversal, symlink escape, injection blocking) that **must** live in-process. MCP can't replace them.

### TUI / Session-Coupled Tools (3) — **Can't be MCP, too tightly coupled**

| Tool | Rationale |
|------|-----------|
| `question` | Communicates directly with the TUI via event bus |
| `todowrite` | Renders in the TUI sidebar, shared state |
| `skill` | Reads built-in compiled skills + project/global config |

### Agent Orchestration Tools (5) — **Can't be MCP, core engine integration**

| Tool | Rationale |
|------|-----------|
| `task` | Spawns child sessions via the `SessionEngine` |
| `team_create` | Manages in-memory team state |
| `team_delete` | Manages in-memory team state |
| `team_task` | Manages in-memory team state |
| `send_message` | Routes messages via internal bus |

### LSP Tools (4) — **Won't be replaced by MCP, but worth questioning**

| Tool | Rationale |
|------|-----------|
| `lsp_diagnostics` | Wraps flok's own LSP client |
| `lsp_goto_definition` | Wraps flok's own LSP client |
| `lsp_find_references` | Wraps flok's own LSP client |
| `lsp_symbols` | Wraps flok's own LSP client |

These *could* theoretically be an MCP server, but flok already runs the LSP client internally, so the MCP round-trip would add latency and complexity for no gain.

### Planning Tools (3) — **Questionable overlap**

| Tool | Rationale |
|------|-----------|
| `plan` | Writes markdown to `.flok/plan.md` |
| `plan_create` | Creates structured JSON plans in `.flok/plans/` |
| `plan_update` | Updates structured JSON plan status |

### Other Tools (3)

| Tool | Rationale |
|------|-----------|
| `webfetch` | URL fetching with SSRF protection |
| `agent_memory` | Persistent per-agent `.flok/memory/` files |
| `code_review` | Spawns parallel sub-reviewers via `ReviewEngine` |

---

## Will MCP Make Any Existing Tools Obsolete?

**Short answer: No.** None of the 26 tools become obsolete with MCP. Here's why:

1. **Core workspace tools** — Security-critical, must stay in-process.
2. **TUI/session tools** — Require direct access to flok's event bus and internal state. MCP has no way to render in your TUI sidebar or prompt the user via your dialog system.
3. **Orchestration tools** — Deeply coupled to the session engine. An MCP server can't spawn a flok sub-agent.
4. **LSP tools** — Already in-process; wrapping them in MCP would add latency for no benefit.

MCP's value is in **extending** the tool surface (external databases, SaaS APIs, specialized search engines, custom dev tools) — not replacing what flok already does well natively.

---

## But Does Flok Have Too Many Tools? (Regardless of MCP)

**Yes, I think there's bloat worth addressing.** Here are the areas I'd scrutinize:

### 1. Three planning tools is likely two too many

`plan`, `plan_create`, and `plan_update` overlap in intent. You have both a markdown plan system AND a structured JSON plan system. Consider:
- Do agents actually use both? Or does one get ignored?
- Could `plan` be dropped in favor of `plan_create`/`plan_update`? Or vice versa?
- `todowrite` also overlaps with planning — it's a task list with statuses, which is conceptually similar to a plan with steps.

### 2. Four team tools could be two

`team_create` + `team_delete` + `team_task` + `send_message` — that's four tools for team coordination. Could consolidate:
- `team` (create/delete/list via an `action` parameter)
- `team_message` (send messages + manage task board)

### 3. `write` vs `edit` vs `fast_apply` — three file mutation tools

This is actually fine conceptually (overwrite vs. surgical replace vs. smart patch), but it's a lot of surface for the LLM to reason about. Many coding agents get by with two: a "create/overwrite" tool and a "patch" tool. You could consider:
- Merging `edit` and `fast_apply` into a single `edit` tool with a `mode` parameter
- Or dropping `edit` entirely if `fast_apply` handles the surgical case well enough

### 4. `grep` vs `smart_grep` — two search tools

`smart_grep` already supports a `text` query mode that does raw regex. If `smart_grep`'s `text` mode is as fast as `grep`, you could drop `grep`. If not, the distinction is justified by performance.

### 5. `code_review` is surprisingly heavy for a built-in tool

It spawns parallel sub-reviewers, runs a `ReviewEngine`, etc. This feels like it should be a **skill** or a **workflow** rather than a tool. When MCP arrives, this is exactly the kind of thing that could be an external MCP server. It's not security-sensitive and doesn't need in-process access.

---

## My Recommendation

Before adding MCP tools to the registry, I'd **consolidate to reduce the base count**:

| Action | Saves |
|--------|-------|
| Merge `plan` into `plan_create`/`plan_update` (or vice versa) | -1 tool |
| Merge `team_create`/`team_delete` into a single `team` tool with actions | -1 tool |
| Merge `team_task` and `send_message` into a single `team_communicate` tool | -1 tool |
| Consider merging `edit` into `fast_apply` | -1 tool |
| Consider moving `code_review` to a skill/workflow | -1 tool |

That would take you from **26 → ~21 tools**, which is a healthier baseline before MCP starts adding dynamic tools on top. The LLM has to emit a JSON schema call for every tool invocation — the fewer tools it has to disambiguate between, the more reliable its tool selection becomes.

When MCP lands, the **namespaced tool names** (e.g., `filesystem_read_file`, `github_search_issues`) will naturally separate the MCP tools from built-ins, which helps. But the total count still matters for the LLM's context window and decision quality.

