//! # Configuration
//!
//! Configuration loading and hot-reload for flok. Configuration is loaded
//! from TOML files with the following precedence (highest to lowest):
//!
//! 1. Environment variables (`FLOK_*`)
//! 2. Project config (`flok.toml` in project root)
//! 3. Global config (`~/.config/flok/flok.toml`)

use std::path::{Path, PathBuf};

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize, Serializer};

/// Top-level flok configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FlokConfig {
    /// Provider configurations keyed by provider name.
    pub provider: std::collections::HashMap<String, ProviderConfig>,
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
    pub permission: std::collections::HashMap<String, PermissionToolConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LspConfig {
    pub enabled: bool,
    pub request_timeout_ms: u64,
    pub rust: RustLspConfig,
}

impl Default for LspConfig {
    fn default() -> Self {
        Self { enabled: true, request_timeout_ms: 5_000, rust: RustLspConfig::default() }
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
    config: &std::collections::HashMap<String, PermissionToolConfig, S>,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// API key. Read ONLY from the config file at runtime. Wrapped in
    /// `SecretString` for zeroize-on-drop and redacted `Debug`.
    #[serde(default, serialize_with = "serialize_api_key_opt")]
    pub api_key: Option<SecretString>,
    /// Base URL override.
    pub base_url: Option<String>,
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
/// Checks for: `.git`, `Cargo.toml`, `package.json`, `go.mod`, `pyproject.toml`,
/// `.flok`, `flok.toml`. Returns the first directory containing any marker,
/// or `start_dir` if none found.
pub fn detect_project_root(start_dir: &Path) -> PathBuf {
    const MARKERS: &[&str] =
        &[".git", "Cargo.toml", "package.json", "go.mod", "pyproject.toml", ".flok", "flok.toml"];

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

/// Merge `source` config into `target`. Source values override target values.
fn merge_config(target: &mut FlokConfig, source: &FlokConfig) {
    for (key, value) in &source.provider {
        target.provider.insert(key.clone(), value.clone());
    }
    target.lsp = source.lsp.clone();
    // Worktree config: source overrides target entirely if present in source file
    // (serde default handles missing fields; if explicitly set in source, override)
    target.worktree = source.worktree.clone();
    // Permission config: merge at the permission-type level
    for (key, value) in &source.permission {
        target.permission.insert(key.clone(), value.clone());
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
        ];
        for path in &paths {
            std::fs::create_dir_all(path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::{ExposeSecret, SecretString};

    #[test]
    fn default_config_is_valid() {
        let config = FlokConfig::default();
        assert!(config.provider.is_empty());
        assert!(config.lsp.enabled);
        assert_eq!(config.lsp.rust.command, "rust-analyzer");
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
        "#;

        let config: FlokConfig = toml::from_str(toml_str).unwrap();
        assert!(config.lsp.enabled);
        assert_eq!(config.lsp.request_timeout_ms, 1200);
        assert_eq!(config.lsp.rust.command, "custom-ra");
        assert_eq!(config.lsp.rust.args, vec!["--stdio"]);
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
                },
            )]
            .into_iter()
            .collect(),
            lsp: LspConfig::default(),
            worktree: WorktreeConfig::default(),
            permission: std::collections::HashMap::new(),
        };
        merge_config(&mut base, &overlay);
        assert_eq!(
            base.provider["anthropic"].api_key.as_ref().map(SecretString::expose_secret),
            Some("key-123"),
        );
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
}
