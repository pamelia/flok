//! MCP client support.
//!
//! Runtime support includes:
//! - Codex-style config parsing in `config.rs`
//! - stdio transport
//! - Streamable HTTP transport
//! - initialize + initialized notification
//! - tools/list discovery
//! - tools/call execution
//! - namespaced dynamic tool registration

use std::collections::HashMap;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Client, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, Mutex};

use crate::config::McpServerConfig;
use crate::tool::{PermissionLevel, Tool, ToolContext, ToolOutput, ToolRegistry};

const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
const NON_MATCHING_EVENT_ERROR: &str = "non-matching MCP event";

type DynReader = Box<dyn AsyncRead + Send + Unpin>;
type DynWriter = Box<dyn AsyncWrite + Send + Unpin>;

/// Register all configured MCP tools that this runtime currently supports.
///
/// Failed servers are logged and skipped so startup continues.
pub async fn register_configured_tools(
    registry: &mut ToolRegistry,
    configs: &HashMap<String, McpServerConfig, impl BuildHasher>,
) {
    let mut server_names: Vec<_> = configs.keys().cloned().collect();
    server_names.sort();

    for server_name in server_names {
        let Some(config) = configs.get(&server_name) else {
            continue;
        };

        if config.disabled() {
            tracing::info!(server = %server_name, "skipping disabled MCP server");
            continue;
        }

        match register_single_server(registry, &server_name, config).await {
            Ok(count) => {
                tracing::info!(server = %server_name, tool_count = count, "registered MCP tools");
            }
            Err(err) => {
                tracing::warn!(server = %server_name, error = %err, "failed to register MCP server");
            }
        }
    }
}

async fn register_single_server(
    registry: &mut ToolRegistry,
    server_name: &str,
    config: &McpServerConfig,
) -> anyhow::Result<usize> {
    let client = Arc::new(McpClient::spawn(server_name, config)?);
    client.initialize().await?;
    let tools = client.list_tools().await?;

    let dynamic_tools = tools
        .into_iter()
        .map(|tool| {
            let qualified_name = format!("{server_name}_{}", tool.name);
            let registry_name = qualified_name.clone();
            let description = tool.description.unwrap_or_else(|| "MCP tool".to_string());
            let wrapper: Arc<dyn Tool> = Arc::new(McpTool {
                qualified_name,
                server_name: server_name.to_string(),
                original_name: tool.name,
                input_schema: tool.input_schema,
                client: Arc::clone(&client),
            });
            (registry_name, description, wrapper)
        })
        .collect::<Vec<_>>();

    let count = dynamic_tools.len();
    registry.replace_dynamic_namespace(server_name, dynamic_tools);
    Ok(count)
}

enum McpTransport {
    Stdio(StdioTransport),
    Http(Box<HttpTransport>),
}

struct McpTool {
    qualified_name: String,
    server_name: String,
    original_name: String,
    input_schema: Value,
    client: Arc<McpClient>,
}

#[async_trait::async_trait]
impl Tool for McpTool {
    fn name(&self) -> &'static str {
        "mcp"
    }

    fn description(&self) -> &'static str {
        "MCP tool"
    }

    fn parameters_schema(&self) -> Value {
        self.input_schema.clone()
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Dangerous
    }

    fn describe_invocation(&self, _args: &Value) -> String {
        format!(
            "Call MCP tool '{}' on server '{}' via '{}'",
            self.original_name, self.server_name, self.qualified_name
        )
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        let result = self.client.call_tool(&self.original_name, args).await?;
        Ok(ToolOutput {
            content: render_tool_result(&result.content, result.structured_content.as_ref()),
            is_error: result.is_error,
            metadata: result.structured_content.unwrap_or(Value::Null),
        })
    }
}

fn render_tool_result(content: &[McpContentBlock], structured_content: Option<&Value>) -> String {
    let mut parts = Vec::new();
    for block in content {
        if let ("text", Some(text)) = (block.kind.as_str(), block.text.as_deref()) {
            parts.push(text.to_string());
        } else {
            let block_json = serde_json::to_string(block)
                .unwrap_or_else(|_| "{\"type\":\"unknown\"}".to_string());
            parts.push(block_json);
        }
    }

    if parts.is_empty() {
        if let Some(structured_content) = structured_content {
            return serde_json::to_string_pretty(structured_content)
                .unwrap_or_else(|_| structured_content.to_string());
        }
        return String::new();
    }

    parts.join("\n")
}

struct McpClient {
    server_name: String,
    transport: McpTransport,
    next_id: AtomicU64,
}

impl McpClient {
    fn spawn(server_name: &str, config: &McpServerConfig) -> anyhow::Result<Self> {
        match (config.is_stdio(), config.is_remote()) {
            (true, false) => Self::spawn_stdio(server_name, config),
            (false, true) => Self::spawn_http(server_name, config),
            (false, false) => anyhow::bail!("MCP server must define either `command` or `url`"),
            (true, true) => anyhow::bail!("MCP server cannot define both `command` and `url`"),
        }
    }

    fn spawn_stdio(server_name: &str, config: &McpServerConfig) -> anyhow::Result<Self> {
        Ok(Self {
            server_name: server_name.to_string(),
            transport: McpTransport::Stdio(StdioTransport::spawn(server_name, config)?),
            next_id: AtomicU64::new(1),
        })
    }

    fn spawn_http(server_name: &str, config: &McpServerConfig) -> anyhow::Result<Self> {
        Ok(Self {
            server_name: server_name.to_string(),
            transport: McpTransport::Http(Box::new(HttpTransport::new(server_name, config)?)),
            next_id: AtomicU64::new(1),
        })
    }

    async fn initialize(&self) -> anyhow::Result<()> {
        let response = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {
                        "name": "flok",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                }),
            )
            .await?;

        let initialize: InitializeResult =
            serde_json::from_value(response).context("failed to parse MCP initialize response")?;
        tracing::debug!(
            server = %self.server_name,
            protocol_version = %initialize.protocol_version,
            "MCP initialized"
        );

        if let McpTransport::Http(transport) = &self.transport {
            transport.set_protocol_version(initialize.protocol_version.clone()).await;
        }

        self.notify("notifications/initialized", Value::Null).await?;
        Ok(())
    }

    async fn list_tools(&self) -> anyhow::Result<Vec<McpDiscoveredTool>> {
        let mut tools = Vec::new();
        let mut cursor = None::<String>;

        loop {
            let params = cursor.as_ref().map_or(Value::Null, |cursor| json!({ "cursor": cursor }));
            let response = self.request("tools/list", params).await?;
            let page: ListToolsResult =
                serde_json::from_value(response).context("failed to parse MCP tools/list")?;
            tools.extend(page.tools);

            match page.next_cursor {
                Some(next) if !next.is_empty() => cursor = Some(next),
                _ => break,
            }
        }

        Ok(tools)
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> anyhow::Result<CallToolResult> {
        let response =
            self.request("tools/call", json!({ "name": name, "arguments": arguments })).await?;
        serde_json::from_value(response).context("failed to parse MCP tools/call result")
    }

    async fn notify(&self, method: &str, params: Value) -> anyhow::Result<()> {
        match &self.transport {
            McpTransport::Stdio(transport) => transport.notify(method, params).await,
            McpTransport::Http(transport) => transport.notify(method, params).await,
        }
    }

    async fn request(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        match &self.transport {
            McpTransport::Stdio(transport) => transport.request(id, method, params).await,
            McpTransport::Http(transport) => transport.request(id, method, params).await,
        }
    }

    #[cfg(test)]
    fn from_test_io(server_name: &str, writer: DynWriter, reader: DynReader) -> Self {
        Self {
            server_name: server_name.to_string(),
            transport: McpTransport::Stdio(StdioTransport::new(server_name, writer, reader)),
            next_id: AtomicU64::new(1),
        }
    }
}

struct StdioTransport {
    server_name: String,
    writer: Mutex<DynWriter>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<anyhow::Result<Value>>>>>,
}

impl StdioTransport {
    fn spawn(server_name: &str, config: &McpServerConfig) -> anyhow::Result<Self> {
        let command = config.command.as_deref().context("missing MCP stdio command")?;

        let mut cmd = tokio::process::Command::new(command);
        if let Some(args) = &config.args {
            cmd.args(args);
        }
        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }
        if let Some(env) = &config.env {
            cmd.envs(env);
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut child =
            cmd.spawn().with_context(|| format!("failed to spawn MCP server '{server_name}'"))?;

        let stdin = child.stdin.take().context("missing MCP stdin pipe")?;
        let stdout = child.stdout.take().context("missing MCP stdout pipe")?;
        let stderr = child.stderr.take().context("missing MCP stderr pipe")?;

        let stderr_server = server_name.to_string();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                tracing::debug!(server = %stderr_server, stderr = %line, "MCP stderr");
            }
        });

        let wait_server = server_name.to_string();
        tokio::spawn(async move {
            match child.wait().await {
                Ok(status) => tracing::info!(server = %wait_server, %status, "MCP server exited"),
                Err(error) => {
                    tracing::warn!(server = %wait_server, error = %error, "MCP server wait failed");
                }
            }
        });

        Ok(Self::new(server_name, Box::new(stdin), Box::new(stdout)))
    }

    fn new(server_name: &str, writer: DynWriter, reader: DynReader) -> Self {
        let transport = Self {
            server_name: server_name.to_string(),
            writer: Mutex::new(writer),
            pending: Arc::new(Mutex::new(HashMap::new())),
        };
        transport.spawn_reader_task(reader);
        transport
    }

    fn spawn_reader_task(&self, reader: DynReader) {
        let pending = self.pending.clone();
        let server_name = self.server_name.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                match serde_json::from_str::<JsonRpcEnvelope>(&line) {
                    Ok(message) => {
                        if let Some(id) = message.id.and_then(|value| value.as_u64()) {
                            let sender = pending.lock().await.remove(&id);
                            if let Some(sender) = sender {
                                let result = match (message.result, message.error) {
                                    (Some(result), None) => Ok(result),
                                    (None, Some(error)) => {
                                        Err(anyhow!("MCP error {}: {}", error.code, error.message))
                                    }
                                    _ => Err(anyhow!("invalid MCP response envelope")),
                                };
                                let _ = sender.send(result);
                            }
                        }
                    }
                    Err(error) => {
                        tracing::warn!(
                            server = %server_name,
                            error = %error,
                            payload = %line,
                            "failed to parse MCP response"
                        );
                    }
                }
            }

            let mut pending = pending.lock().await;
            for (_, sender) in pending.drain() {
                let _ = sender.send(Err(anyhow!("MCP connection closed")));
            }
            tracing::info!(server = %server_name, "MCP reader loop ended");
        });
    }

    async fn notify(&self, method: &str, params: Value) -> anyhow::Result<()> {
        let message = if params.is_null() {
            json!({
                "jsonrpc": "2.0",
                "method": method,
            })
        } else {
            json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
            })
        };

        self.write_message(&message).await
    }

    async fn request(&self, id: u64, method: &str, params: Value) -> anyhow::Result<Value> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let message = if params.is_null() {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
            })
        } else {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            })
        };

        if let Err(error) = self.write_message(&message).await {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }

        rx.await.with_context(|| format!("MCP request channel closed for method '{method}'"))?
    }

    async fn write_message(&self, message: &Value) -> anyhow::Result<()> {
        let payload = serde_json::to_string(message).context("failed to serialize MCP message")?;
        let mut writer = self.writer.lock().await;
        writer
            .write_all(payload.as_bytes())
            .await
            .with_context(|| format!("failed to write MCP request to '{}'", self.server_name))?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
        Ok(())
    }
}

struct HttpTransport {
    server_name: String,
    endpoint: String,
    backend: HttpTransportBackend,
    session_id: Mutex<Option<String>>,
    protocol_version: Mutex<String>,
    default_headers: HeaderMap,
}

impl HttpTransport {
    fn new(server_name: &str, config: &McpServerConfig) -> anyhow::Result<Self> {
        let endpoint = config.url.clone().context("missing MCP remote url")?;
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds()))
            .build()
            .context("failed to build MCP HTTP client")?;
        Ok(Self {
            server_name: server_name.to_string(),
            endpoint,
            backend: HttpTransportBackend::Reqwest(client),
            session_id: Mutex::new(None),
            protocol_version: Mutex::new(MCP_PROTOCOL_VERSION.to_string()),
            default_headers: build_remote_headers(config)?,
        })
    }

    #[cfg(test)]
    fn new_test(
        server_name: &str,
        config: &McpServerConfig,
        responses: Vec<HttpResponseData>,
    ) -> anyhow::Result<(Self, Arc<Mutex<Vec<TestHttpRequest>>>)> {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let backend = TestHttpBackend {
            requests: Arc::clone(&requests),
            responses: Mutex::new(std::collections::VecDeque::from(responses)),
        };
        Ok((
            Self {
                server_name: server_name.to_string(),
                endpoint: config.url.clone().context("missing MCP remote url")?,
                backend: HttpTransportBackend::Test(backend),
                session_id: Mutex::new(None),
                protocol_version: Mutex::new(MCP_PROTOCOL_VERSION.to_string()),
                default_headers: build_remote_headers(config)?,
            },
            requests,
        ))
    }

    async fn set_protocol_version(&self, protocol_version: String) {
        *self.protocol_version.lock().await = protocol_version;
    }

    async fn notify(&self, method: &str, params: Value) -> anyhow::Result<()> {
        let message = if params.is_null() {
            json!({
                "jsonrpc": "2.0",
                "method": method,
            })
        } else {
            json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
            })
        };

        let response = self.post_message(&message).await?;
        self.capture_session_id(&response.headers).await?;
        let status = response.status;

        if status == StatusCode::ACCEPTED {
            return Ok(());
        }
        if status.is_success() {
            if !response.body.trim().is_empty() {
                tracing::debug!(
                    server = %self.server_name,
                    method = %method,
                    body = %response.body,
                    "MCP notification returned a non-empty response body"
                );
            }
            return Ok(());
        }

        Err(http_status_error(method, status, &response.body))
    }

    async fn request(&self, id: u64, method: &str, params: Value) -> anyhow::Result<Value> {
        let message = if params.is_null() {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
            })
        } else {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            })
        };

        let response = self.post_message(&message).await?;
        self.capture_session_id(&response.headers).await?;
        parse_http_response(&self.server_name, method, id, response)
    }

    async fn post_message(&self, message: &Value) -> anyhow::Result<HttpResponseData> {
        let protocol_version = self.protocol_version.lock().await.clone();
        let session_id = self.session_id.lock().await.clone();

        let mut headers = self.default_headers.clone();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json, text/event-stream"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            HeaderName::from_static("mcp-protocol-version"),
            HeaderValue::from_str(&protocol_version)
                .context("invalid MCP protocol version header")?,
        );
        if let Some(session_id) = session_id {
            headers.insert(
                HeaderName::from_static("mcp-session-id"),
                HeaderValue::from_str(&session_id).context("invalid MCP session id header")?,
            );
        }

        match &self.backend {
            HttpTransportBackend::Reqwest(client) => {
                let response = client
                    .post(&self.endpoint)
                    .headers(headers)
                    .json(message)
                    .send()
                    .await
                    .with_context(|| {
                        format!("failed to send remote MCP request to '{}'", self.server_name)
                    })?;
                let status = response.status();
                let response_headers = response.headers().clone();
                let body = response.text().await.with_context(|| {
                    format!("failed to read remote MCP response from '{}'", self.server_name)
                })?;
                Ok(HttpResponseData { status, headers: response_headers, body })
            }
            #[cfg(test)]
            HttpTransportBackend::Test(backend) => {
                backend
                    .requests
                    .lock()
                    .await
                    .push(TestHttpRequest { headers, body: message.clone() });
                backend
                    .responses
                    .lock()
                    .await
                    .pop_front()
                    .context("missing queued MCP HTTP test response")
            }
        }
    }

    async fn capture_session_id(&self, headers: &HeaderMap) -> anyhow::Result<()> {
        if let Some(session_id) = headers.get("Mcp-Session-Id") {
            let session_id = session_id.to_str().context("invalid MCP session header")?.to_string();
            *self.session_id.lock().await = Some(session_id);
        }
        Ok(())
    }
}

enum HttpTransportBackend {
    Reqwest(Client),
    #[cfg(test)]
    Test(TestHttpBackend),
}

struct HttpResponseData {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

#[cfg(test)]
struct TestHttpBackend {
    requests: Arc<Mutex<Vec<TestHttpRequest>>>,
    responses: Mutex<std::collections::VecDeque<HttpResponseData>>,
}

#[cfg(test)]
#[derive(Clone)]
struct TestHttpRequest {
    headers: HeaderMap,
    body: Value,
}

fn build_remote_headers(config: &McpServerConfig) -> anyhow::Result<HeaderMap> {
    let mut headers = HeaderMap::new();

    if let Some(configured_headers) = &config.headers {
        for (name, value) in configured_headers {
            let header_name = HeaderName::from_bytes(name.as_bytes())
                .with_context(|| format!("invalid MCP header name '{name}'"))?;
            let header_value = HeaderValue::from_str(value)
                .with_context(|| format!("invalid MCP header value for '{name}'"))?;
            headers.insert(header_name, header_value);
        }
    }

    if let Some(env_var) = &config.bearer_token_env_var {
        let bearer_token = SecretString::from(
            std::env::var(env_var)
                .with_context(|| format!("MCP bearer token env var '{env_var}' is not set"))?,
        );
        if bearer_token.expose_secret().trim().is_empty() {
            anyhow::bail!("MCP bearer token env var '{env_var}' is empty");
        }

        let authorization = format!("Bearer {}", bearer_token.expose_secret());
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&authorization)
                .context("invalid Authorization header built from MCP bearer token")?,
        );
    }

    Ok(headers)
}

fn parse_http_response(
    server_name: &str,
    method: &str,
    id: u64,
    response: HttpResponseData,
) -> anyhow::Result<Value> {
    let status = response.status;
    let content_type = response
        .headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = response.body;

    if !status.is_success() {
        return Err(http_status_error(method, status, &body));
    }

    if content_type.starts_with("text/event-stream") {
        return parse_sse_response(server_name, method, id, &body);
    }
    if content_type.is_empty() || content_type.starts_with("application/json") {
        return parse_jsonrpc_response(method, id, &body);
    }

    Err(anyhow!("unsupported MCP response content type '{content_type}' for method '{method}'"))
}

fn parse_jsonrpc_response(method: &str, expected_id: u64, body: &str) -> anyhow::Result<Value> {
    if body.trim().is_empty() {
        anyhow::bail!("empty MCP response body for method '{method}'");
    }

    let envelope: JsonRpcEnvelope = serde_json::from_str(body)
        .with_context(|| format!("invalid MCP JSON body for '{method}'"))?;
    resolve_jsonrpc_envelope("json", method, expected_id, envelope)
}

fn parse_sse_response(
    server_name: &str,
    method: &str,
    expected_id: u64,
    body: &str,
) -> anyhow::Result<Value> {
    let mut event_type = String::new();
    let mut data_lines = Vec::new();

    for raw_line in body.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.is_empty() {
            if let Some(result) =
                finalize_sse_event(server_name, method, expected_id, &event_type, &data_lines)?
            {
                return Ok(result);
            }
            event_type.clear();
            data_lines.clear();
            continue;
        }

        if let Some(stripped) = line.strip_prefix("event:") {
            event_type = stripped.trim_start().to_string();
        } else if let Some(stripped) = line.strip_prefix("data:") {
            data_lines.push(stripped.trim_start().to_string());
        }
    }

    if let Some(result) =
        finalize_sse_event(server_name, method, expected_id, &event_type, &data_lines)?
    {
        return Ok(result);
    }

    Err(anyhow!("MCP SSE response for method '{method}' ended without a JSON-RPC result"))
}

fn finalize_sse_event(
    server_name: &str,
    method: &str,
    expected_id: u64,
    event_type: &str,
    data_lines: &[String],
) -> anyhow::Result<Option<Value>> {
    if data_lines.is_empty() {
        return Ok(None);
    }

    if event_type == "endpoint" {
        tracing::warn!(
            server = %server_name,
            method = %method,
            "MCP server advertised deprecated HTTP+SSE endpoint; fallback is not implemented"
        );
        return Ok(None);
    }

    let data = data_lines.join("\n");
    let envelope: JsonRpcEnvelope = serde_json::from_str(&data)
        .with_context(|| format!("invalid MCP SSE event payload for method '{method}'"))?;

    match resolve_jsonrpc_envelope("sse", method, expected_id, envelope) {
        Ok(value) => Ok(Some(value)),
        Err(error) if is_non_matching_jsonrpc_event(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn resolve_jsonrpc_envelope(
    source: &str,
    method: &str,
    expected_id: u64,
    envelope: JsonRpcEnvelope,
) -> anyhow::Result<Value> {
    if let Some(id) = envelope.id.as_ref().and_then(Value::as_u64) {
        if id != expected_id {
            return Err(non_matching_jsonrpc_event());
        }
        return match (envelope.result, envelope.error) {
            (Some(result), None) => Ok(result),
            (None, Some(error)) => Err(anyhow!("MCP error {}: {}", error.code, error.message)),
            _ => Err(anyhow!("invalid {source} JSON-RPC response envelope for method '{method}'")),
        };
    }

    if let Some(server_method) = envelope.method {
        tracing::warn!(
            method = %method,
            server_method = %server_method,
            "ignoring unsupported server-to-client MCP message"
        );
        return Err(non_matching_jsonrpc_event());
    }

    Err(anyhow!("unexpected {source} JSON-RPC envelope without id for method '{method}'"))
}

fn non_matching_jsonrpc_event() -> anyhow::Error {
    anyhow!(NON_MATCHING_EVENT_ERROR)
}

fn is_non_matching_jsonrpc_event(error: &anyhow::Error) -> bool {
    error.to_string() == NON_MATCHING_EVENT_ERROR
}

fn http_status_error(method: &str, status: StatusCode, body: &str) -> anyhow::Error {
    if body.trim().is_empty() {
        return anyhow!("MCP HTTP {status} for method '{method}'");
    }

    if let Ok(envelope) = serde_json::from_str::<JsonRpcEnvelope>(body) {
        if let Some(error) = envelope.error {
            return anyhow!(
                "MCP HTTP {} for method '{}': {} ({})",
                status,
                method,
                error.message,
                error.code
            );
        }
    }

    anyhow!("MCP HTTP {} for method '{}': {}", status, method, body.trim())
}

#[derive(Debug, Deserialize)]
struct JsonRpcEnvelope {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: Option<String>,
    result: Option<Value>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

#[derive(Debug, Deserialize)]
struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    protocol_version: String,
}

#[derive(Debug, Deserialize)]
struct ListToolsResult {
    #[serde(default, rename = "nextCursor")]
    next_cursor: Option<String>,
    tools: Vec<McpDiscoveredTool>,
}

#[derive(Debug, Deserialize)]
struct McpDiscoveredTool {
    name: String,
    description: Option<String>,
    #[serde(rename = "inputSchema")]
    input_schema: Value,
}

#[derive(Debug, Deserialize)]
struct CallToolResult {
    content: Vec<McpContentBlock>,
    #[serde(default, rename = "structuredContent")]
    structured_content: Option<Value>,
    #[serde(default, rename = "isError")]
    is_error: bool,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct McpContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderName, HeaderValue, AUTHORIZATION};
    use reqwest::StatusCode;
    use tokio::io::{duplex, split};

    #[tokio::test]
    async fn mcp_client_initializes_and_lists_tools() {
        let (client_stream, server_stream) = duplex(4096);
        let (client_reader, client_writer) = split(client_stream);
        let (mut server_reader, mut server_writer) = split(server_stream);

        tokio::spawn(async move {
            let mut reader = BufReader::new(&mut server_reader).lines();

            let initialize = reader.next_line().await.unwrap().unwrap();
            let initialize: Value = serde_json::from_str(&initialize).unwrap();
            assert_eq!(initialize["method"], "initialize");
            assert_eq!(initialize["params"]["protocolVersion"], MCP_PROTOCOL_VERSION);
            let init_id = initialize["id"].as_u64().unwrap();
            server_writer
                .write_all(
                    format!(
                        "{{\"jsonrpc\":\"2.0\",\"id\":{init_id},\"result\":{{\"protocolVersion\":\"{MCP_PROTOCOL_VERSION}\"}}}}\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();

            let initialized = reader.next_line().await.unwrap().unwrap();
            let initialized: Value = serde_json::from_str(&initialized).unwrap();
            assert_eq!(initialized["method"], "notifications/initialized");

            let list_tools = reader.next_line().await.unwrap().unwrap();
            let list_tools: Value = serde_json::from_str(&list_tools).unwrap();
            assert_eq!(list_tools["method"], "tools/list");
            let list_id = list_tools["id"].as_u64().unwrap();
            server_writer
                .write_all(
                    format!(
                        "{{\"jsonrpc\":\"2.0\",\"id\":{list_id},\"result\":{{\"tools\":[{{\"name\":\"list_repositories\",\"description\":\"List repositories\",\"inputSchema\":{{\"type\":\"object\"}}}}]}}}}\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });

        let client =
            McpClient::from_test_io("github", Box::new(client_writer), Box::new(client_reader));
        client.initialize().await.unwrap();
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "list_repositories");
        assert_eq!(tools[0].description.as_deref(), Some("List repositories"));
    }

    #[tokio::test]
    async fn mcp_tool_execute_returns_text_and_error_flag() {
        let (client_stream, server_stream) = duplex(4096);
        let (client_reader, client_writer) = split(client_stream);
        let (mut server_reader, mut server_writer) = split(server_stream);

        tokio::spawn(async move {
            let mut reader = BufReader::new(&mut server_reader).lines();
            let request = reader.next_line().await.unwrap().unwrap();
            let request: Value = serde_json::from_str(&request).unwrap();
            assert_eq!(request["method"], "tools/call");
            assert_eq!(request["params"]["name"], "list_repositories");
            let id = request["id"].as_u64().unwrap();
            server_writer
                .write_all(
                    format!(
                        "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"content\":[{{\"type\":\"text\",\"text\":\"repo-a\\nrepo-b\"}}],\"isError\":false}}}}\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });

        let client = Arc::new(McpClient::from_test_io(
            "github",
            Box::new(client_writer),
            Box::new(client_reader),
        ));
        let tool = McpTool {
            qualified_name: "github_list_repositories".to_string(),
            server_name: "github".to_string(),
            original_name: "list_repositories".to_string(),
            input_schema: json!({"type": "object"}),
            client,
        };

        let output = tool
            .execute(json!({"owner": "octocat"}), &ToolContext::test(std::env::temp_dir()))
            .await
            .unwrap();
        assert_eq!(output.content, "repo-a\nrepo-b");
        assert!(!output.is_error);
    }

    #[tokio::test]
    async fn remote_http_transport_initializes_lists_and_calls_tools() {
        let token_var = "FLOK_TEST_GITHUB_PAT_TOKEN";
        std::env::set_var(token_var, "ghp_test_token");

        let config = McpServerConfig {
            url: Some("https://api.githubcopilot.com/mcp/".to_string()),
            bearer_token_env_var: Some(token_var.to_string()),
            ..McpServerConfig::default()
        };

        let (transport, requests) = HttpTransport::new_test(
            "github",
            &config,
            vec![
                HttpResponseData {
                    status: StatusCode::OK,
                    headers: HeaderMap::from_iter([(
                        HeaderName::from_static("mcp-session-id"),
                        HeaderValue::from_static("session-123"),
                    )]),
                    body: format!(
                        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"protocolVersion\":\"{MCP_PROTOCOL_VERSION}\"}}}}"
                    ),
                },
                HttpResponseData {
                    status: StatusCode::ACCEPTED,
                    headers: HeaderMap::new(),
                    body: String::new(),
                },
                HttpResponseData {
                    status: StatusCode::OK,
                    headers: HeaderMap::from_iter([(
                        CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    )]),
                    body: "{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[{\"name\":\"list_repositories\",\"description\":\"List repositories\",\"inputSchema\":{\"type\":\"object\"}}]}}".to_string(),
                },
                HttpResponseData {
                    status: StatusCode::OK,
                    headers: HeaderMap::from_iter([(
                        CONTENT_TYPE,
                        HeaderValue::from_static("text/event-stream"),
                    )]),
                    body: concat!(
                        "event: message\n",
                        "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{\"progress\":50}}\n",
                        "\n",
                        "event: message\n",
                        "data: {\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"repo-a\"}],\"isError\":false}}\n",
                        "\n"
                    )
                    .to_string(),
                },
            ],
        )
        .unwrap();
        let client = McpClient {
            server_name: "github".to_string(),
            transport: McpTransport::Http(Box::new(transport)),
            next_id: AtomicU64::new(1),
        };
        client.initialize().await.unwrap();
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "list_repositories");

        let result =
            client.call_tool("list_repositories", json!({"owner": "octocat"})).await.unwrap();
        assert_eq!(
            render_tool_result(&result.content, result.structured_content.as_ref()),
            "repo-a"
        );
        assert!(!result.is_error);

        let requests = requests.lock().await.clone();
        assert_eq!(requests.len(), 4);

        let initialize = &requests[0];
        assert_eq!(
            initialize.headers.get(AUTHORIZATION).and_then(|value| value.to_str().ok()),
            Some("Bearer ghp_test_token")
        );
        assert_eq!(
            initialize
                .headers
                .get(HeaderName::from_static("mcp-protocol-version"))
                .and_then(|value| value.to_str().ok()),
            Some(MCP_PROTOCOL_VERSION)
        );
        assert!(initialize.headers.get(HeaderName::from_static("mcp-session-id")).is_none());
        assert_eq!(initialize.body["method"], "initialize");
        assert_eq!(initialize.body["id"], 1);

        let initialized = &requests[1];
        assert_eq!(initialized.body["method"], "notifications/initialized");
        assert_eq!(
            initialized
                .headers
                .get(HeaderName::from_static("mcp-session-id"))
                .and_then(|value| value.to_str().ok()),
            Some("session-123")
        );

        let list_tools = &requests[2];
        assert_eq!(list_tools.body["method"], "tools/list");
        assert_eq!(list_tools.body["id"], 2);

        let call_tool = &requests[3];
        assert_eq!(call_tool.body["method"], "tools/call");
        assert_eq!(call_tool.body["params"]["name"], "list_repositories");
        assert_eq!(call_tool.body["id"], 3);

        std::env::remove_var(token_var);
    }

    #[test]
    fn build_remote_headers_requires_configured_token_env_var() {
        let config = McpServerConfig {
            url: Some("https://api.githubcopilot.com/mcp/".to_string()),
            bearer_token_env_var: Some("FLOK_TEST_MISSING_TOKEN".to_string()),
            ..McpServerConfig::default()
        };

        let error = build_remote_headers(&config).unwrap_err();
        assert!(error
            .to_string()
            .contains("MCP bearer token env var 'FLOK_TEST_MISSING_TOKEN' is not set"));
    }

    #[test]
    fn render_tool_result_falls_back_to_structured_content() {
        let rendered =
            render_tool_result(&[], Some(&json!({"repositories": ["repo-a", "repo-b"]})));
        assert!(rendered.contains("repo-a"));
        assert!(rendered.contains("repositories"));
    }
}
