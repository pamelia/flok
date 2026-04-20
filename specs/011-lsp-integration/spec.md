# Feature Specification: Native LSP Integration

**Feature Branch**: `011-lsp-integration`
**Created**: 2026-03-28
**Status**: Accepted (2026-04-19 — feature shipped; spec retroactively locked to match built reality.)

## User Scenarios & Testing

### User Story 1 - Agent Queries Type Information Before Editing (Priority: P0)
**Why this priority**: An agent that knows `x` is `Vec<UserModel>` writes better code than one guessing from context. This is the core value proposition.
**Acceptance Scenarios**:
1. **Given** an LSP server is running for the project's language, **When** the agent reads a file and encounters a variable, **Then** it can query the LSP for the variable's type and documentation via a `lsp_hover` tool.
2. **Given** the agent needs to understand a function's callers, **When** it uses `lsp_references`, **Then** it receives a list of all call sites with file paths and line numbers.
3. **Given** the agent needs to navigate to a definition, **When** it uses `lsp_goto_definition`, **Then** it receives the file path and position of the definition.

### User Story 2 - Agent Gets Compiler Diagnostics at Checkpoints (Priority: P0)
**Why this priority**: LSP diagnostics give the agent a "compiler in the loop" -- it can self-correct without burning tokens re-reading files. But diagnostics must be delivered at the right time, not after every intermediate edit.
**Acceptance Scenarios**:
1. **Given** the agent has completed a logical unit of edits (e.g., finished modifying a function, added an import, renamed across files), **When** the agent explicitly calls `lsp_diagnostics`, **Then** it receives current diagnostics (errors, warnings) for the specified files.
2. **Given** the agent receives a type error diagnostic, **When** it reads the error, **Then** it can fix the error and re-check without a full rebuild cycle.
3. **Given** the agent is midway through a multi-edit sequence (e.g., edit 3 of 8), **When** it edits a file, **Then** diagnostics are NOT automatically appended to the edit tool output -- the agent checks when ready.
4. **Given** the agent completes all edits for a task, **When** it calls `lsp_diagnostics`, **Then** diagnostics for all affected files are collected and presented together.

### User Story 3 - LSP Server Auto-Detection and Management (Priority: P1)
**Why this priority**: Users shouldn't have to configure LSP servers manually for common languages.
**Acceptance Scenarios**:
1. **Given** a Rust project (detected by `Cargo.toml`), **When** flok starts, **Then** it automatically launches `rust-analyzer` if available on PATH.
2. **Given** a TypeScript project, **When** flok starts, **Then** it launches `typescript-language-server` if available.
3. **Given** no LSP server is available for the project language, **When** the agent tries to use LSP tools, **Then** it receives a clear message that LSP is unavailable and falls back to non-LSP tools.

### User Story 4 - Agent Uses Symbol Search for Codebase Navigation (Priority: P1)
**Why this priority**: Symbol-level search is more precise than text grep for code navigation.
**Acceptance Scenarios**:
1. **Given** the agent needs to find all implementations of a trait/interface, **When** it uses `lsp_implementations`, **Then** it receives all implementing types with locations.
2. **Given** the agent needs to find a symbol by name, **When** it uses `lsp_workspace_symbols` with a query, **Then** it receives matching symbols across the entire workspace.
3. **Given** the agent asks "where is authentication handled", **When** it combines `lsp_workspace_symbols("auth")` with grep results, **Then** it gets symbol-level results (not just line matches).

### Edge Cases
- LSP server crashes: restart automatically (max 3 times), then disable LSP and warn
- LSP server is slow (> 5s for a response): timeout, return partial results if available
- File edited outside the worktree (e.g., git checkout): send `didOpen`/`didChange` notifications to keep LSP in sync
- Project uses multiple languages: launch multiple LSP servers (one per language)
- LSP server not installed: graceful degradation -- all LSP tools return "LSP unavailable" without crashing
- Very large workspace (> 10K files): LSP initialization may be slow, don't block agent work
- Worktree agents: each worktree gets its own LSP server instance (separate root paths)

## Requirements

### Functional Requirements

- **FR-001**: Flok MUST provide LSP-powered tools available to agents:

  | Tool | LSP Method | Description | Permission Default |
  |------|-----------|-------------|-------------------|
  | `lsp_hover` | `textDocument/hover` | Type info, docs at position | allow |
  | `lsp_goto_definition` | `textDocument/definition` | Jump to definition | allow |
  | `lsp_references` | `textDocument/references` | Find all references | allow |
  | `lsp_implementations` | `textDocument/implementation` | Find implementations | allow |
  | `lsp_workspace_symbols` | `workspace/symbol` | Search symbols by name | allow |
  | `lsp_diagnostics` | (reads cached `publishDiagnostics`) | Get current errors/warnings for file(s) -- agent calls this explicitly at checkpoints | allow |
  | `lsp_rename_preview` | `textDocument/prepareRename` + `textDocument/rename` | Preview a rename refactor | allow |
  | `lsp_code_actions` | `textDocument/codeAction` | Get available code actions | allow |

- **FR-002**: Flok MUST auto-detect project languages and launch appropriate LSP servers:

  | Language | Detection | LSP Server | Fallback |
  |----------|-----------|------------|----------|
  | Rust | `Cargo.toml` | `rust-analyzer` | None |
  | TypeScript/JavaScript | `tsconfig.json`, `package.json` | `typescript-language-server` | None |
  | Python | `pyproject.toml`, `setup.py`, `requirements.txt` | `pylsp` or `pyright` | None |
  | Go | `go.mod` | `gopls` | None |
  | C/C++ | `CMakeLists.txt`, `compile_commands.json` | `clangd` | None |
  | Java | `pom.xml`, `build.gradle` | `jdtls` | None |

- **FR-003**: LSP servers MUST be configurable and overridable in `flok.toml`:
  ```toml
  [lsp.rust]
  command = "rust-analyzer"
  args = []
  settings = { "rust-analyzer.checkOnSave" = true }

  [lsp.python]
  command = "pyright-langserver"
  args = ["--stdio"]
  ```

- **FR-004**: Flok MUST keep LSP servers synchronized with file changes:
  - Send `textDocument/didOpen` when the agent first reads a file
  - Send `textDocument/didChange` after the agent edits a file
  - Send `textDocument/didClose` when appropriate (session end)
  - Handle `workspace/didChangeWatchedFiles` for external changes

- **FR-005**: LSP diagnostics MUST be collected in the background (via `textDocument/publishDiagnostics` notifications) but MUST NOT be automatically injected into edit/write tool output. Diagnostics are available on-demand via the `lsp_diagnostics` tool. The agent's system prompt should instruct it to check diagnostics after completing a logical unit of work, not after every individual edit.

- **FR-005a**: The `edit` and `write` tools MUST still send `textDocument/didChange` notifications to keep the LSP server's view of the file in sync. This is file synchronization, not diagnostic delivery -- these are separate concerns.

- **FR-005b**: Flok MUST NOT wait for or block on LSP diagnostics during edit/write tool execution. The edit tool returns immediately after applying the file change and notifying the LSP.

- **FR-006**: LSP tool calls MUST timeout after 5s (configurable) and return a timeout error.

- **FR-007**: LSP server lifecycle MUST be managed automatically:
  - Start on first LSP tool call (lazy initialization) or on project open
  - Restart on crash (max 3 retries, then disable)
  - Shutdown on flok exit
  - Separate instance per worktree

- **FR-008**: LSP initialization MUST NOT block flok startup or the TUI. LSP tools return "LSP initializing..." if called before the server is ready.

- **FR-009**: Flok MUST support multiple concurrent LSP servers for polyglot projects.

- **FR-010**: LSP tools MUST be disabled (hidden from tool list) when no LSP server is available for the project, to avoid confusing the LLM.

### Key Entities

```rust
pub struct LspManager {
    servers: DashMap<String, LspServer>,  // Keyed by language ID
    configs: HashMap<String, LspServerConfig>,
}

pub struct LspServer {
    pub language: String,
    pub status: ArcSwap<LspServerStatus>,
    pub capabilities: ArcSwap<Option<ServerCapabilities>>,
    client: Arc<LspClient>,
}

pub enum LspServerStatus {
    Starting,
    Ready,
    Failed(String),
    Disabled,
}

pub struct LspServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub root_path: PathBuf,
    pub settings: serde_json::Value,
    pub timeout: Duration,         // Default 5s per request
}

pub struct LspClient {
    stdin: tokio::process::ChildStdin,
    pending: DashMap<u64, oneshot::Sender<serde_json::Value>>,
    next_id: AtomicU64,
    diagnostics: DashMap<PathBuf, Vec<Diagnostic>>,
}

pub struct Diagnostic {
    pub path: PathBuf,
    pub range: Range,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub source: Option<String>,
    pub code: Option<String>,
}

pub enum DiagnosticSeverity { Error, Warning, Info, Hint }
```

## Design

### Overview

The LSP integration makes flok a semantically-aware coding agent. Instead of treating code as text (grep + regex), the agent can query type information, navigate symbol relationships, and get compiler diagnostics on demand. The LSP client communicates with language servers via stdio using the standard Language Server Protocol (JSON-RPC over stdin/stdout).

**Key design principle**: LSP serves two distinct roles that must not be conflated:
1. **Code intelligence** (hover, go-to-definition, references, symbols) -- available any time via explicit tool calls.
2. **Diagnostic feedback** (errors, warnings) -- collected in the background but delivered only when the agent explicitly requests it at a logical checkpoint. Diagnostics are never automatically appended to edit/write tool output, because intermediate states during multi-edit sequences are expected to be broken and would produce misleading feedback.

### Detailed Design

#### LSP Client Implementation

The LSP client uses the same JSON-RPC transport pattern as the MCP stdio transport (spec-009), but with LSP-specific message handling:

```rust
impl LspClient {
    pub async fn initialize(
        config: &LspServerConfig,
    ) -> Result<Self> {
        let child = tokio::process::Command::new(&config.command)
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(&config.root_path)
            .kill_on_drop(true)
            .spawn()?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        let client = Self {
            stdin,
            pending: DashMap::new(),
            next_id: AtomicU64::new(1),
            diagnostics: DashMap::new(),
        };

        // Spawn reader task for responses and notifications
        tokio::spawn(client.clone().reader_loop(stdout));

        // Send initialize request
        let init_result = client.request("initialize", json!({
            "processId": std::process::id(),
            "rootPath": config.root_path,
            "rootUri": format!("file://{}", config.root_path.display()),
            "capabilities": {
                "textDocument": {
                    "hover": { "contentFormat": ["plaintext"] },
                    "definition": { "linkSupport": false },
                    "references": {},
                    "implementation": {},
                    "codeAction": {},
                    "rename": { "prepareSupport": true },
                    "publishDiagnostics": { "relatedInformation": true },
                },
                "workspace": {
                    "symbol": {},
                    "didChangeWatchedFiles": { "dynamicRegistration": false },
                }
            },
            "initializationOptions": config.settings,
        })).await?;

        // Send initialized notification
        client.notify("initialized", json!({})).await?;

        Ok(client)
    }
}
```

#### File Synchronization

The LSP client tracks which files are "open" (known to the LSP server) and synchronizes changes:

```rust
impl LspClient {
    /// Called by the `read` tool when a file is first accessed
    pub async fn did_open(&self, path: &Path, content: &str) -> Result<()> {
        let language_id = detect_language(path);
        self.notify("textDocument/didOpen", json!({
            "textDocument": {
                "uri": path_to_uri(path),
                "languageId": language_id,
                "version": 1,
                "text": content,
            }
        })).await
    }

    /// Called by the `edit` or `write` tool after modifying a file
    pub async fn did_change(&self, path: &Path, content: &str, version: i32) -> Result<()> {
        self.notify("textDocument/didChange", json!({
            "textDocument": {
                "uri": path_to_uri(path),
                "version": version,
            },
            "contentChanges": [{ "text": content }],
        })).await
    }
}
```

#### Diagnostics Collection (Background, Not Per-Edit)

Diagnostics are pushed by the LSP server via `textDocument/publishDiagnostics` notifications. The client collects them silently in a `DashMap`. **Diagnostics are never automatically injected into edit/write tool output.**

```rust
// In the reader loop -- silently cache diagnostics as they arrive:
fn handle_notification(&self, method: &str, params: Value) {
    match method {
        "textDocument/publishDiagnostics" => {
            let uri = params["uri"].as_str().unwrap();
            let path = uri_to_path(uri);
            let diagnostics: Vec<Diagnostic> = parse_diagnostics(&params["diagnostics"]);
            self.diagnostics.insert(path, diagnostics);
        }
        _ => {}
    }
}
```

The agent retrieves diagnostics explicitly via the `lsp_diagnostics` tool when it is ready to check:

```rust
pub struct LspDiagnosticsTool;

impl Tool for LspDiagnosticsTool {
    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let paths: Vec<PathBuf> = match args.get("paths") {
            Some(p) => serde_json::from_value(p.clone())?,
            None => vec![], // Empty = return diagnostics for all tracked files
        };

        let lsp = &ctx.state.lsp;
        let mut all_diagnostics = Vec::new();

        // If specific paths requested, optionally wait briefly for fresh diagnostics
        // (the LSP may still be processing recent didChange notifications)
        if !paths.is_empty() {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        let files = if paths.is_empty() {
            lsp.all_diagnostics()
        } else {
            lsp.diagnostics_for(&paths)
        };

        for (path, diags) in files {
            all_diagnostics.extend(diags.iter().map(|d| format_diagnostic(&path, d)));
        }

        if all_diagnostics.is_empty() {
            Ok(ToolOutput::text("No diagnostics. Code looks clean."))
        } else {
            Ok(ToolOutput::text(all_diagnostics.join("\n")))
        }
    }
}
```

**Design rationale -- why not per-edit diagnostics:**

During a multi-edit sequence (e.g., renaming a type and updating 8 call sites), intermediate states are *expected* to be broken. Feeding the agent diagnostics after edit 1 of 8 is actively harmful:
- The diagnostics are noise -- the agent already knows the code is incomplete.
- The agent may waste tokens "fixing" errors that would resolve after the remaining edits.
- Each diagnostic wait adds latency (up to 2s per edit = 16s wasted on an 8-edit sequence).

Instead, the agent checks diagnostics at **logical checkpoints** -- after completing a coherent unit of work. The `edit`/`write` tools still notify the LSP via `didChange` (to keep its view in sync), but do not wait for or return diagnostics.

#### Tool Implementations

Each LSP tool is a thin wrapper around the LSP client:

```rust
pub struct LspHoverTool;

impl Tool for LspHoverTool {
    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let path: PathBuf = serde_json::from_value(args["path"].clone())?;
        let line: u32 = serde_json::from_value(args["line"].clone())?;
        let character: u32 = serde_json::from_value(args["character"].clone())?;

        let lsp = ctx.state.lsp.server_for_file(&path)
            .ok_or_else(|| anyhow!("No LSP server available for {}", path.display()))?;

        let result = lsp.request("textDocument/hover", json!({
            "textDocument": { "uri": path_to_uri(&path) },
            "position": { "line": line, "character": character },
        })).await?;

        let content = extract_hover_content(&result);
        Ok(ToolOutput { content, ..Default::default() })
    }
}
```

#### Integration with Edit Tools

The `edit` and `write` tools synchronize file state with the LSP server but **do not block on or return diagnostics**. This separates file synchronization (keep LSP in sync) from diagnostic feedback (agent checks when ready):

```
Agent edits file → edit tool applies change
                 → LSP notified (didChange) -- fire-and-forget
                 → Tool returns immediately with edit result only

Agent completes logical unit of work (e.g., finishes a function refactor)
                 → Agent calls lsp_diagnostics
                 → Reads errors, self-corrects if needed
```

```rust
// In edit tool, after applying the edit:
if let Some(lsp) = state.lsp.server_for_file(&file_path) {
    // Fire-and-forget: keep LSP in sync, but don't wait for diagnostics.
    // The LSP will process this in the background and update its
    // cached diagnostics, which the agent can query later.
    lsp.did_change(&file_path, &new_content, version).await?;
}
// Tool output contains ONLY the edit result -- no diagnostics appended.
```

This design means the agent operates in two distinct modes:
1. **Editing mode**: rapid sequential edits with no diagnostic overhead.
2. **Verification mode**: explicit `lsp_diagnostics` call to check the result.

The system prompt instructs the agent to call `lsp_diagnostics` after completing a coherent set of changes, not after every individual edit.

#### Per-Worktree LSP Instances

When worktree isolation is enabled (spec-010), each worktree gets its own LSP server instance:

```rust
impl LspManager {
    pub async fn server_for_root(&self, root: &Path) -> Option<&LspServer> {
        // LSP servers are keyed by (language, root_path)
        // Each worktree has a different root_path, so gets a separate server
    }
}
```

### Alternatives Considered

1. **Use `tower-lsp` as the client library**: Considered. `tower-lsp` is primarily a server framework, not a client library. We use raw JSON-RPC over stdio for simplicity and control.
2. **Use `lsp-types` crate for type definitions**: Adopted. `lsp-types` provides the LSP type definitions (Position, Range, Diagnostic, etc.) without imposing a transport layer.
3. **Query LSP on every file read**: Rejected. Too expensive. LSP tools are available on-demand, and diagnostics are pushed after edits.
4. **Bundle LSP servers with flok**: Rejected. LSP servers are language-specific and large. Users install them via their package manager. Flok auto-detects what's available.
5. **Use tree-sitter for type inference instead of LSP**: Rejected for type info (tree-sitter doesn't do type inference), but tree-sitter is used separately for fast apply (spec-014). LSP and tree-sitter serve complementary roles.
6. **Inject diagnostics automatically after every edit** ("compiler in the loop"): Rejected. During multi-edit sequences (e.g., renaming a type across 8 files), intermediate states are expected to be broken. Per-edit diagnostics are noise that wastes tokens, adds latency (up to 2s per edit), and can derail the agent into "fixing" transient errors. The checkpoint-based approach (agent calls `lsp_diagnostics` when ready) gives the same self-correction benefit without the pathological intermediate-state problem.

## Success Criteria

- **SC-001**: LSP hover response returns in < 500ms (excluding server startup)
- **SC-002**: `lsp_diagnostics` tool returns current diagnostics within 1s (reads from cache; LSP background processing is async)
- **SC-003**: LSP initialization does not block TUI launch
- **SC-004**: LSP restart on crash completes within 5s
- **SC-005**: Agent self-correction rate improves by 30%+ when LSP diagnostics are available (measured by reduction in retry turns for type errors)
- **SC-006**: Zero LSP-related crashes in flok (graceful degradation on all LSP failures)

## Assumptions

- Users have LSP servers installed for their primary project language
- LSP servers follow the LSP specification faithfully (JSON-RPC 2.0 over stdio)
- `rust-analyzer`, `typescript-language-server`, `pylsp`/`pyright`, `gopls`, `clangd` are the most common LSP servers
- Diagnostics after edit are "good enough" -- we don't need to run a full build to catch most errors
- LSP startup time (especially `rust-analyzer`) can be 5-30s for large projects -- don't block on it

## Open Questions

- Should we expose `textDocument/completion` for agent-assisted code completion?
- ~~Should LSP diagnostics be automatically injected into the agent's context, or only when explicitly requested?~~ **Resolved: explicitly requested.** Automatic per-edit injection produces noise during multi-edit sequences and can derail the agent. See FR-005 and the "Design rationale" in the Diagnostics Collection section.
- Should we support LSP code actions as agent-callable tools (auto-fix suggestions)?
- How should we handle LSP servers that require project-specific configuration (e.g., `rust-analyzer` settings in `rust-analyzer.json`)?
- Should we add a `lsp_status` TUI panel showing connected language servers and their health?
