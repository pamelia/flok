//! # Configuration
//!
//! Configuration loading and hot-reload for flok. Configuration is loaded
//! from TOML files with the following precedence (highest to lowest):
//!
//! 1. Environment variables (`FLOK_*`)
//! 2. Project config (`flok.toml` in project root)
//! 3. Global config (`~/.config/flok/flok.toml`)

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Top-level flok configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FlokConfig {
    /// Provider configurations keyed by provider name.
    pub provider: std::collections::HashMap<String, ProviderConfig>,
    /// Git worktree isolation settings.
    pub worktree: WorktreeConfig,
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
    /// API key (can also come from environment).
    pub api_key: Option<String>,
    /// Base URL override.
    pub base_url: Option<String>,
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
    // Worktree config: source overrides target entirely if present in source file
    // (serde default handles missing fields; if explicitly set in source, override)
    target.worktree = source.worktree.clone();
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

    #[test]
    fn default_config_is_valid() {
        let config = FlokConfig::default();
        assert!(config.provider.is_empty());
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
                ProviderConfig { api_key: Some("key-123".to_string()), base_url: None },
            )]
            .into_iter()
            .collect(),
            worktree: WorktreeConfig::default(),
        };
        merge_config(&mut base, &overlay);
        assert_eq!(base.provider["anthropic"].api_key.as_deref(), Some("key-123"));
    }

    #[test]
    fn parse_minimal_toml() {
        let toml_str = r#"
            [provider.anthropic]
            api_key = "sk-test-123"
        "#;
        let config: FlokConfig = toml::from_str(toml_str).unwrap();
        assert!(config.provider.contains_key("anthropic"));
        assert_eq!(config.provider["anthropic"].api_key.as_deref(), Some("sk-test-123"));
    }
}
