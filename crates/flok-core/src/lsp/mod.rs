use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context};
use dashmap::DashMap;
use serde::Deserialize;
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{oneshot, Mutex};
use url::Url;

use crate::config::LspConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeverityFilter {
    Error,
    Warning,
    Information,
    Hint,
    All,
}

impl SeverityFilter {
    pub fn matches(self, severity: Option<u64>) -> bool {
        match self {
            Self::All => true,
            Self::Error => severity.unwrap_or(1) == 1,
            Self::Warning => severity.unwrap_or(1) == 2,
            Self::Information => severity.unwrap_or(1) == 3,
            Self::Hint => severity.unwrap_or(1) == 4,
        }
    }
}

#[derive(Clone)]
pub struct LspManager {
    project_root: PathBuf,
    config: LspConfig,
    enabled: bool,
    documents: Arc<Mutex<HashMap<PathBuf, TrackedDocument>>>,
    diagnostics: Arc<Mutex<HashMap<PathBuf, Vec<Diagnostic>>>>,
    server: Arc<Mutex<Option<Arc<LspServer>>>>,
    startup_error: Arc<Mutex<Option<String>>>,
}

impl LspManager {
    pub fn new(project_root: PathBuf, config: LspConfig) -> Self {
        let project_root = std::fs::canonicalize(&project_root).unwrap_or(project_root);
        let enabled = config.enabled && project_root.join("Cargo.toml").exists();
        Self {
            project_root,
            config,
            enabled,
            documents: Arc::new(Mutex::new(HashMap::new())),
            diagnostics: Arc::new(Mutex::new(HashMap::new())),
            server: Arc::new(Mutex::new(None)),
            startup_error: Arc::new(Mutex::new(None)),
        }
    }

    pub fn disabled(project_root: PathBuf) -> Self {
        let config = LspConfig { enabled: false, ..LspConfig::default() };
        Self::new(project_root, config)
    }

    pub fn tools_enabled(&self) -> bool {
        self.enabled
    }

    pub async fn track_read(&self, path: &Path, text: String) -> anyhow::Result<()> {
        self.track_document(path, text).await
    }

    pub async fn track_write(&self, path: &Path, text: String) -> anyhow::Result<()> {
        self.track_document(path, text).await
    }

    pub async fn diagnostics(
        &self,
        path: &Path,
        severity: SeverityFilter,
    ) -> anyhow::Result<String> {
        let normalized = self.normalize_supported_target(path)?;
        let is_file = normalized.is_file();

        if is_file {
            self.ensure_document_ready(&normalized).await?;
        } else {
            self.ensure_server().await?;
        }

        let timeout = self.request_timeout();
        let deadline = Instant::now() + timeout;

        loop {
            let snapshot = self.diagnostics.lock().await.clone();
            if let Some(formatted) = format_diagnostics_snapshot(
                &self.project_root,
                &normalized,
                is_file,
                &snapshot,
                severity,
            ) {
                return Ok(formatted);
            }

            if Instant::now() >= deadline {
                return Ok("No diagnostics found.".to_string());
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn goto_definition(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> anyhow::Result<String> {
        let normalized = self.ensure_document_ready(path).await?;
        let server = self.ensure_server().await?;
        let params = serde_json::json!({
            "textDocument": { "uri": path_to_uri(&normalized)? },
            "position": {
                "line": one_based_to_zero_based(line)?,
                "character": character,
            }
        });
        let response =
            server.request("textDocument/definition", params, self.request_timeout()).await?;
        let locations = parse_definition_locations(&response)?;
        format_locations("definition", &self.project_root, &locations)
    }

    pub async fn find_references(
        &self,
        path: &Path,
        line: u32,
        character: u32,
        include_declaration: bool,
    ) -> anyhow::Result<String> {
        let normalized = self.ensure_document_ready(path).await?;
        let server = self.ensure_server().await?;
        let params = serde_json::json!({
            "textDocument": { "uri": path_to_uri(&normalized)? },
            "position": {
                "line": one_based_to_zero_based(line)?,
                "character": character,
            },
            "context": {
                "includeDeclaration": include_declaration,
            }
        });
        let response =
            server.request("textDocument/references", params, self.request_timeout()).await?;
        let locations = parse_locations_array(&response)?;
        format_locations("reference", &self.project_root, &locations)
    }

    pub async fn document_symbols(&self, path: &Path) -> anyhow::Result<String> {
        let normalized = self.ensure_document_ready(path).await?;
        let server = self.ensure_server().await?;
        let params = serde_json::json!({
            "textDocument": { "uri": path_to_uri(&normalized)? },
        });
        let response =
            server.request("textDocument/documentSymbol", params, self.request_timeout()).await?;
        format_document_symbols(&self.project_root, &response)
    }

    pub async fn workspace_symbols(&self, query: &str, limit: usize) -> anyhow::Result<String> {
        let server = self.ensure_server().await?;
        let params = serde_json::json!({ "query": query });
        let response = server.request("workspace/symbol", params, self.request_timeout()).await?;
        format_workspace_symbols(&self.project_root, &response, limit)
    }

    async fn track_document(&self, path: &Path, text: String) -> anyhow::Result<()> {
        let Some(normalized) = self.normalize_relevant_path(path)? else {
            return Ok(());
        };

        let server = self.current_server().await;
        let mut action = None;

        {
            let mut documents = self.documents.lock().await;
            if let Some(existing) = documents.get_mut(&normalized) {
                if existing.text != text {
                    existing.text.clone_from(&text);
                    existing.version += 1;
                    if existing.opened_in_server && server.is_some() {
                        action = Some(SyncAction::Change {
                            path: normalized.clone(),
                            text,
                            version: existing.version,
                        });
                    } else if server.is_some() {
                        existing.opened_in_server = true;
                        action = Some(SyncAction::Open {
                            path: normalized.clone(),
                            text,
                            version: existing.version,
                        });
                    }
                } else if !existing.opened_in_server && server.is_some() {
                    existing.opened_in_server = true;
                    action = Some(SyncAction::Open {
                        path: normalized.clone(),
                        text,
                        version: existing.version,
                    });
                }
            } else {
                documents.insert(
                    normalized.clone(),
                    TrackedDocument {
                        text: text.clone(),
                        version: 1,
                        opened_in_server: server.is_some(),
                    },
                );
                if server.is_some() {
                    action = Some(SyncAction::Open { path: normalized.clone(), text, version: 1 });
                }
            }
        }

        if let (Some(server), Some(action)) = (server, action) {
            self.apply_sync_action(&server, action).await?;
        }

        Ok(())
    }

    async fn ensure_document_ready(&self, path: &Path) -> anyhow::Result<PathBuf> {
        let normalized = self.normalize_supported_file(path)?;

        let is_tracked = {
            let documents = self.documents.lock().await;
            documents.contains_key(&normalized)
        };

        if !is_tracked {
            let text = tokio::fs::read_to_string(&normalized)
                .await
                .with_context(|| format!("failed to read {}", normalized.display()))?;
            self.track_document(&normalized, text).await?;
        }

        let server = self.ensure_server().await?;
        let action = {
            let mut documents = self.documents.lock().await;
            let document = documents
                .get_mut(&normalized)
                .ok_or_else(|| anyhow!("file is not tracked for lsp: {}", normalized.display()))?;
            if document.opened_in_server {
                None
            } else {
                let text = document.text.clone();
                let version = document.version;
                document.opened_in_server = true;
                Some(SyncAction::Open { path: normalized.clone(), text, version })
            }
        };

        if let Some(action) = action {
            self.apply_sync_action(&server, action).await?;
        }

        Ok(normalized)
    }

    async fn apply_sync_action(
        &self,
        server: &Arc<LspServer>,
        action: SyncAction,
    ) -> anyhow::Result<()> {
        match action {
            SyncAction::Open { path, text, version } => {
                if let Err(error) = server
                    .notify(
                        "textDocument/didOpen",
                        serde_json::json!({
                            "textDocument": {
                                "uri": path_to_uri(&path)?,
                                "languageId": "rust",
                                "version": version,
                                "text": text,
                            }
                        }),
                    )
                    .await
                {
                    self.mark_closed(&path).await;
                    return Err(error);
                }
            }
            SyncAction::Change { path, text, version } => {
                server
                    .notify(
                        "textDocument/didChange",
                        serde_json::json!({
                            "textDocument": {
                                "uri": path_to_uri(&path)?,
                                "version": version,
                            },
                            "contentChanges": [{ "text": text }],
                        }),
                    )
                    .await?;
            }
        }

        Ok(())
    }

    async fn mark_closed(&self, path: &Path) {
        let mut documents = self.documents.lock().await;
        if let Some(document) = documents.get_mut(path) {
            document.opened_in_server = false;
        }
    }

    async fn current_server(&self) -> Option<Arc<LspServer>> {
        self.server.lock().await.clone()
    }

    async fn ensure_server(&self) -> anyhow::Result<Arc<LspServer>> {
        if !self.enabled {
            bail!("native lsp is disabled or no Rust project was detected");
        }

        if let Some(message) = self.startup_error.lock().await.clone() {
            bail!(message);
        }

        let mut guard = self.server.lock().await;
        if let Some(server) = guard.as_ref() {
            return Ok(Arc::clone(server));
        }

        let server = match LspServer::spawn(
            self.project_root.clone(),
            self.config.rust.command.clone(),
            self.config.rust.args.clone(),
            Arc::clone(&self.diagnostics),
            self.request_timeout(),
        )
        .await
        {
            Ok(server) => Arc::new(server),
            Err(error) => {
                let message = error.to_string();
                *self.startup_error.lock().await = Some(message.clone());
                return Err(anyhow!(message));
            }
        };

        *guard = Some(Arc::clone(&server));
        Ok(server)
    }

    fn request_timeout(&self) -> Duration {
        Duration::from_millis(self.config.request_timeout_ms.max(1))
    }

    fn normalize_supported_target(&self, path: &Path) -> anyhow::Result<PathBuf> {
        if !self.enabled {
            bail!("native lsp is disabled or no Rust project was detected");
        }

        if path.is_dir() {
            let normalized = std::fs::canonicalize(path)
                .with_context(|| format!("failed to resolve {}", path.display()))?;
            if normalized.starts_with(&self.project_root) {
                return Ok(normalized);
            }
            bail!("path must be inside the project root");
        }

        self.normalize_supported_file(path)
    }

    fn normalize_supported_file(&self, path: &Path) -> anyhow::Result<PathBuf> {
        self.normalize_relevant_path(path)?.ok_or_else(|| {
            anyhow!("native lsp currently supports Rust files inside the project root")
        })
    }

    fn normalize_relevant_path(&self, path: &Path) -> anyhow::Result<Option<PathBuf>> {
        if !self.enabled {
            return Ok(None);
        }

        let normalized = normalize_path(path)?;
        if !normalized.starts_with(&self.project_root) {
            return Ok(None);
        }

        if normalized.extension().and_then(std::ffi::OsStr::to_str) != Some("rs") {
            return Ok(None);
        }

        Ok(Some(normalized))
    }
}

impl std::fmt::Debug for LspManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspManager")
            .field("project_root", &self.project_root)
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

struct LspServer {
    project_root: PathBuf,
    _child: Mutex<Child>,
    writer: Arc<Mutex<ChildStdin>>,
    pending: Arc<DashMap<u64, oneshot::Sender<anyhow::Result<serde_json::Value>>>>,
    next_request_id: AtomicU64,
}

impl LspServer {
    async fn spawn(
        project_root: PathBuf,
        command: String,
        args: Vec<String>,
        diagnostics: Arc<Mutex<HashMap<PathBuf, Vec<Diagnostic>>>>,
        timeout: Duration,
    ) -> anyhow::Result<Self> {
        let mut child = Command::new(&command)
            .args(&args)
            .current_dir(&project_root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to start lsp server `{command}`"))?;

        let stdin = child.stdin.take().ok_or_else(|| anyhow!("failed to open lsp stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("failed to open lsp stdout"))?;

        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(drain_stderr(stderr));
        }

        let server = Self {
            project_root: project_root.clone(),
            _child: Mutex::new(child),
            writer: Arc::new(Mutex::new(stdin)),
            pending: Arc::new(DashMap::new()),
            next_request_id: AtomicU64::new(1),
        };

        server.spawn_reader(stdout, diagnostics);

        let root_uri = path_to_uri(&project_root)?;
        let initialize = serde_json::json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {},
            "clientInfo": {
                "name": "flok",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "workspaceFolders": [{
                "uri": path_to_uri(&project_root)?,
                "name": project_root
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .unwrap_or("workspace"),
            }],
            "trace": "off",
        });
        let _ = server.request("initialize", initialize, timeout).await?;
        server.notify("initialized", serde_json::json!({})).await?;

        Ok(server)
    }

    fn spawn_reader(
        &self,
        stdout: ChildStdout,
        diagnostics: Arc<Mutex<HashMap<PathBuf, Vec<Diagnostic>>>>,
    ) {
        let pending = self.pending.clone();
        let writer = self.writer.clone();
        let project_root = self.project_root.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                let message = match read_message(&mut reader).await {
                    Ok(Some(message)) => message,
                    Ok(None) => break,
                    Err(error) => {
                        fail_pending(&pending, &error);
                        break;
                    }
                };

                if let Err(error) = handle_incoming_message(
                    &pending,
                    &diagnostics,
                    &writer,
                    &project_root,
                    &message,
                )
                .await
                {
                    tracing::debug!(%error, "failed to handle lsp message");
                }
            }

            let error = anyhow!("lsp server connection closed");
            fail_pending(&pending, &error);
        });
    }

    async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> anyhow::Result<serde_json::Value> {
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.insert(id, tx);

        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        if let Err(error) = self.send(payload).await {
            let _ = self.pending.remove(&id);
            return Err(error);
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(anyhow!("lsp response channel closed for {method}")),
            Err(_) => {
                let _ = self.pending.remove(&id);
                Err(anyhow!("lsp request timed out for {method}"))
            }
        }
    }

    async fn notify(&self, method: &str, params: serde_json::Value) -> anyhow::Result<()> {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.send(payload).await
    }

    async fn send(&self, payload: serde_json::Value) -> anyhow::Result<()> {
        let mut writer = self.writer.lock().await;
        write_rpc_message(&mut *writer, &payload).await
    }
}

#[derive(Clone)]
struct TrackedDocument {
    text: String,
    version: i32,
    opened_in_server: bool,
}

enum SyncAction {
    Open { path: PathBuf, text: String, version: i32 },
    Change { path: PathBuf, text: String, version: i32 },
}

#[derive(Debug, Clone, Deserialize)]
struct Diagnostic {
    range: Range,
    severity: Option<u64>,
    message: String,
    source: Option<String>,
    code: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct Range {
    start: Position,
    end: Position,
}

#[derive(Debug, Clone, Deserialize)]
struct Position {
    line: u32,
    character: u32,
}

#[derive(Debug, Clone, Deserialize)]
struct Location {
    uri: String,
    range: Range,
}

#[derive(Debug, Clone, Deserialize)]
struct LocationLink {
    #[serde(rename = "targetUri")]
    target_uri: String,
    #[serde(rename = "targetSelectionRange")]
    target_selection_range: Range,
}

#[derive(Debug, Clone, Deserialize)]
struct DocumentSymbol {
    name: String,
    kind: u32,
    range: Range,
    detail: Option<String>,
    #[serde(default)]
    children: Vec<DocumentSymbol>,
}

#[derive(Debug, Clone, Deserialize)]
struct SymbolInformation {
    name: String,
    kind: u32,
    location: Location,
    #[serde(rename = "containerName")]
    container_name: Option<String>,
}

#[derive(Deserialize)]
struct PublishDiagnosticsParams {
    uri: String,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Deserialize)]
struct RpcErrorObject {
    code: i64,
    message: String,
}

async fn handle_incoming_message(
    pending: &DashMap<u64, oneshot::Sender<anyhow::Result<serde_json::Value>>>,
    diagnostics: &Arc<Mutex<HashMap<PathBuf, Vec<Diagnostic>>>>,
    writer: &Arc<Mutex<ChildStdin>>,
    project_root: &Path,
    message: &[u8],
) -> anyhow::Result<()> {
    let value: serde_json::Value = serde_json::from_slice(message)?;

    if value.get("method").and_then(serde_json::Value::as_str)
        == Some("textDocument/publishDiagnostics")
    {
        let params: PublishDiagnosticsParams = serde_json::from_value(
            value.get("params").cloned().ok_or_else(|| anyhow!("missing diagnostics params"))?,
        )?;
        if let Some(path) = uri_to_path(project_root, &params.uri) {
            diagnostics.lock().await.insert(path, params.diagnostics);
        }
        return Ok(());
    }

    if let Some(response) = response_for_server_request(project_root, &value)? {
        let mut writer = writer.lock().await;
        write_rpc_message(&mut *writer, &response).await?;
        return Ok(());
    }

    let Some(id) = value.get("id").and_then(serde_json::Value::as_u64) else {
        return Ok(());
    };

    let Some((_, sender)) = pending.remove(&id) else {
        return Ok(());
    };

    if let Some(error_value) = value.get("error") {
        let error: RpcErrorObject = serde_json::from_value(error_value.clone())?;
        let _ = sender.send(Err(anyhow!("lsp error {}: {}", error.code, error.message)));
        return Ok(());
    }

    let result = value.get("result").cloned().unwrap_or(serde_json::Value::Null);
    let _ = sender.send(Ok(result));
    Ok(())
}

fn response_for_server_request(
    project_root: &Path,
    value: &serde_json::Value,
) -> anyhow::Result<Option<serde_json::Value>> {
    let Some(method) = value.get("method").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    let Some(id) = value.get("id").cloned() else {
        return Ok(None);
    };

    let result = match method {
        "workspace/configuration" => configuration_response(value.get("params")),
        "workspace/workspaceFolders" => workspace_folders_response(project_root)?,
        _ => serde_json::Value::Null,
    };

    Ok(Some(serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })))
}

fn configuration_response(params: Option<&serde_json::Value>) -> serde_json::Value {
    let items = params
        .and_then(|params| params.get("items"))
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();

    serde_json::Value::Array(
        items
            .into_iter()
            .map(|item| match item.get("section").and_then(serde_json::Value::as_str) {
                Some(section)
                    if section == "rust-analyzer" || section.starts_with("rust-analyzer.") =>
                {
                    serde_json::json!({})
                }
                _ => serde_json::Value::Null,
            })
            .collect(),
    )
}

fn workspace_folders_response(project_root: &Path) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::json!([
        {
            "uri": path_to_uri(project_root)?,
            "name": project_root
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or("workspace"),
        }
    ]))
}

fn fail_pending(
    pending: &DashMap<u64, oneshot::Sender<anyhow::Result<serde_json::Value>>>,
    error: &anyhow::Error,
) {
    let message = error.to_string();
    let ids: Vec<u64> = pending.iter().map(|entry| *entry.key()).collect();
    for id in ids {
        if let Some((_, sender)) = pending.remove(&id) {
            let _ = sender.send(Err(anyhow!(message.clone())));
        }
    }
}

async fn read_message<R>(reader: &mut R) -> anyhow::Result<Option<Vec<u8>>>
where
    R: AsyncBufRead + Unpin,
{
    let mut content_length = None;

    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).await?;
        if bytes == 0 {
            return Ok(None);
        }

        if line == "\r\n" {
            break;
        }

        let trimmed = line.trim_end_matches(['\r', '\n']);
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            content_length = Some(value.trim().parse::<usize>()?);
        }
    }

    let length = content_length.ok_or_else(|| anyhow!("missing content length header"))?;
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload).await?;
    Ok(Some(payload))
}

async fn write_rpc_message<W>(writer: &mut W, payload: &serde_json::Value) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let bytes = serde_json::to_vec(payload)?;
    let header = format!("Content-Length: {}\r\n\r\n", bytes.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

async fn drain_stderr(stderr: tokio::process::ChildStderr) {
    let mut reader = BufReader::new(stderr);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => tracing::debug!(message = line.trim_end(), "lsp stderr"),
            Err(error) => {
                tracing::debug!(%error, "failed reading lsp stderr");
                break;
            }
        }
    }
}

fn normalize_path(path: &Path) -> anyhow::Result<PathBuf> {
    if path.exists() {
        return std::fs::canonicalize(path)
            .with_context(|| format!("failed to resolve {}", path.display()));
    }

    let parent = path.parent().ok_or_else(|| anyhow!("path has no parent: {}", path.display()))?;
    let parent = std::fs::canonicalize(parent)
        .with_context(|| format!("failed to resolve {}", parent.display()))?;
    let file_name =
        path.file_name().ok_or_else(|| anyhow!("path has no file name: {}", path.display()))?;
    Ok(parent.join(file_name))
}

fn path_to_uri(path: &Path) -> anyhow::Result<String> {
    Url::from_file_path(path)
        .map(|url| url.to_string())
        .map_err(|()| anyhow!("failed to convert {} to file uri", path.display()))
}

fn uri_to_path(project_root: &Path, uri: &str) -> Option<PathBuf> {
    let path = Url::parse(uri).ok()?.to_file_path().ok()?;
    if !path.starts_with(project_root) {
        return None;
    }
    Some(path)
}

fn one_based_to_zero_based(line: u32) -> anyhow::Result<u32> {
    line.checked_sub(1).ok_or_else(|| anyhow!("line must be 1-based and greater than zero"))
}

fn format_diagnostics_snapshot(
    project_root: &Path,
    target: &Path,
    is_file: bool,
    snapshot: &HashMap<PathBuf, Vec<Diagnostic>>,
    severity: SeverityFilter,
) -> Option<String> {
    let has_entry = if is_file {
        snapshot.contains_key(target)
    } else {
        snapshot.keys().any(|path| path.starts_with(target))
    };

    if !has_entry {
        return None;
    }

    let mut lines = Vec::new();
    let mut entries: Vec<_> = snapshot
        .iter()
        .filter(|(path, _)| if is_file { *path == target } else { path.starts_with(target) })
        .collect();
    entries.sort_by(|(left, _), (right, _)| left.cmp(right));

    for (path, diagnostics) in entries {
        for diagnostic in
            diagnostics.iter().filter(|diagnostic| severity.matches(diagnostic.severity))
        {
            let code = diagnostic.code.as_ref().map(format_diagnostic_code).unwrap_or_default();
            let source = diagnostic.source.as_deref().unwrap_or("lsp");
            lines.push(format!(
                "{}:{}:{} [{}{}{}] {}",
                display_path(project_root, path),
                diagnostic.range.start.line + 1,
                diagnostic.range.start.character,
                severity_name(diagnostic.severity),
                if code.is_empty() { "" } else { "/" },
                code,
                diagnostic.message,
            ));
            if source != "lsp" {
                if let Some(line) = lines.last_mut() {
                    let _ = write!(line, " ({source})");
                }
            }
        }
    }

    if lines.is_empty() {
        Some("No diagnostics found.".to_string())
    } else {
        Some(lines.join("\n"))
    }
}

fn format_diagnostic_code(code: &serde_json::Value) -> String {
    match code {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Number(value) => value.to_string(),
        _ => String::new(),
    }
}

fn severity_name(severity: Option<u64>) -> &'static str {
    match severity.unwrap_or(1) {
        1 => "error",
        2 => "warning",
        3 => "information",
        4 => "hint",
        _ => "unknown",
    }
}

fn parse_definition_locations(value: &serde_json::Value) -> anyhow::Result<Vec<Location>> {
    if value.is_null() {
        return Ok(Vec::new());
    }

    if let Ok(location) = serde_json::from_value::<Location>(value.clone()) {
        return Ok(vec![location]);
    }

    if let Ok(locations) = serde_json::from_value::<Vec<Location>>(value.clone()) {
        return Ok(locations);
    }

    let links: Vec<LocationLink> = serde_json::from_value(value.clone())?;
    Ok(links
        .into_iter()
        .map(|link| Location { uri: link.target_uri, range: link.target_selection_range })
        .collect())
}

fn parse_locations_array(value: &serde_json::Value) -> anyhow::Result<Vec<Location>> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    serde_json::from_value(value.clone()).map_err(Into::into)
}

fn format_locations(
    kind: &str,
    project_root: &Path,
    locations: &[Location],
) -> anyhow::Result<String> {
    if locations.is_empty() {
        return Ok(format!("No {kind} found."));
    }

    let mut lines = Vec::new();
    for location in locations {
        let path = uri_to_path(project_root, &location.uri)
            .ok_or_else(|| anyhow!("invalid file uri in lsp response"))?;
        lines.push(format!(
            "{}:{}:{}-{}:{}",
            display_path(project_root, &path),
            location.range.start.line + 1,
            location.range.start.character,
            location.range.end.line + 1,
            location.range.end.character,
        ));
    }
    Ok(lines.join("\n"))
}

fn format_document_symbols(
    project_root: &Path,
    value: &serde_json::Value,
) -> anyhow::Result<String> {
    if value.is_null() {
        return Ok("No symbols found.".to_string());
    }

    if let Ok(symbols) = serde_json::from_value::<Vec<DocumentSymbol>>(value.clone()) {
        if symbols.is_empty() {
            return Ok("No symbols found.".to_string());
        }

        let mut lines = Vec::new();
        for symbol in &symbols {
            push_document_symbol(&mut lines, symbol, 0);
        }
        return Ok(lines.join("\n"));
    }

    let infos: Vec<SymbolInformation> = serde_json::from_value(value.clone())?;
    if infos.is_empty() {
        return Ok("No symbols found.".to_string());
    }

    let mut lines = Vec::new();
    for info in infos {
        let path = uri_to_path(project_root, &info.location.uri)
            .ok_or_else(|| anyhow!("invalid file uri in lsp response"))?;
        lines.push(format!(
            "{} {} — {}:{}:{}",
            symbol_kind_name(info.kind),
            info.name,
            display_path(project_root, &path),
            info.location.range.start.line + 1,
            info.location.range.start.character,
        ));
    }
    Ok(lines.join("\n"))
}

fn format_workspace_symbols(
    project_root: &Path,
    value: &serde_json::Value,
    limit: usize,
) -> anyhow::Result<String> {
    if value.is_null() {
        return Ok("No symbols found.".to_string());
    }

    let symbols: Vec<SymbolInformation> = serde_json::from_value(value.clone())?;
    if symbols.is_empty() {
        return Ok("No symbols found.".to_string());
    }

    let mut lines = Vec::new();
    for symbol in symbols.into_iter().take(limit) {
        let path = uri_to_path(project_root, &symbol.location.uri)
            .ok_or_else(|| anyhow!("invalid file uri in lsp response"))?;
        let container = symbol.container_name.unwrap_or_default();
        lines.push(format!(
            "{} {}{} — {}:{}:{}",
            symbol_kind_name(symbol.kind),
            symbol.name,
            if container.is_empty() { String::new() } else { format!(" ({container})") },
            display_path(project_root, &path),
            symbol.location.range.start.line + 1,
            symbol.location.range.start.character,
        ));
    }
    Ok(lines.join("\n"))
}

fn push_document_symbol(lines: &mut Vec<String>, symbol: &DocumentSymbol, indent: usize) {
    let detail = symbol.detail.as_deref().map(|detail| format!(" — {detail}")).unwrap_or_default();
    lines.push(format!(
        "{}{} {}:{}:{}{}",
        "  ".repeat(indent),
        symbol_kind_name(symbol.kind),
        symbol.name,
        symbol.range.start.line + 1,
        symbol.range.start.character,
        detail,
    ));
    for child in &symbol.children {
        push_document_symbol(lines, child, indent + 1);
    }
}

fn symbol_kind_name(kind: u32) -> &'static str {
    match kind {
        1 => "File",
        2 => "Module",
        3 => "Namespace",
        4 => "Package",
        5 => "Class",
        6 => "Method",
        7 => "Property",
        8 => "Field",
        9 => "Constructor",
        10 => "Enum",
        11 => "Interface",
        12 => "Function",
        13 => "Variable",
        14 => "Constant",
        15 => "String",
        16 => "Number",
        17 => "Boolean",
        18 => "Array",
        19 => "Object",
        20 => "Key",
        21 => "Null",
        22 => "EnumMember",
        23 => "Struct",
        24 => "Event",
        25 => "Operator",
        26 => "TypeParameter",
        _ => "Symbol",
    }
}

fn display_path(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root).unwrap_or(path).display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsp_tools_disable_without_cargo_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let manager = LspManager::new(dir.path().to_path_buf(), LspConfig::default());
        assert!(!manager.tools_enabled());
    }

    #[test]
    fn lsp_tools_enable_for_rust_workspace() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        let manager = LspManager::new(dir.path().to_path_buf(), LspConfig::default());
        assert!(manager.tools_enabled());
    }

    #[test]
    fn configuration_requests_get_minimal_rust_analyzer_response() {
        let dir = tempfile::tempdir().unwrap();
        let response = response_for_server_request(
            dir.path(),
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "workspace/configuration",
                "params": {
                    "items": [
                        { "section": "rust-analyzer" },
                        { "section": "other.section" }
                    ]
                }
            }),
        )
        .unwrap()
        .unwrap();

        assert_eq!(response["id"], serde_json::json!(7));
        assert_eq!(response["result"], serde_json::json!([{}, null]));
    }

    #[test]
    fn register_capability_requests_receive_null_result() {
        let dir = tempfile::tempdir().unwrap();
        let response = response_for_server_request(
            dir.path(),
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 11,
                "method": "client/registerCapability",
                "params": {
                    "registrations": []
                }
            }),
        )
        .unwrap()
        .unwrap();

        assert_eq!(response["id"], serde_json::json!(11));
        assert!(response["result"].is_null());
    }

    #[tokio::test]
    async fn rust_analyzer_smoke_test() {
        if !command_available("rust-analyzer").await {
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let src = dir.path().join("src/lib.rs");
        std::fs::write(
            &src,
            "pub fn helper() -> i32 {\n    1\n}\n\npub fn call() -> i32 {\n    helper()\n}\n",
        )
        .unwrap();

        let manager = LspManager::new(dir.path().to_path_buf(), LspConfig::default());
        let definitions = manager.goto_definition(&src, 5, 4).await.unwrap();
        assert!(definitions.contains("src/lib.rs:1:"), "{definitions}");
    }

    async fn command_available(command: &str) -> bool {
        Command::new(command).arg("--version").status().await.is_ok_and(|status| status.success())
    }
}
