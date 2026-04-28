//! # Configuration
//!
//! Configuration loading and hot-reload for flok. Configuration is loaded
//! from TOML files with the following precedence (highest to lowest):
//!
//! 1. Environment variables (`FLOK_*`)
//! 2. Project config (`flok.toml` in project root)
//! 3. Global config (`~/.config/flok/flok.toml`)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use arc_swap::ArcSwap;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize, Serializer};

/// Top-level flok configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FlokConfig {
    /// Default model to use when the `--model` CLI flag is not provided.
    ///
    /// Accepts any alias supported by [`crate::provider::ModelRegistry`]
    /// (for example `"sonnet"`, `"opus-4.7"`, `"gpt-5.4"`), or a fully
    /// qualified ID like `"anthropic/claude-opus-4-7"`.
    pub model: Option<String>,
    /// Default reasoning effort to use when the provider supports it.
    pub reasoning_effort: Option<crate::provider::ReasoningEffort>,
    /// Provider configurations keyed by provider name.
    pub provider: HashMap<String, ProviderConfig>,
    /// Per-built-in-agent routing and prompt overrides.
    pub agents: HashMap<String, AgentConfig>,
    /// MCP server configurations keyed by server name.
    ///
    /// Supports both `[mcp_servers.<name>]` and the legacy alias `[mcp.<name>]`.
    #[serde(alias = "mcp")]
    pub mcp_servers: HashMap<String, McpServerConfig>,
    /// TUI runtime behavior.
    pub tui: TuiConfig,
    pub lsp: LspConfig,
    /// Git worktree isolation settings.
    pub worktree: WorktreeConfig,
    /// Permission rules keyed by permission type.
    ///
    /// Each entry can be:
    /// - A bare action: `bash = "allow"` (applies to all patterns)
    /// - A table of pattern → action: `[permission.bash]` with `"git *" = "allow"`
    ///
    /// # Example
    ///
    /// ```toml
    /// [permission]
    /// read = "allow"
    /// glob = "allow"
    ///
    /// [permission.bash]
    /// "*" = "allow"
    /// "rm -rf *" = "deny"
    ///
    /// [permission.external_directory]
    /// "*" = "ask"
    /// ```
    pub permission: HashMap<String, PermissionToolConfig>,
    /// Runtime provider fallback behavior.
    pub runtime_fallback: RuntimeFallbackConfig,
    /// Request-time model routing behavior.
    pub intelligent_routing: IntelligentRoutingConfig,
    /// Tool output compression configuration.
    pub output_compression: OutputCompressionConfig,
}

/// Controls whether the TUI uses the terminal's alternate screen buffer.
///
/// `auto` preserves scrollback inside Zellij by avoiding the alternate screen there,
/// while keeping the fullscreen experience elsewhere.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AltScreenMode {
    /// Automatically disable alternate screen inside Zellij, enable elsewhere.
    #[default]
    Auto,
    /// Always use the alternate screen buffer.
    Always,
    /// Never use the alternate screen buffer.
    Never,
}

/// TUI-specific configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TuiConfig {
    /// Alternate-screen behavior for interactive mode.
    pub alternate_screen: AltScreenMode,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self { alternate_screen: AltScreenMode::Auto }
    }
}

/// Configuration for a single MCP server.
///
/// The shape is intentionally close to Codex-style MCP config so users can
/// translate existing setups with minimal friction.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct McpServerConfig {
    /// Disable the server without removing config.
    pub disabled: Option<bool>,
    /// Per-server tool call timeout in seconds. Defaults to 30 when unset.
    pub timeout_seconds: Option<u64>,
    /// Stdio transport command.
    pub command: Option<String>,
    /// Stdio transport arguments.
    pub args: Option<Vec<String>>,
    /// Optional stdio working directory.
    pub cwd: Option<PathBuf>,
    /// Extra environment variables for stdio servers.
    pub env: Option<HashMap<String, String>>,
    /// Remote transport URL.
    pub url: Option<String>,
    /// Extra remote headers.
    pub headers: Option<HashMap<String, String>>,
    /// Env var containing a bearer token for remote HTTP auth.
    pub bearer_token_env_var: Option<String>,
}

impl McpServerConfig {
    /// Effective timeout in seconds.
    #[must_use]
    pub fn timeout_seconds(&self) -> u64 {
        self.timeout_seconds.unwrap_or(30)
    }

    /// Whether this server is disabled.
    #[must_use]
    pub fn disabled(&self) -> bool {
        self.disabled.unwrap_or(false)
    }

    /// Whether this entry is configured for stdio transport.
    #[must_use]
    pub fn is_stdio(&self) -> bool {
        self.command.is_some()
    }

    /// Whether this entry is configured for remote transport.
    #[must_use]
    pub fn is_remote(&self) -> bool {
        self.url.is_some()
    }
}

/// Versioned runtime config snapshot used by long-lived sessions.
#[derive(Debug, Clone)]
pub struct ConfigSnapshot {
    pub version: u64,
    pub config: Arc<FlokConfig>,
}

/// Atomically swappable runtime config handle.
#[derive(Debug, Clone)]
pub struct LiveConfig {
    current: Arc<ArcSwap<ConfigSnapshot>>,
    next_version: Arc<AtomicU64>,
}

impl LiveConfig {
    /// Create a new live config starting at version 1.
    #[must_use]
    pub fn new(config: FlokConfig) -> Self {
        Self {
            current: Arc::new(ArcSwap::from_pointee(ConfigSnapshot {
                version: 1,
                config: Arc::new(config),
            })),
            next_version: Arc::new(AtomicU64::new(2)),
        }
    }

    /// Load the current versioned snapshot.
    #[must_use]
    pub fn snapshot(&self) -> Arc<ConfigSnapshot> {
        self.current.load_full()
    }

    /// Load just the current config value.
    #[must_use]
    pub fn current(&self) -> Arc<FlokConfig> {
        let snapshot = self.current.load_full();
        Arc::clone(&snapshot.config)
    }

    /// Replace the current config and return the new version number.
    pub fn store(&self, config: FlokConfig) -> u64 {
        let version = self.next_version.fetch_add(1, Ordering::Relaxed);
        self.current.store(Arc::new(ConfigSnapshot { version, config: Arc::new(config) }));
        version
    }
}

/// File state used by the config watcher to detect edits, creation, and deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigFileStamp {
    pub exists: bool,
    pub modified: Option<SystemTime>,
}

/// Tool output compression configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct OutputCompressionConfig {
    /// Whether output compression is enabled.
    pub enabled: bool,
    /// Skip compression entirely below this line count.
    pub passthrough_threshold_lines: usize,
    /// Maximum line budget before truncation fires.
    pub max_lines: usize,
    /// Number of lines preserved at the start during truncation.
    pub head_lines: usize,
    /// Number of lines preserved at the end during truncation.
    pub tail_lines: usize,
    /// Final hard character budget after all line-based stages.
    pub max_chars: usize,
    /// Minimum exact-repeat run length to group.
    pub group_exact_min: usize,
    /// Minimum normalized-repeat run length to group.
    pub group_similar_min: usize,
    /// Tool names that should use this pipeline.
    pub apply_to_tools: Vec<String>,
}

impl Default for OutputCompressionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            passthrough_threshold_lines: 40,
            max_lines: 200,
            head_lines: 50,
            tail_lines: 50,
            max_chars: 20_000,
            group_exact_min: 3,
            group_similar_min: 5,
            apply_to_tools: vec!["bash".to_string()],
        }
    }
}

/// Per-agent routing and system-prompt overrides.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AgentConfig {
    /// Preferred model for this built-in agent.
    pub model: Option<String>,
    /// Preferred reasoning effort for this built-in agent.
    pub reasoning_effort: Option<crate::provider::ReasoningEffort>,
    /// Ordered fallback model IDs or aliases that replace the provider chain.
    pub fallback_models: Vec<String>,
    /// Extra text appended to the built-in system prompt.
    pub prompt_append: Option<String>,
}

/// Runtime provider fallback behavior.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RuntimeFallbackConfig {
    /// Whether runtime provider fallback is enabled.
    pub enabled: bool,
    /// HTTP status codes that trigger fallback.
    pub retry_on_errors: Vec<u16>,
    /// Total attempts across the full fallback chain.
    pub max_attempts: u32,
    /// Provider cooldown duration after a retriable failure.
    pub cooldown_seconds: u64,
    /// Whether to emit `BusEvent::ProviderFallback` notifications.
    pub notify_on_fallback: bool,
}

impl Default for RuntimeFallbackConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            retry_on_errors: vec![429, 500, 502, 503, 529],
            max_attempts: 3,
            cooldown_seconds: 120,
            notify_on_fallback: true,
        }
    }
}

/// Request-time model routing behavior.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct IntelligentRoutingConfig {
    /// Whether request-time model routing is enabled.
    pub enabled: bool,
    /// Minimum complexity score required before upgrading the request model.
    pub complexity_threshold: u32,
    /// Optional hard session spend budget in micro-USD.
    ///
    /// When the current session cost is at or above this value, routing prefers
    /// the cheapest eligible configured model for subsequent requests.
    pub max_session_cost_microusd: Option<u64>,
}

impl Default for IntelligentRoutingConfig {
    fn default() -> Self {
        Self { enabled: true, complexity_threshold: 4, max_session_cost_microusd: None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LspConfig {
    pub enabled: bool,
    pub request_timeout_ms: u64,
    pub rust: RustLspConfig,
    pub javascript: JavascriptLspConfig,
    pub python: PythonLspConfig,
    pub go: GoLspConfig,
    pub java: JavaLspConfig,
}

impl Default for LspConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            request_timeout_ms: 5_000,
            rust: RustLspConfig::default(),
            javascript: JavascriptLspConfig::default(),
            python: PythonLspConfig::default(),
            go: GoLspConfig::default(),
            java: JavaLspConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RustLspConfig {
    pub command: String,
    pub args: Vec<String>,
}

impl Default for RustLspConfig {
    fn default() -> Self {
        Self { command: "rust-analyzer".to_string(), args: Vec::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct JavascriptLspConfig {
    pub command: String,
    pub args: Vec<String>,
}

impl Default for JavascriptLspConfig {
    fn default() -> Self {
        Self {
            command: "typescript-language-server".to_string(),
            args: vec!["--stdio".to_string()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PythonLspConfig {
    pub command: String,
    pub args: Vec<String>,
}

impl Default for PythonLspConfig {
    fn default() -> Self {
        Self { command: "pyright-langserver".to_string(), args: vec!["--stdio".to_string()] }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GoLspConfig {
    pub command: String,
    pub args: Vec<String>,
}

impl Default for GoLspConfig {
    fn default() -> Self {
        Self { command: "gopls".to_string(), args: Vec::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct JavaLspConfig {
    pub command: String,
    pub args: Vec<String>,
}

impl Default for JavaLspConfig {
    fn default() -> Self {
        Self { command: "jdtls".to_string(), args: Vec::new() }
    }
}

/// Permission configuration for a single tool/permission type.
///
/// Supports two forms in TOML:
/// - Bare action: `read = "allow"` → applies to all patterns (`"*"`)
/// - Pattern table: `[permission.bash]` with specific patterns
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PermissionToolConfig {
    /// A bare action string: applies to all patterns (`"*"`).
    Action(crate::permission::PermissionAction),
    /// A map of pattern → action.
    Patterns(std::collections::HashMap<String, crate::permission::PermissionAction>),
}

impl PermissionToolConfig {
    /// Convert this config entry into permission rules for the given permission type.
    pub fn into_rules(self, permission: &str) -> Vec<crate::permission::PermissionRule> {
        match self {
            Self::Action(action) => {
                vec![crate::permission::PermissionRule::new(permission, "*", action)]
            }
            Self::Patterns(patterns) => patterns
                .into_iter()
                .map(|(pattern, action)| {
                    crate::permission::PermissionRule::new(permission, pattern, action)
                })
                .collect(),
        }
    }
}

/// Convert all permission config entries into a flat list of rules.
pub fn permission_config_to_rules<S: std::hash::BuildHasher>(
    config: &HashMap<String, PermissionToolConfig, S>,
) -> Vec<crate::permission::PermissionRule> {
    config
        .iter()
        .flat_map(|(permission, tool_config)| tool_config.clone().into_rules(permission))
        .collect()
}

/// Configuration for git worktree isolation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorktreeConfig {
    /// Enable worktree isolation for background agents.
    pub enabled: bool,
    /// Automatically merge non-conflicting changes on agent completion.
    pub auto_merge: bool,
    /// Remove worktree directory after successful merge.
    pub cleanup_on_complete: bool,
}

impl Default for WorktreeConfig {
    fn default() -> Self {
        Self { enabled: true, auto_merge: true, cleanup_on_complete: true }
    }
}

/// Configuration for a single LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProviderConfig {
    /// API key. Read ONLY from the config file at runtime. Wrapped in
    /// `SecretString` for zeroize-on-drop and redacted `Debug`.
    #[serde(serialize_with = "serialize_api_key_opt")]
    pub api_key: Option<SecretString>,
    /// Base URL override.
    pub base_url: Option<String>,
    /// Default model for this provider. Used as a fallback when neither
    /// `--model` CLI flag nor top-level `model` config is set — the first
    /// provider (alphabetical by key) with a `default_model` wins.
    pub default_model: Option<String>,
    /// Ordered list of fallback providers to try after retriable failures.
    pub fallback: Vec<String>,
}

/// Serialize an `Option<SecretString>` by exposing its plaintext.
///
/// This is the SOLE egress point where the secret plaintext leaves
/// `SecretString`. Used only by `run_auth_login` when persisting the
/// config file back to disk. All other code paths must use
/// `expose_secret()` at the exact use site (e.g. HTTP header build).
// `serialize_with` from serde requires `&Option<T>`; `Option<&T>` is not
// a valid shape for this hook, so the `ref_option` lint is a false positive.
#[allow(clippy::ref_option)]
fn serialize_api_key_opt<S: Serializer>(
    secret: &Option<SecretString>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match secret {
        Some(s) => serializer.serialize_some(s.expose_secret()),
        None => serializer.serialize_none(),
    }
}

/// Detect the project root by walking up from `start_dir` looking for markers.
///
/// Checks for: `.git`, `Cargo.toml`, `pom.xml`, `build.gradle`,
/// `build.gradle.kts`, `package.json`, `go.mod`, `pyproject.toml`, `flok.toml`,
/// `.flok/flok.toml`. Returns the first directory containing any marker, or
/// `start_dir` if none found.
pub fn detect_project_root(start_dir: &Path) -> PathBuf {
    const MARKERS: &[&str] = &[
        ".git",
        "Cargo.toml",
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
        "package.json",
        "go.mod",
        "pyproject.toml",
        "flok.toml",
        ".flok/flok.toml",
    ];

    let mut dir = start_dir.to_path_buf();
    loop {
        for marker in MARKERS {
            if dir.join(marker).exists() {
                return dir;
            }
        }
        if !dir.pop() {
            // Reached filesystem root — fall back to start_dir
            return start_dir.to_path_buf();
        }
    }
}

/// Load configuration from the given project root.
///
/// Merges config from multiple layers (project > global > defaults).
/// Project config values override global config values.
///
/// # Errors
///
/// Returns an error if a config file exists but cannot be parsed.
pub fn load_config(project_root: &Path) -> anyhow::Result<FlokConfig> {
    let mut config = FlokConfig::default();

    // Layer 1: Global config (lowest priority)
    if let Some(config_dir) = directories::BaseDirs::new().map(|d| d.config_dir().to_path_buf()) {
        let global_config = config_dir.join("flok").join("flok.toml");
        if global_config.exists() {
            let content = std::fs::read_to_string(&global_config)?;
            let global: FlokConfig = toml::from_str(&content)?;
            merge_config(&mut config, &global);
        }
    }

    // Layer 2: Project-local config (higher priority)
    let project_config = project_root.join("flok.toml");
    if project_config.exists() {
        let content = std::fs::read_to_string(&project_config)?;
        let project: FlokConfig = toml::from_str(&content)?;
        merge_config(&mut config, &project);
    }

    // Layer 3: .flok/flok.toml (highest file priority)
    let dotflok_config = project_root.join(".flok").join("flok.toml");
    if dotflok_config.exists() {
        let content = std::fs::read_to_string(&dotflok_config)?;
        let dotflok: FlokConfig = toml::from_str(&content)?;
        merge_config(&mut config, &dotflok);
    }

    Ok(config)
}

/// All config paths that participate in the merged runtime snapshot.
#[must_use]
pub fn config_paths(project_root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(config_dir) = directories::BaseDirs::new().map(|d| d.config_dir().to_path_buf()) {
        paths.push(config_dir.join("flok").join("flok.toml"));
    }
    paths.push(project_root.join("flok.toml"));
    paths.push(project_root.join(".flok").join("flok.toml"));
    paths
}

/// Capture the current existence/modification state for all relevant config files.
pub fn capture_config_stamps(
    paths: &[PathBuf],
) -> anyhow::Result<HashMap<PathBuf, ConfigFileStamp>> {
    paths
        .iter()
        .map(|path| {
            let stamp = match std::fs::metadata(path) {
                Ok(metadata) => {
                    ConfigFileStamp { exists: true, modified: metadata.modified().ok() }
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    ConfigFileStamp { exists: false, modified: None }
                }
                Err(err) => return Err(anyhow::Error::new(err)),
            };
            Ok((path.clone(), stamp))
        })
        .collect()
}

/// Return the config paths whose file stamp changed between two snapshots.
#[must_use]
pub fn diff_config_stamps<PS, CS>(
    previous: &HashMap<PathBuf, ConfigFileStamp, PS>,
    current: &HashMap<PathBuf, ConfigFileStamp, CS>,
) -> Vec<PathBuf>
where
    PS: std::hash::BuildHasher,
    CS: std::hash::BuildHasher,
{
    current
        .iter()
        .filter_map(|(path, stamp)| (previous.get(path) != Some(stamp)).then_some(path.clone()))
        .collect()
}

/// Merge `source` config into `target`. Source values override target values.
fn merge_config(target: &mut FlokConfig, source: &FlokConfig) {
    if source.model.is_some() {
        target.model.clone_from(&source.model);
    }
    if source.reasoning_effort.is_some() {
        target.reasoning_effort = source.reasoning_effort;
    }
    for (key, value) in &source.provider {
        let entry = target.provider.entry(key.clone()).or_default();
        if value.api_key.is_some() {
            entry.api_key.clone_from(&value.api_key);
        }
        if value.base_url.is_some() {
            entry.base_url.clone_from(&value.base_url);
        }
        if value.default_model.is_some() {
            entry.default_model.clone_from(&value.default_model);
        }
        if !value.fallback.is_empty() {
            entry.fallback.clone_from(&value.fallback);
        }
    }
    for (key, value) in &source.agents {
        let entry = target.agents.entry(key.clone()).or_default();
        if value.model.is_some() {
            entry.model.clone_from(&value.model);
        }
        if value.reasoning_effort.is_some() {
            entry.reasoning_effort = value.reasoning_effort;
        }
        if !value.fallback_models.is_empty() {
            entry.fallback_models.clone_from(&value.fallback_models);
        }
        if value.prompt_append.is_some() {
            entry.prompt_append.clone_from(&value.prompt_append);
        }
    }
    for (key, value) in &source.mcp_servers {
        let entry = target.mcp_servers.entry(key.clone()).or_default();
        if value.disabled.is_some() {
            entry.disabled = value.disabled;
        }
        if value.timeout_seconds.is_some() {
            entry.timeout_seconds = value.timeout_seconds;
        }
        if value.command.is_some() {
            entry.command.clone_from(&value.command);
        }
        if value.args.is_some() {
            entry.args.clone_from(&value.args);
        }
        if value.cwd.is_some() {
            entry.cwd.clone_from(&value.cwd);
        }
        if value.env.is_some() {
            entry.env.clone_from(&value.env);
        }
        if value.url.is_some() {
            entry.url.clone_from(&value.url);
        }
        if value.headers.is_some() {
            entry.headers.clone_from(&value.headers);
        }
        if value.bearer_token_env_var.is_some() {
            entry.bearer_token_env_var.clone_from(&value.bearer_token_env_var);
        }
    }
    target.tui = source.tui.clone();
    target.lsp = source.lsp.clone();
    // Worktree config: source overrides target entirely if present in source file
    // (serde default handles missing fields; if explicitly set in source, override)
    target.worktree = source.worktree.clone();
    // Permission config: merge at the permission-type level
    for (key, value) in &source.permission {
        target.permission.insert(key.clone(), value.clone());
    }
    if source.runtime_fallback != RuntimeFallbackConfig::default() {
        target.runtime_fallback = source.runtime_fallback.clone();
    }
    if source.intelligent_routing != IntelligentRoutingConfig::default() {
        target.intelligent_routing = source.intelligent_routing.clone();
    }
    let default_output_compression = OutputCompressionConfig::default();
    if source.output_compression.enabled != default_output_compression.enabled {
        target.output_compression.enabled = source.output_compression.enabled;
    }
    if source.output_compression.passthrough_threshold_lines
        != default_output_compression.passthrough_threshold_lines
    {
        target.output_compression.passthrough_threshold_lines =
            source.output_compression.passthrough_threshold_lines;
    }
    if source.output_compression.max_lines != default_output_compression.max_lines {
        target.output_compression.max_lines = source.output_compression.max_lines;
    }
    if source.output_compression.head_lines != default_output_compression.head_lines {
        target.output_compression.head_lines = source.output_compression.head_lines;
    }
    if source.output_compression.tail_lines != default_output_compression.tail_lines {
        target.output_compression.tail_lines = source.output_compression.tail_lines;
    }
    if source.output_compression.max_chars != default_output_compression.max_chars {
        target.output_compression.max_chars = source.output_compression.max_chars;
    }
    if source.output_compression.group_exact_min != default_output_compression.group_exact_min {
        target.output_compression.group_exact_min = source.output_compression.group_exact_min;
    }
    if source.output_compression.group_similar_min != default_output_compression.group_similar_min {
        target.output_compression.group_similar_min = source.output_compression.group_similar_min;
    }
    if source.output_compression.apply_to_tools != default_output_compression.apply_to_tools {
        target
            .output_compression
            .apply_to_tools
            .clone_from(&source.output_compression.apply_to_tools);
    }
}

/// Ensure XDG-compliant directories exist.
///
/// Creates: config dir, data dir, cache dir, state dir.
///
/// # Errors
///
/// Returns an error if directory creation fails.
pub fn ensure_directories() -> anyhow::Result<()> {
    if let Some(dirs) = directories::BaseDirs::new() {
        let paths = [
            dirs.config_dir().join("flok"),
            dirs.data_dir().join("flok"),
            dirs.cache_dir().join("flok"),
            flok_state_root(),
        ];
        for path in &paths {
            std::fs::create_dir_all(path)?;
        }
    }
    Ok(())
}

/// Root directory for generated flok runtime state.
///
/// This intentionally lives under `~/.flok` instead of the project tree so
/// compactions, plans, memory, snapshots, and other agent artifacts do not
/// pollute repositories.
#[must_use]
pub fn flok_state_root() -> PathBuf {
    if cfg!(test) {
        return std::env::temp_dir().join("flok-test-state");
    }

    if let Some(home) = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()) {
        return home.join(".flok");
    }

    std::env::temp_dir().join("flok")
}

/// Stable per-project directory for generated runtime state.
#[must_use]
pub fn project_state_dir(project_root: &Path) -> PathBuf {
    let canonical =
        std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let slug = canonical
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .map(sanitize_project_slug)
        .filter(|slug| !slug.is_empty())
        .unwrap_or_else(|| "project".to_string());
    let hash = blake3::hash(canonical.to_string_lossy().as_bytes()).to_hex().to_string();
    flok_state_root().join("projects").join(format!("{slug}-{}", &hash[..16]))
}

fn sanitize_project_slug(value: &str) -> String {
    let slug: String = value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') { ch } else { '-' })
        .collect();
    slug.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::{ExposeSecret, SecretString};

    #[test]
    fn default_config_is_valid() {
        let config = FlokConfig::default();
        assert!(config.provider.is_empty());
        assert!(config.reasoning_effort.is_none());
        assert!(config.mcp_servers.is_empty());
        assert_eq!(config.tui.alternate_screen, AltScreenMode::Auto);
        assert!(config.lsp.enabled);
        assert_eq!(config.lsp.rust.command, "rust-analyzer");
        assert_eq!(config.lsp.javascript.command, "typescript-language-server");
        assert_eq!(config.lsp.python.command, "pyright-langserver");
        assert_eq!(config.lsp.go.command, "gopls");
        assert_eq!(config.lsp.java.command, "jdtls");
        assert_eq!(config.runtime_fallback, RuntimeFallbackConfig::default());
        assert_eq!(config.intelligent_routing, IntelligentRoutingConfig::default());
        assert_eq!(config.output_compression, OutputCompressionConfig::default());
    }

    #[test]
    fn project_state_dir_is_outside_project_root() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = project_state_dir(dir.path());
        assert!(!state_dir.starts_with(dir.path()));
        assert!(state_dir.to_string_lossy().contains("projects"));
    }

    #[test]
    fn parse_output_compression_config() {
        let toml_str = r#"
            [output_compression]
            enabled = true
            max_lines = 300
            head_lines = 80
            tail_lines = 80
            apply_to_tools = ["bash", "grep"]
        "#;

        let config: FlokConfig = toml::from_str(toml_str).unwrap();
        assert!(config.output_compression.enabled);
        assert_eq!(config.output_compression.max_lines, 300);
        assert_eq!(config.output_compression.head_lines, 80);
        assert_eq!(config.output_compression.tail_lines, 80);
        assert_eq!(config.output_compression.apply_to_tools, vec!["bash", "grep"]);
        assert_eq!(config.output_compression.max_chars, 20_000);
    }

    #[test]
    fn output_compression_defaults_when_missing() {
        let config: FlokConfig = toml::from_str("").unwrap();
        assert_eq!(config.output_compression, OutputCompressionConfig::default());
    }

    #[test]
    fn parse_lsp_config() {
        let toml_str = r#"
            [lsp]
            enabled = true
            request_timeout_ms = 1200

            [lsp.rust]
            command = "custom-ra"
            args = ["--stdio"]

            [lsp.javascript]
            command = "custom-ts-lsp"
            args = ["--stdio"]

            [lsp.python]
            command = "custom-py-lsp"
            args = ["--stdio"]

            [lsp.go]
            command = "custom-gopls"
            args = ["serve"]

            [lsp.java]
            command = "custom-jdtls"
            args = ["--data", ".flok/jdtls-workspace"]
        "#;

        let config: FlokConfig = toml::from_str(toml_str).unwrap();
        assert!(config.lsp.enabled);
        assert_eq!(config.lsp.request_timeout_ms, 1200);
        assert_eq!(config.lsp.rust.command, "custom-ra");
        assert_eq!(config.lsp.rust.args, vec!["--stdio"]);
        assert_eq!(config.lsp.javascript.command, "custom-ts-lsp");
        assert_eq!(config.lsp.javascript.args, vec!["--stdio"]);
        assert_eq!(config.lsp.python.command, "custom-py-lsp");
        assert_eq!(config.lsp.python.args, vec!["--stdio"]);
        assert_eq!(config.lsp.go.command, "custom-gopls");
        assert_eq!(config.lsp.go.args, vec!["serve"]);
        assert_eq!(config.lsp.java.command, "custom-jdtls");
        assert_eq!(config.lsp.java.args, vec!["--data", ".flok/jdtls-workspace"]);
    }

    #[test]
    fn parse_tui_config() {
        let toml_str = r#"
            [tui]
            alternate_screen = "never"
        "#;

        let config: FlokConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.tui.alternate_screen, AltScreenMode::Never);
    }

    #[test]
    fn parse_mcp_servers_config_codex_style() {
        let toml_str = r#"
            [mcp_servers.github]
            url = "https://api.githubcopilot.com/mcp/"
            bearer_token_env_var = "GITHUB_PAT_TOKEN"
            timeout_seconds = 45

            [mcp_servers.filesystem]
            command = "npx"
            args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
        "#;

        let config: FlokConfig = toml::from_str(toml_str).unwrap();
        let github = &config.mcp_servers["github"];
        assert_eq!(github.url.as_deref(), Some("https://api.githubcopilot.com/mcp/"));
        assert_eq!(github.bearer_token_env_var.as_deref(), Some("GITHUB_PAT_TOKEN"));
        assert_eq!(github.timeout_seconds(), 45);
        assert!(github.is_remote());

        let filesystem = &config.mcp_servers["filesystem"];
        assert_eq!(filesystem.command.as_deref(), Some("npx"));
        assert_eq!(
            filesystem.args.as_deref(),
            Some(
                vec![
                    "-y".to_string(),
                    "@modelcontextprotocol/server-filesystem".to_string(),
                    ".".to_string(),
                ]
                .as_slice()
            )
        );
        assert!(filesystem.is_stdio());
        assert_eq!(filesystem.timeout_seconds(), 30);
    }

    #[test]
    fn parse_mcp_servers_config_accepts_legacy_alias() {
        let toml_str = r#"
            [mcp.search]
            url = "https://mcp.example.com/search"
        "#;

        let config: FlokConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.mcp_servers.get("search").and_then(|server| server.url.as_deref()),
            Some("https://mcp.example.com/search")
        );
    }

    #[test]
    fn merge_mcp_server_overlay_preserves_unspecified_fields() {
        let mut base: FlokConfig = toml::from_str(
            r#"
            [mcp_servers.github]
            url = "https://api.githubcopilot.com/mcp/"
            bearer_token_env_var = "GITHUB_PAT_TOKEN"
            timeout_seconds = 30
        "#,
        )
        .unwrap();

        let overlay: FlokConfig = toml::from_str(
            r"
            [mcp_servers.github]
            timeout_seconds = 60
            disabled = true
        ",
        )
        .unwrap();

        merge_config(&mut base, &overlay);
        let github = &base.mcp_servers["github"];
        assert_eq!(github.url.as_deref(), Some("https://api.githubcopilot.com/mcp/"));
        assert_eq!(github.bearer_token_env_var.as_deref(), Some("GITHUB_PAT_TOKEN"));
        assert_eq!(github.timeout_seconds(), 60);
        assert!(github.disabled());
    }

    #[test]
    fn live_config_store_increments_version() {
        let live = LiveConfig::new(FlokConfig::default());
        assert_eq!(live.snapshot().version, 1);

        let version =
            live.store(FlokConfig { model: Some("gpt-5.4".to_string()), ..FlokConfig::default() });

        let snapshot = live.snapshot();
        assert_eq!(version, 2);
        assert_eq!(snapshot.version, 2);
        assert_eq!(snapshot.config.model.as_deref(), Some("gpt-5.4"));
    }

    #[test]
    fn config_paths_include_project_and_dotflok_layers() {
        let dir = tempfile::tempdir().unwrap();
        let paths = config_paths(dir.path());
        assert!(paths.iter().any(|path| path.ends_with("flok.toml")));
        assert!(paths.iter().any(|path| path.ends_with(std::path::Path::new(".flok/flok.toml"))));
    }

    #[test]
    fn diff_config_stamps_detects_created_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flok.toml");
        let paths = vec![path.clone()];
        let before = capture_config_stamps(&paths).unwrap();

        std::fs::write(&path, "model = \"sonnet\"\n").unwrap();
        let after = capture_config_stamps(&paths).unwrap();

        assert_eq!(diff_config_stamps(&before, &after), vec![path]);
    }

    #[test]
    fn detect_project_root_finds_git() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();

        let root = detect_project_root(&sub);
        assert_eq!(root, dir.path());
    }

    #[test]
    fn detect_project_root_finds_cargo_toml() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("src");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();

        let root = detect_project_root(&sub);
        assert_eq!(root, dir.path());
    }

    #[test]
    fn detect_project_root_finds_java_build_file() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("server").join("src");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(dir.path().join("pom.xml"), "<project />").unwrap();

        let root = detect_project_root(&sub);
        assert_eq!(root, dir.path());
    }

    #[test]
    fn detect_project_root_fallback_to_start() {
        let dir = tempfile::tempdir().unwrap();
        // No markers — should return start_dir itself
        let root = detect_project_root(dir.path());
        assert_eq!(root, dir.path());
    }

    #[test]
    fn config_merging() {
        let mut base = FlokConfig::default();
        let overlay = FlokConfig {
            provider: [(
                "anthropic".to_string(),
                ProviderConfig {
                    api_key: Some(SecretString::from("key-123".to_string())),
                    base_url: None,
                    default_model: None,
                    fallback: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        merge_config(&mut base, &overlay);
        assert_eq!(
            base.provider["anthropic"].api_key.as_ref().map(SecretString::expose_secret),
            Some("key-123"),
        );
    }

    #[test]
    fn merge_tui_overlay_overrides_alt_screen_mode() {
        let mut base = FlokConfig {
            tui: TuiConfig { alternate_screen: AltScreenMode::Always },
            ..Default::default()
        };
        let overlay = FlokConfig {
            tui: TuiConfig { alternate_screen: AltScreenMode::Never },
            ..Default::default()
        };

        merge_config(&mut base, &overlay);
        assert_eq!(base.tui.alternate_screen, AltScreenMode::Never);
    }

    #[test]
    fn parse_minimal_toml() {
        let toml_str = r#"
            [provider.anthropic]
            api_key = "sk-test-123"
        "#;
        let config: FlokConfig = toml::from_str(toml_str).unwrap();
        assert!(config.provider.contains_key("anthropic"));
        assert_eq!(
            config.provider["anthropic"].api_key.as_ref().map(SecretString::expose_secret),
            Some("sk-test-123"),
        );
    }

    #[test]
    fn serialize_roundtrip_exposes_key() {
        use std::collections::HashMap;
        let mut provider = HashMap::new();
        provider.insert(
            "anthropic".to_string(),
            ProviderConfig {
                api_key: Some(SecretString::from("sk-test-roundtrip".to_string())),
                base_url: None,
                default_model: None,
                fallback: Vec::new(),
            },
        );
        let config = FlokConfig { provider, ..Default::default() };

        let toml_string = toml::to_string_pretty(&config).expect("serialize");
        assert!(
            toml_string.contains("sk-test-roundtrip"),
            "expected plaintext in serialized TOML, got:\n{toml_string}"
        );

        let parsed: FlokConfig = toml::from_str(&toml_string).expect("deserialize");
        assert_eq!(
            parsed.provider["anthropic"].api_key.as_ref().map(SecretString::expose_secret),
            Some("sk-test-roundtrip"),
        );
    }

    #[test]
    fn debug_format_is_redacted() {
        let provider = ProviderConfig {
            api_key: Some(SecretString::from("plain-text-xyz".to_string())),
            base_url: None,
            default_model: None,
            fallback: Vec::new(),
        };
        let rendered = format!("{provider:?}");
        assert!(!rendered.contains("plain-text-xyz"), "Debug output leaked plaintext: {rendered}");
        assert!(rendered.contains("REDACTED"), "Debug output missing REDACTED marker: {rendered}");
    }

    #[test]
    fn parse_permission_bare_action() {
        let toml_str = r#"
            [permission]
            read = "allow"
            glob = "allow"
        "#;
        let config: FlokConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.permission.len(), 2);

        let rules = permission_config_to_rules(&config.permission);
        assert_eq!(rules.len(), 2);
        // Both should have pattern "*"
        for rule in &rules {
            assert_eq!(rule.pattern, "*");
            assert_eq!(rule.action, crate::permission::PermissionAction::Allow);
        }
    }

    #[test]
    fn parse_permission_pattern_table() {
        let toml_str = r#"
            [permission.bash]
            "*" = "allow"
            "rm -rf *" = "deny"
            "docker *" = "ask"
        "#;
        let config: FlokConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.permission.len(), 1);

        let rules = permission_config_to_rules(&config.permission);
        assert_eq!(rules.len(), 3);
        // All should be "bash" permission
        for rule in &rules {
            assert_eq!(rule.permission, "bash");
        }
    }

    #[test]
    fn parse_permission_mixed() {
        let toml_str = r#"
            [permission]
            read = "allow"
            glob = "allow"

            [permission.bash]
            "*" = "allow"
            "rm -rf *" = "deny"

            [permission.external_directory]
            "*" = "ask"
        "#;
        let config: FlokConfig = toml::from_str(toml_str).unwrap();
        // "read", "glob", "bash", "external_directory"
        assert_eq!(config.permission.len(), 4);

        let rules = permission_config_to_rules(&config.permission);
        // read (1) + glob (1) + bash (2) + external_directory (1) = 5
        assert_eq!(rules.len(), 5);
    }

    #[test]
    fn permission_merge_overrides() {
        let mut base = FlokConfig::default();
        let overlay: FlokConfig = toml::from_str(
            r#"
            [permission.bash]
            "*" = "allow"
            "#,
        )
        .unwrap();
        merge_config(&mut base, &overlay);
        assert!(base.permission.contains_key("bash"));
    }

    #[test]
    fn parse_config_with_default_model() {
        let toml_str = r#"
            model = "opus-4.7"
            reasoning_effort = "high"

            [provider.anthropic]
            api_key = "sk-test"
            default_model = "opus-4.7"
        "#;
        let config: FlokConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.model.as_deref(), Some("opus-4.7"));
        assert_eq!(config.reasoning_effort, Some(crate::provider::ReasoningEffort::High));
        assert_eq!(config.provider["anthropic"].default_model.as_deref(), Some("opus-4.7"),);
        assert_eq!(
            config.provider["anthropic"].api_key.as_ref().map(SecretString::expose_secret),
            Some("sk-test"),
        );
    }

    #[test]
    fn parse_config_without_default_model_defaults_to_none() {
        let toml_str = r#"
            [provider.anthropic]
            api_key = "sk-test"
        "#;
        let config: FlokConfig = toml::from_str(toml_str).unwrap();
        assert!(config.model.is_none());
        assert!(config.provider["anthropic"].default_model.is_none());
        assert!(config.provider["anthropic"].fallback.is_empty());
    }

    #[test]
    fn parse_agents_config_full() {
        let toml_str = r#"
            [agents.explore]
            model = "haiku-4-5"
            reasoning_effort = "low"
            fallback_models = ["minimax", "gpt-5.4-nano"]
            prompt_append = "Be concise."
        "#;

        let config: FlokConfig = toml::from_str(toml_str).unwrap();
        let explore = &config.agents["explore"];

        assert_eq!(explore.model.as_deref(), Some("haiku-4-5"));
        assert_eq!(explore.reasoning_effort, Some(crate::provider::ReasoningEffort::Low));
        assert_eq!(explore.fallback_models, vec!["minimax", "gpt-5.4-nano"]);
        assert_eq!(explore.prompt_append.as_deref(), Some("Be concise."));
    }

    #[test]
    fn parse_agents_config_partial() {
        let toml_str = r#"
            [agents.explore]
            model = "haiku"
            reasoning_effort = "high"

            [agents.general]
            prompt_append = "Keep output short."
        "#;

        let config: FlokConfig = toml::from_str(toml_str).unwrap();

        assert_eq!(config.agents["explore"].model.as_deref(), Some("haiku"));
        assert_eq!(
            config.agents["explore"].reasoning_effort,
            Some(crate::provider::ReasoningEffort::High)
        );
        assert!(config.agents["explore"].fallback_models.is_empty());
        assert!(config.agents["explore"].prompt_append.is_none());

        assert!(config.agents["general"].model.is_none());
        assert!(config.agents["general"].reasoning_effort.is_none());
        assert!(config.agents["general"].fallback_models.is_empty());
        assert_eq!(config.agents["general"].prompt_append.as_deref(), Some("Keep output short."));
    }

    #[test]
    fn parse_multiple_agent_blocks() {
        let toml_str = r#"
            [agents.explore]
            model = "haiku"

            [agents.feasibility-reviewer]
            prompt_append = "Prioritize operational risk."
        "#;

        let config: FlokConfig = toml::from_str(toml_str).unwrap();

        assert_eq!(config.agents.len(), 2);
        assert_eq!(config.agents["explore"].model.as_deref(), Some("haiku"));
        assert_eq!(
            config.agents["feasibility-reviewer"].prompt_append.as_deref(),
            Some("Prioritize operational risk."),
        );
    }

    #[test]
    fn parse_no_agents_section() {
        let config: FlokConfig =
            toml::from_str("[provider.anthropic]\napi_key = \"sk-test\"").unwrap();

        assert!(config.agents.is_empty());
    }

    #[test]
    fn merge_agents_overlay_preserves_unspecified_fields() {
        let mut base: FlokConfig = toml::from_str(
            r#"
            reasoning_effort = "medium"
            [agents.explore]
            model = "haiku"
            reasoning_effort = "low"
            fallback_models = ["gpt-5.4"]
            "#,
        )
        .unwrap();
        let overlay: FlokConfig = toml::from_str(
            r#"
            reasoning_effort = "high"
            [agents.explore]
            prompt_append = "Be concise."
            "#,
        )
        .unwrap();

        merge_config(&mut base, &overlay);

        assert_eq!(base.reasoning_effort, Some(crate::provider::ReasoningEffort::High));
        let explore = &base.agents["explore"];
        assert_eq!(explore.model.as_deref(), Some("haiku"));
        assert_eq!(explore.reasoning_effort, Some(crate::provider::ReasoningEffort::Low));
        assert_eq!(explore.fallback_models, vec!["gpt-5.4"]);
        assert_eq!(explore.prompt_append.as_deref(), Some("Be concise."));
    }

    #[test]
    fn parse_unknown_agent_config_does_not_error() {
        let toml_str = r#"
            [agents.nonexistent]
            model = "haiku"
        "#;

        let config: FlokConfig = toml::from_str(toml_str).unwrap();

        assert_eq!(config.agents["nonexistent"].model.as_deref(), Some("haiku"));
    }

    #[test]
    fn merge_model_overlay_overrides_base() {
        let mut base = FlokConfig { model: Some("sonnet".into()), ..Default::default() };
        let overlay = FlokConfig { model: Some("opus-4.7".into()), ..Default::default() };
        merge_config(&mut base, &overlay);
        assert_eq!(base.model.as_deref(), Some("opus-4.7"));
    }

    #[test]
    fn merge_model_unset_overlay_preserves_base() {
        let mut base = FlokConfig { model: Some("sonnet".into()), ..Default::default() };
        let overlay = FlokConfig::default();
        merge_config(&mut base, &overlay);
        assert_eq!(base.model.as_deref(), Some("sonnet"));
    }

    #[test]
    fn merge_default_model_overlay_preserves_api_key() {
        let mut base = FlokConfig::default();
        base.provider.insert(
            "anthropic".to_string(),
            ProviderConfig {
                api_key: Some(SecretString::from("sk-base".to_string())),
                base_url: None,
                default_model: None,
                fallback: Vec::new(),
            },
        );
        let overlay = FlokConfig {
            provider: [(
                "anthropic".to_string(),
                ProviderConfig {
                    api_key: None,
                    base_url: None,
                    default_model: Some("opus-4.7".into()),
                    fallback: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        merge_config(&mut base, &overlay);
        assert_eq!(
            base.provider["anthropic"].api_key.as_ref().map(SecretString::expose_secret),
            Some("sk-base"),
            "api_key from base must survive an overlay that only sets default_model",
        );
        assert_eq!(base.provider["anthropic"].default_model.as_deref(), Some("opus-4.7"),);
    }

    #[test]
    fn parse_runtime_fallback_config() {
        let toml_str = "
            [runtime_fallback]
            enabled = false
            retry_on_errors = [429, 503]
            max_attempts = 5
            cooldown_seconds = 30
            notify_on_fallback = false
        ";

        let config: FlokConfig = toml::from_str(toml_str).unwrap();
        assert!(!config.runtime_fallback.enabled);
        assert_eq!(config.runtime_fallback.retry_on_errors, vec![429, 503]);
        assert_eq!(config.runtime_fallback.max_attempts, 5);
        assert_eq!(config.runtime_fallback.cooldown_seconds, 30);
        assert!(!config.runtime_fallback.notify_on_fallback);
    }

    #[test]
    fn parse_provider_fallback_chain() {
        let toml_str = "
            [provider.anthropic]
            fallback = [\"openai\"]
        ";

        let config: FlokConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.provider["anthropic"].fallback, vec!["openai"]);
    }

    #[test]
    fn runtime_fallback_defaults_when_missing() {
        let config: FlokConfig = toml::from_str("").unwrap();
        assert_eq!(config.runtime_fallback, RuntimeFallbackConfig::default());
    }

    #[test]
    fn parse_intelligent_routing_config() {
        let toml_str = "
            [intelligent_routing]
            enabled = false
            complexity_threshold = 6
            max_session_cost_microusd = 123_456
        ";

        let config: FlokConfig = toml::from_str(toml_str).unwrap();
        assert!(!config.intelligent_routing.enabled);
        assert_eq!(config.intelligent_routing.complexity_threshold, 6);
        assert_eq!(config.intelligent_routing.max_session_cost_microusd, Some(123_456));
    }

    #[test]
    fn intelligent_routing_defaults_when_missing() {
        let config: FlokConfig = toml::from_str("").unwrap();
        assert_eq!(config.intelligent_routing, IntelligentRoutingConfig::default());
    }

    #[test]
    fn merge_output_compression_overlay() {
        let mut base: FlokConfig = toml::from_str(
            r#"
            [output_compression]
            enabled = false
            max_lines = 123
            apply_to_tools = ["bash", "grep"]
            "#,
        )
        .unwrap();
        let overlay: FlokConfig = toml::from_str(
            r"
            [output_compression]
            max_lines = 300
            head_lines = 80
            tail_lines = 80
            ",
        )
        .unwrap();

        merge_config(&mut base, &overlay);

        assert!(!base.output_compression.enabled);
        assert_eq!(base.output_compression.max_lines, 300);
        assert_eq!(base.output_compression.head_lines, 80);
        assert_eq!(base.output_compression.tail_lines, 80);
    }

    #[test]
    fn provider_fallback_defaults_to_empty_vec() {
        let toml_str = "
            [provider.openai]
            api_key = \"sk-test\"
        ";

        let config: FlokConfig = toml::from_str(toml_str).unwrap();
        assert!(config.provider["openai"].fallback.is_empty());
    }
}
