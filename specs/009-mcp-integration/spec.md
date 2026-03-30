# Feature Specification: MCP Integration

**Feature Branch**: `009-mcp-integration`
**Created**: 2026-03-28
**Status**: Draft

## User Scenarios & Testing

### User Story 1 - Developer Connects to an MCP Server (Priority: P1)
**Why this priority**: MCP extends flok's capabilities without modifying the binary.
**Acceptance Scenarios**:
1. **Given** an MCP server is configured in `flok.toml`, **When** flok starts, **Then** it connects to the server and discovers its tools within 5s.
2. **Given** an MCP server exposes a `web_search` tool, **When** the LLM calls it, **Then** the request is routed to the MCP server and the result returned to the LLM.
3. **Given** an MCP server disconnects, **When** the LLM tries to call one of its tools, **Then** it receives a clear error and can recover.

### User Story 2 - Developer Uses a Stdio MCP Server (Priority: P1)
**Why this priority**: Stdio is the most common MCP transport for local tools.
**Acceptance Scenarios**:
1. **Given** config specifies `command = "npx" args = ["-y", "@modelcontextprotocol/server-filesystem"]`, **When** flok starts, **Then** it spawns the process and communicates via stdin/stdout JSON-RPC.
2. **Given** the MCP server process crashes, **When** detected, **Then** flok marks the server as failed and excludes its tools until restart.

### User Story 3 - Developer Uses a Remote MCP Server (Priority: P2)
**Why this priority**: Remote MCP servers enable shared team capabilities.
**Acceptance Scenarios**:
1. **Given** config specifies `url = "https://mcp.example.com"`, **When** flok connects, **Then** it uses StreamableHTTP transport with SSE fallback.
2. **Given** the remote server requires OAuth, **When** flok detects the auth requirement, **Then** it opens a browser for the OAuth flow and stores the token.

### Edge Cases
- MCP server has slow tool calls (>30s): timeout per-call (configurable, default 30s)
- MCP server returns invalid JSON: log error, return tool error to LLM
- Tool name collision between MCP servers: prefix with server name (e.g., `filesystem_read_file`)
- MCP server declared in config but not installed: log warning, skip, don't block startup
- MCP server returns tools dynamically (list changes): handle `ToolListChanged` notification
- OAuth token expires: auto-refresh before next request

## Requirements

### Functional Requirements

- **FR-001**: Flok MUST implement the MCP client specification (JSON-RPC 2.0).
- **FR-002**: Flok MUST support two MCP transports:
  - **Stdio**: Spawn child process, communicate via stdin/stdout
  - **StreamableHTTP**: HTTP POST for requests, SSE for responses (with SSE-only fallback)
- **FR-003**: MCP servers MUST be configured in `flok.toml`:
  ```toml
  [mcp.filesystem]
  command = "npx"
  args = ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/dir"]
  timeout = 30  # seconds

  [mcp.remote-search]
  url = "https://mcp.example.com/search"
  headers = { Authorization = "Bearer $SEARCH_API_KEY" }
  ```
- **FR-004**: Flok MUST discover tools from MCP servers via the `tools/list` method.
- **FR-005**: MCP tool names MUST be namespaced: `{server_name}_{tool_name}` to prevent collisions.
- **FR-006**: Flok MUST handle `ToolListChanged` notifications by re-discovering tools.
- **FR-007**: Flok MUST support MCP prompts (`prompts/list`, `prompts/get`) for template-based interactions.
- **FR-008**: Flok MUST support MCP resources (`resources/list`, `resources/read`) for context injection.
- **FR-009**: MCP connections MUST be managed with lifecycle awareness:
  - Connect on startup (parallel with other initialization)
  - Reconnect on connection loss (with exponential backoff)
  - Graceful shutdown on flok exit
- **FR-010**: Flok MUST support OAuth 2.0 for remote MCP servers (authorization code flow with PKCE).
- **FR-011**: Each MCP server MUST have an independent timeout (configurable, default 30s per tool call).
- **FR-012**: MCP servers MUST be disableable without removing config:
  ```toml
  [mcp.filesystem]
  command = "npx"
  args = [...]
  disabled = true
  ```

### Key Entities

```rust
pub struct McpServerConfig {
    pub name: String,
    pub transport: McpTransport,
    pub timeout: Duration,           // Default 30s
    pub disabled: bool,
    pub env: HashMap<String, String>,
}

pub enum McpTransport {
    Stdio {
        command: String,
        args: Vec<String>,
        cwd: Option<PathBuf>,
    },
    Http {
        url: String,
        headers: HashMap<String, String>,
        auth: Option<McpAuthConfig>,
    },
}

pub enum McpAuthConfig {
    Bearer(String),
    OAuth {
        client_id: Option<String>,
        // Dynamic client registration if not provided
    },
}

pub enum McpServerStatus {
    Connecting,
    Connected,
    Disabled,
    Failed(String),
    NeedsAuth,
}

pub struct McpManager {
    servers: DashMap<String, McpServer>,
}

pub struct McpServer {
    pub config: McpServerConfig,
    pub status: ArcSwap<McpServerStatus>,
    pub tools: ArcSwap<Vec<McpToolDefinition>>,
    transport: Box<dyn McpTransportHandle>,
}

pub struct McpToolDefinition {
    pub name: String,              // Original name from server
    pub qualified_name: String,    // "{server}_{name}" for registry
    pub description: String,
    pub input_schema: serde_json::Value,
}
```

## Design

### Overview

The MCP integration provides flok with extensible tool capabilities via the Model Context Protocol. It manages connections to external MCP servers, discovers their tools, and routes tool calls from the LLM to the appropriate server. The design prioritizes: (1) non-blocking startup (MCP connections don't delay TUI launch), (2) graceful degradation (failed servers don't crash flok), and (3) transparent tool routing (the LLM doesn't need to know it's calling an MCP tool).

### Detailed Design

#### McpManager Lifecycle

```rust
impl McpManager {
    pub async fn start(configs: &[McpServerConfig]) -> Self {
        let manager = Self { servers: DashMap::new() };

        // Connect to all servers concurrently
        let futures: Vec<_> = configs.iter()
            .filter(|c| !c.disabled)
            .map(|config| {
                let manager = &manager;
                async move {
                    match Self::connect_server(config).await {
                        Ok(server) => {
                            manager.servers.insert(config.name.clone(), server);
                        }
                        Err(e) => {
                            tracing::warn!(
                                server = %config.name,
                                error = %e,
                                "Failed to connect MCP server"
                            );
                            // Insert with Failed status (don't block others)
                            manager.servers.insert(config.name.clone(), McpServer {
                                status: ArcSwap::new(Arc::new(McpServerStatus::Failed(e.to_string()))),
                                ..
                            });
                        }
                    }
                }
            })
            .collect();

        // Timeout: don't wait more than 10s total for all MCP servers
        let _ = tokio::time::timeout(
            Duration::from_secs(10),
            futures::future::join_all(futures),
        ).await;

        manager
    }

    pub fn all_tools(&self) -> Vec<McpToolDefinition> {
        self.servers.iter()
            .filter(|s| matches!(&*s.status.load(), McpServerStatus::Connected))
            .flat_map(|s| s.tools.load().iter().cloned())
            .collect()
    }

    pub async fn call_tool(
        &self,
        qualified_name: &str,
        arguments: serde_json::Value,
    ) -> Result<String> {
        // Parse server name from qualified_name
        let (server_name, tool_name) = parse_qualified_name(qualified_name)?;
        let server = self.servers.get(server_name)
            .ok_or_else(|| anyhow!("MCP server '{}' not found", server_name))?;

        // Execute with per-server timeout
        tokio::time::timeout(
            server.config.timeout,
            server.transport.call_tool(tool_name, arguments),
        ).await?
    }
}
```

#### Stdio Transport

```rust
pub struct StdioTransport {
    stdin: tokio::process::ChildStdin,
    pending: DashMap<u64, oneshot::Sender<JsonRpcResponse>>,
    next_id: AtomicU64,
}

impl StdioTransport {
    pub async fn spawn(config: &McpServerConfig) -> Result<Self> {
        let child = tokio::process::Command::new(&config.transport.command())
            .args(config.transport.args())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(&config.env)
            .kill_on_drop(true)
            .spawn()?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        // Spawn reader task: reads JSON-RPC responses and routes to pending map
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            // Read Content-Length delimited JSON-RPC messages
            // Route responses to pending oneshot senders
            // Handle notifications (ToolListChanged, etc.)
        });

        Ok(Self { stdin, pending: DashMap::new(), next_id: AtomicU64::new(1) })
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.insert(id, tx);

        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        // Write to stdin with Content-Length header
        let body = serde_json::to_string(&msg)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        self.stdin.write_all(header.as_bytes()).await?;
        self.stdin.write_all(body.as_bytes()).await?;

        // Wait for response
        let response = rx.await?;
        response.into_result()
    }
}
```

#### HTTP Transport

```rust
pub struct HttpTransport {
    client: reqwest::Client,
    base_url: String,
    headers: HeaderMap,
    session_id: Option<String>,  // For stateful sessions
}

impl HttpTransport {
    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let msg = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });

        let mut req = self.client.post(&self.base_url)
            .headers(self.headers.clone())
            .json(&msg);

        if let Some(sid) = &self.session_id {
            req = req.header("mcp-session-id", sid);
        }

        let response = req.send().await?;

        // Check content-type for SSE vs JSON
        let content_type = response.headers().get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if content_type.contains("text/event-stream") {
            // Parse SSE stream, collect JSON-RPC response
            self.parse_sse_response(response).await
        } else {
            // Direct JSON response
            let result: JsonRpcResponse = response.json().await?;
            result.into_result()
        }
    }
}
```

#### Tool Registration in ToolRegistry

MCP tools are registered dynamically in the tool registry:

```rust
impl ToolRegistry {
    pub fn register_mcp_tools(&self, mcp: &McpManager) {
        for tool_def in mcp.all_tools() {
            let mcp = Arc::clone(mcp);
            let qualified_name = tool_def.qualified_name.clone();

            self.mcp_tools.insert(
                tool_def.qualified_name.clone(),
                Arc::new(McpToolWrapper {
                    definition: tool_def,
                    mcp,
                    qualified_name,
                }),
            );
        }
    }
}

pub struct McpToolWrapper {
    definition: McpToolDefinition,
    mcp: Arc<McpManager>,
    qualified_name: String,
}

#[async_trait]
impl Tool for McpToolWrapper {
    fn name(&self) -> &str { &self.qualified_name }
    fn description(&self) -> &str { &self.definition.description }
    fn parameters_schema(&self) -> Value { self.definition.input_schema.clone() }

    async fn execute(&self, args: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let result = self.mcp.call_tool(&self.qualified_name, args).await?;
        Ok(ToolOutput { content: result, ..Default::default() })
    }
}
```

### Alternatives Considered

1. **Use the `rmcp` crate (like Spacebot)**: Evaluate. If `rmcp` is well-maintained and supports both transports, use it instead of implementing from scratch. If it has too many dependencies or lacks StreamableHTTP, implement our own.
2. **Skip MCP, only support built-in tools**: Rejected. MCP is the industry standard for tool extensibility. Without it, users can't integrate custom tools (databases, APIs, etc.) without forking flok.
3. **MCP server auto-discovery**: Deferred. For now, servers must be explicitly configured. Auto-discovery via well-known files or mDNS can be added later.
4. **MCP server sandboxing**: Deferred. Stdio servers run as child processes with the user's permissions. Sandboxing can be added later (same as for `bash` tool).

## Success Criteria

- **SC-001**: MCP server connection (stdio) established in < 2s
- **SC-002**: Tool call round-trip to MCP server < 50ms overhead (excluding server processing time)
- **SC-003**: MCP initialization doesn't block TUI startup (async, with 10s total timeout)
- **SC-004**: Graceful degradation: failed MCP servers don't affect built-in tools
- **SC-005**: Tool list refresh on `ToolListChanged` notification < 100ms

## Assumptions

- MCP servers follow the specification faithfully (JSON-RPC 2.0)
- Stdio MCP servers use Content-Length framing (not newline-delimited)
- OAuth for remote MCP servers follows standard flows (authorization code + PKCE)
- Most users will use 0-3 MCP servers (not hundreds)

## Open Questions

- Should we support MCP server process management (auto-restart on crash)?
- Should we support MCP sampling (server-initiated LLM requests)?
- Should we bundle any MCP servers (e.g., filesystem server) with flok?
- Should MCP tool permissions use the same system as built-in tools?
