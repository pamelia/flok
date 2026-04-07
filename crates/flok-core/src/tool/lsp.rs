use std::sync::Arc;

use crate::lsp::{LspManager, SeverityFilter};

use super::{Tool, ToolContext, ToolOutput};

pub struct LspDiagnosticsTool {
    manager: Arc<LspManager>,
}

impl LspDiagnosticsTool {
    pub fn new(manager: Arc<LspManager>) -> Self {
        Self { manager }
    }
}

#[async_trait::async_trait]
impl Tool for LspDiagnosticsTool {
    fn name(&self) -> &'static str {
        "lsp_diagnostics"
    }

    fn description(&self) -> &'static str {
        "Get LSP diagnostics for a Rust file or directory."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["file_path"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Rust file or directory to inspect"
                },
                "severity": {
                    "type": "string",
                    "enum": ["error", "warning", "information", "hint", "all"],
                    "description": "Optional severity filter (default: all)"
                }
            }
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let file_path = required_str(&args, "file_path")?;
        let severity = parse_severity(args.get("severity").and_then(serde_json::Value::as_str));
        let path = resolve_path(&ctx.project_root, file_path);
        let content = self.manager.diagnostics(&path, severity).await?;
        Ok(ToolOutput::success(content))
    }
}

pub struct LspGotoDefinitionTool {
    manager: Arc<LspManager>,
}

impl LspGotoDefinitionTool {
    pub fn new(manager: Arc<LspManager>) -> Self {
        Self { manager }
    }
}

#[async_trait::async_trait]
impl Tool for LspGotoDefinitionTool {
    fn name(&self) -> &'static str {
        "lsp_goto_definition"
    }

    fn description(&self) -> &'static str {
        "Find the definition location for a Rust symbol."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["file_path", "line", "character"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Rust file containing the symbol reference"
                },
                "line": {
                    "type": "integer",
                    "description": "1-based line number"
                },
                "character": {
                    "type": "integer",
                    "description": "0-based character offset"
                }
            }
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let file_path = required_str(&args, "file_path")?;
        let line = required_u32(&args, "line")?;
        let character = required_u32(&args, "character")?;
        let path = resolve_path(&ctx.project_root, file_path);
        let content = self.manager.goto_definition(&path, line, character).await?;
        Ok(ToolOutput::success(content))
    }
}

pub struct LspFindReferencesTool {
    manager: Arc<LspManager>,
}

impl LspFindReferencesTool {
    pub fn new(manager: Arc<LspManager>) -> Self {
        Self { manager }
    }
}

#[async_trait::async_trait]
impl Tool for LspFindReferencesTool {
    fn name(&self) -> &'static str {
        "lsp_find_references"
    }

    fn description(&self) -> &'static str {
        "Find references to a Rust symbol."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["file_path", "line", "character"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Rust file containing the symbol reference"
                },
                "line": {
                    "type": "integer",
                    "description": "1-based line number"
                },
                "character": {
                    "type": "integer",
                    "description": "0-based character offset"
                },
                "include_declaration": {
                    "type": "boolean",
                    "description": "Whether to include the symbol declaration in results"
                }
            }
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let file_path = required_str(&args, "file_path")?;
        let line = required_u32(&args, "line")?;
        let character = required_u32(&args, "character")?;
        let include_declaration =
            args.get("include_declaration").and_then(serde_json::Value::as_bool).unwrap_or(false);
        let path = resolve_path(&ctx.project_root, file_path);
        let content =
            self.manager.find_references(&path, line, character, include_declaration).await?;
        Ok(ToolOutput::success(content))
    }
}

pub struct LspSymbolsTool {
    manager: Arc<LspManager>,
}

impl LspSymbolsTool {
    pub fn new(manager: Arc<LspManager>) -> Self {
        Self { manager }
    }
}

#[async_trait::async_trait]
impl Tool for LspSymbolsTool {
    fn name(&self) -> &'static str {
        "lsp_symbols"
    }

    fn description(&self) -> &'static str {
        "List document symbols for a Rust file or search workspace symbols."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["scope"],
            "properties": {
                "scope": {
                    "type": "string",
                    "enum": ["document", "workspace"],
                    "description": "Search within one file or across the workspace"
                },
                "file_path": {
                    "type": "string",
                    "description": "Required when scope=document"
                },
                "query": {
                    "type": "string",
                    "description": "Required when scope=workspace"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum workspace results to return (default: 50)"
                }
            }
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let scope = required_str(&args, "scope")?;
        let content = match scope {
            "document" => {
                let file_path = required_str(&args, "file_path")?;
                let path = resolve_path(&ctx.project_root, file_path);
                self.manager.document_symbols(&path).await?
            }
            "workspace" => {
                let query = required_str(&args, "query")?;
                let limit =
                    args.get("limit").and_then(serde_json::Value::as_u64).unwrap_or(50) as usize;
                self.manager.workspace_symbols(query, limit).await?
            }
            _ => return Ok(ToolOutput::error("scope must be `document` or `workspace`")),
        };

        Ok(ToolOutput::success(content))
    }
}

fn required_str<'a>(args: &'a serde_json::Value, key: &str) -> anyhow::Result<&'a str> {
    args.get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: {key}"))
}

fn required_u32(args: &serde_json::Value, key: &str) -> anyhow::Result<u32> {
    let value = args
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: {key}"))?;
    u32::try_from(value).map_err(|_| anyhow::anyhow!("parameter out of range for u32: {key}"))
}

fn parse_severity(value: Option<&str>) -> SeverityFilter {
    match value.unwrap_or("all") {
        "error" => SeverityFilter::Error,
        "warning" => SeverityFilter::Warning,
        "information" => SeverityFilter::Information,
        "hint" => SeverityFilter::Hint,
        _ => SeverityFilter::All,
    }
}

fn resolve_path(project_root: &std::path::Path, file_path: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(file_path);
    let resolved = if path.is_absolute() { path.to_path_buf() } else { project_root.join(path) };
    match std::fs::canonicalize(&resolved) {
        Ok(canonical) if canonical.starts_with(project_root) => canonical,
        _ => resolved,
    }
}
