# Feature Specification: Native LSP Integration

**Feature Branch**: `011-lsp-integration`
**Created**: 2026-03-28
**Status**: Draft

## User Scenarios & Testing

### User Story 1 - Agent Queries Type Information Before Editing (Priority: P0)
**Why this priority**: An agent that knows `x` is `Vec<UserModel>` writes better code than one guessing from context. This is the core value proposition.
**Acceptance Scenarios**:
1. **Given** an LSP server is running for the project's language, **When** the agent reads a file and encounters a variable, **Then** it can query the LSP for the variable's type and documentation via a `lsp_hover` tool.
2. **Given** the agent needs to understand a function's callers, **When** it uses `lsp_references`, **Then** it receives a list of all call sites with file paths and line numbers.
3. **Given** the agent needs to navigate to a definition, **When** it uses `lsp_goto_definition`, **Then** it receives the file path and position of the definition.

### User Story 2 - Agent Gets Compiler Diagnostics After Edits (Priority: P0)
**Why this priority**: LSP diagnostics give the agent a "compiler in the loop" -- it can self-correct without burning tokens re-reading files.
**Acceptance Scenarios**:
1. **Given** the agent has edited a file, **When** the LSP processes the change, **Then** the agent receives diagnostics (errors, warnings) for the edited file within 2s.
2. **Given** the agent receives a type error diagnostic, **When** it reads the error, **Then** it can fix the error and re-check without a full rebuild cycle.
3. **Given** multiple files are affected by a change, **When** the LSP publishes diagnostics, **Then** diagnostics for all affected files are collected and presented.

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
  | `lsp_diagnostics` | (publish diagnostics) | Get errors/warnings for a file | allow |
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

- **FR-005**: LSP diagnostics MUST be collected after each file edit and available to the agent without an explicit tool call. The agent's system prompt should include a note about available diagnostics.

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

The LSP integration makes flok a semantically-aware coding agent. Instead of treating code as text (grep + regex), the agent can query type information, navigate symbol relationships, and get compiler feedback in real-time. The LSP client communicates with language servers via stdio using the standard Language Server Protocol (JSON-RPC over stdin/stdout).

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

#### Diagnostics Collection

Diagnostics are pushed by the LSP server via `textDocument/publishDiagnostics` notifications. The client collects them in a `DashMap` and makes them available to agents:

```rust
// In the reader loop:
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

After an agent edits a file, the `edit` and `write` tools automatically wait briefly (up to 2s) for fresh diagnostics, then append them to the tool result:

```rust
// In edit tool, after applying the edit:
if let Some(lsp) = state.lsp.server_for_file(&file_path) {
    lsp.did_change(&file_path, &new_content, version).await?;

    // Wait for diagnostics (with timeout)
    let diagnostics = tokio::time::timeout(
        Duration::from_secs(2),
        lsp.wait_for_diagnostics(&file_path),
    ).await;

    if let Ok(Some(diags)) = diagnostics {
        let error_count = diags.iter().filter(|d| d.severity == Error).count();
        if error_count > 0 {
            output.push_str(&format!(
                "\n\n⚠ LSP reports {} error(s) after edit:\n{}",
                error_count,
                format_diagnostics(&diags)
            ));
        }
    }
}
```

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

The `edit` and `write` tools are enhanced to automatically synchronize with LSP and report diagnostics. This creates a "compiler in the loop" that reduces wasted tokens:

```
Agent edits file → edit tool applies change
                 → LSP notified (didChange)
                 → Wait up to 2s for diagnostics
                 → If errors: append to tool output
                 → Agent reads errors, self-corrects
                 → Repeat (without re-reading the full file)
```

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

## Success Criteria

- **SC-001**: LSP hover response returns in < 500ms (excluding server startup)
- **SC-002**: Diagnostics published within 2s of file edit notification
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
- Should LSP diagnostics be automatically injected into the agent's context, or only when explicitly requested?
- Should we support LSP code actions as agent-callable tools (auto-fix suggestions)?
- How should we handle LSP servers that require project-specific configuration (e.g., `rust-analyzer` settings in `rust-analyzer.json`)?
- Should we add a `lsp_status` TUI panel showing connected language servers and their health?
