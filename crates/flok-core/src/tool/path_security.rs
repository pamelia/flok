use std::path::{Component, Path, PathBuf};

fn normalize_absolute_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }

    normalized
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProtectedWriteTarget {
    ProjectConfig,
    AgentMemory,
    PlanFile,
    DotFlokInternal,
}

impl ProtectedWriteTarget {
    fn error_message(self, path: &Path) -> String {
        match self {
            Self::ProjectConfig => format!(
                "Writes to flok config are blocked; update the config intentionally outside write/edit tools: {}",
                path.display()
            ),
            Self::AgentMemory => format!(
                "Writes to agent memory are blocked here; use the `agent_memory` tool instead: {}",
                path.display()
            ),
            Self::PlanFile => format!(
                "Writes to plan files are blocked here; use the plan tools instead: {}",
                path.display()
            ),
            Self::DotFlokInternal => format!(
                "Writes to .flok internals are blocked; use the dedicated tool for that path: {}",
                path.display()
            ),
        }
    }
}

fn protected_internal_target(path: &Path, project_root: &Path) -> Option<ProtectedWriteTarget> {
    if path == project_root.join("flok.toml") {
        return Some(ProtectedWriteTarget::ProjectConfig);
    }

    let dotflok = project_root.join(".flok");
    if path.starts_with(dotflok.join("memory")) {
        return Some(ProtectedWriteTarget::AgentMemory);
    }
    if path == dotflok.join("plan.md") || path.starts_with(dotflok.join("plans")) {
        return Some(ProtectedWriteTarget::PlanFile);
    }
    if path.starts_with(&dotflok) {
        return Some(ProtectedWriteTarget::DotFlokInternal);
    }

    None
}

fn ensure_existing_ancestor_within_project(
    requested: &Path,
    project_root: &Path,
) -> anyhow::Result<()> {
    let mut current = requested.parent();
    while let Some(path) = current {
        if path.exists() {
            let canonical = std::fs::canonicalize(path)?;
            if !canonical.starts_with(project_root) {
                anyhow::bail!("Path escapes project root via symlink: {}", requested.display());
            }
            return Ok(());
        }
        current = path.parent();
    }

    Ok(())
}

pub(crate) fn resolve_write_path(project_root: &Path, file_path: &str) -> anyhow::Result<PathBuf> {
    let project_root =
        std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let requested = if Path::new(file_path).is_absolute() {
        PathBuf::from(file_path)
    } else {
        project_root.join(file_path)
    };
    let normalized = normalize_absolute_path(&requested);

    if !normalized.starts_with(&project_root) {
        anyhow::bail!("Path escapes project root: {}", requested.display());
    }

    if let Some(target) = protected_internal_target(&normalized, &project_root) {
        anyhow::bail!(target.error_message(&normalized));
    }

    if let Ok(canonical) = std::fs::canonicalize(&requested) {
        if !canonical.starts_with(&project_root) {
            anyhow::bail!("Path escapes project root via symlink: {}", requested.display());
        }
        if let Some(target) = protected_internal_target(&canonical, &project_root) {
            anyhow::bail!(target.error_message(&canonical));
        }
        return Ok(canonical);
    }

    ensure_existing_ancestor_within_project(&requested, &project_root)?;
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_write_path_allows_in_project_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = resolve_write_path(dir.path(), "src/main.rs").expect("path");
        let canonical_root =
            std::fs::canonicalize(dir.path()).unwrap_or_else(|_| dir.path().to_path_buf());
        assert_eq!(path, canonical_root.join("src/main.rs"));
    }

    #[test]
    fn resolve_write_path_blocks_parent_traversal() {
        let dir = tempfile::tempdir().expect("temp dir");
        let error = resolve_write_path(dir.path(), "../escape.txt").expect_err("expected error");
        assert!(error.to_string().contains("escapes project root"));
    }

    #[test]
    fn resolve_write_path_blocks_dotflok_internal_paths() {
        let dir = tempfile::tempdir().expect("temp dir");
        let error =
            resolve_write_path(dir.path(), ".flok/internal.txt").expect_err("expected error");
        assert!(error.to_string().contains(".flok internals"));
    }

    #[test]
    fn resolve_write_path_blocks_project_config() {
        let dir = tempfile::tempdir().expect("temp dir");
        let error = resolve_write_path(dir.path(), "flok.toml").expect_err("expected error");
        assert!(error.to_string().contains("flok config"));
    }

    #[test]
    fn resolve_write_path_blocks_agent_memory_with_tool_hint() {
        let dir = tempfile::tempdir().expect("temp dir");
        let error =
            resolve_write_path(dir.path(), ".flok/memory/default.md").expect_err("expected error");
        assert!(error.to_string().contains("agent_memory"));
    }

    #[test]
    fn resolve_write_path_blocks_plan_file_with_tool_hint() {
        let dir = tempfile::tempdir().expect("temp dir");
        let error = resolve_write_path(dir.path(), ".flok/plan.md").expect_err("expected error");
        assert!(error.to_string().contains("plan tools"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_write_path_blocks_symlink_escape() {
        use std::os::unix::fs::symlink;

        let project = tempfile::tempdir().expect("project temp dir");
        let outside = tempfile::tempdir().expect("outside temp dir");
        symlink(outside.path(), project.path().join("linked")).expect("symlink");

        let error = resolve_write_path(project.path(), "linked/escape.txt").expect_err("error");
        assert!(error.to_string().contains("symlink"));
    }
}
