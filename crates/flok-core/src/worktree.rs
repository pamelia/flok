//! # Git Worktree Isolation
//!
//! Provides filesystem isolation for concurrent background agents using
//! git worktrees. Each background agent gets its own lightweight checkout
//! that shares the `.git` directory with the main repository.
//!
//! Worktrees are created on agent spawn and cleaned up on completion.
//! Changes are merged back into the main working tree through a serialized
//! merge lock to prevent concurrent merge conflicts.

use std::path::{Path, PathBuf};
use std::time::Instant;

use tokio::sync::Mutex;

/// Information about an active worktree.
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    /// The session ID that owns this worktree.
    pub session_id: String,
    /// Absolute path to the worktree directory.
    pub path: PathBuf,
    /// Git branch name: `flok/{session_id}`.
    pub branch: String,
    /// The commit SHA the worktree was branched from.
    pub base_commit: String,
    /// When this worktree was created.
    pub created_at: Instant,
}

/// The result of merging a worktree back into the main working tree.
#[derive(Debug)]
pub enum MergeResult {
    /// All changes merged successfully.
    Clean {
        /// Number of files that were applied.
        files_applied: usize,
    },
    /// Some files have conflicts with the main worktree.
    Conflict {
        /// Files that were applied cleanly.
        files_applied: usize,
        /// Files that had conflicts (not applied).
        conflicts: Vec<String>,
    },
    /// No changes were made in the worktree.
    NothingToMerge,
}

/// Manages git worktrees for agent isolation.
///
/// Each background agent gets its own worktree created from the current
/// HEAD. On completion, changes are merged back through a serialized lock.
pub struct WorktreeManager {
    /// The main project root (where `.git` lives).
    project_root: PathBuf,
    /// Base directory for worktrees: `$XDG_STATE_HOME/flok/worktrees/{project_id}/`.
    worktree_base: PathBuf,
    /// Serializes merge operations to prevent concurrent merges.
    merge_lock: Mutex<()>,
    /// Whether the project is a git repo (worktrees disabled otherwise).
    enabled: bool,
}

impl WorktreeManager {
    /// Create a new worktree manager.
    ///
    /// Worktrees are stored under `$XDG_STATE_HOME/flok/worktrees/{project_id}/`.
    /// Disabled if the project root does not contain a `.git` directory.
    pub fn new(project_id: &str, project_root: PathBuf) -> Self {
        let enabled = project_root.join(".git").exists();

        // Use XDG state dir or fall back to data dir
        let state_base = directories::BaseDirs::new().map_or_else(
            || project_root.join(".flok").join("worktrees"),
            |d| d.data_dir().join("flok").join("worktrees").join(project_id),
        );

        Self { project_root, worktree_base: state_base, merge_lock: Mutex::new(()), enabled }
    }

    /// Whether worktree isolation is available (project is a git repo).
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Create an isolated worktree for a background agent.
    ///
    /// The worktree is branched from `HEAD` (or the specified base commit).
    /// Returns `WorktreeInfo` with the path the agent should use as its
    /// `project_root`.
    ///
    /// # Errors
    ///
    /// Returns an error if git commands fail or the worktree cannot be created.
    pub async fn create(&self, session_id: &str) -> anyhow::Result<WorktreeInfo> {
        if !self.enabled {
            return Err(anyhow::anyhow!("worktree isolation disabled (not a git repository)"));
        }

        let branch_name = format!("flok/{session_id}");
        let worktree_path = self.worktree_base.join(session_id);

        // Ensure base directory exists
        if let Some(parent) = worktree_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Clean up stale worktree at this path if it exists
        if worktree_path.exists() {
            tracing::warn!(path = %worktree_path.display(), "cleaning stale worktree");
            self.remove_worktree_impl(session_id, &worktree_path, &branch_name).await?;
        }

        // Resolve HEAD to a commit SHA before creating the worktree
        let base_commit = resolve_head(&self.project_root).await?;

        // Create worktree with a new branch from HEAD
        let output = tokio::process::Command::new("git")
            .args(["worktree", "add", "-b", &branch_name])
            .arg(&worktree_path)
            .arg("HEAD")
            .current_dir(&self.project_root)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("failed to create worktree: {stderr}"));
        }

        tracing::info!(
            session_id,
            path = %worktree_path.display(),
            base_commit = &base_commit[..8.min(base_commit.len())],
            "created worktree"
        );

        Ok(WorktreeInfo {
            session_id: session_id.to_string(),
            path: worktree_path,
            branch: branch_name,
            base_commit,
            created_at: Instant::now(),
        })
    }

    /// Merge changes from a worktree back into the main working tree.
    ///
    /// Acquires the merge lock to prevent concurrent merges. Files that
    /// have not been modified in the main worktree since the base commit
    /// are copied directly. Files that have been modified in both places
    /// are reported as conflicts.
    ///
    /// # Errors
    ///
    /// Returns an error if git commands or file operations fail.
    pub async fn merge(&self, info: &WorktreeInfo) -> anyhow::Result<MergeResult> {
        let _lock = self.merge_lock.lock().await;

        // Get list of files changed in the worktree since its base commit
        let changed = changed_files_in_worktree(&info.path, &info.base_commit).await?;

        if changed.is_empty() {
            return Ok(MergeResult::NothingToMerge);
        }

        let mut applied = 0usize;
        let mut conflicts = Vec::new();

        for file_path in &changed {
            // Check if this file was also modified in the main worktree
            let main_changed =
                file_changed_since(&self.project_root, &info.base_commit, file_path).await?;

            if main_changed {
                // Conflict: both worktrees modified this file
                conflicts.push(file_path.clone());
            } else {
                // Safe to copy from worktree to main
                copy_file(&info.path, &self.project_root, file_path).await?;
                applied += 1;
            }
        }

        if conflicts.is_empty() {
            tracing::info!(
                session_id = %info.session_id,
                files_applied = applied,
                "worktree merge: clean"
            );
            Ok(MergeResult::Clean { files_applied: applied })
        } else {
            tracing::warn!(
                session_id = %info.session_id,
                files_applied = applied,
                conflicts = conflicts.len(),
                "worktree merge: conflicts detected"
            );
            Ok(MergeResult::Conflict { files_applied: applied, conflicts })
        }
    }

    /// Remove a worktree and its temporary branch.
    ///
    /// # Errors
    ///
    /// Returns an error if git cleanup fails.
    pub async fn remove(&self, info: &WorktreeInfo) -> anyhow::Result<()> {
        self.remove_worktree_impl(&info.session_id, &info.path, &info.branch).await
    }

    /// Clean up stale worktrees from crashed sessions.
    ///
    /// Scans `git worktree list` for branches matching `flok/*` and removes
    /// any whose directories are under our managed base path.
    ///
    /// Returns the number of stale worktrees removed.
    ///
    /// # Errors
    ///
    /// Returns an error if git commands fail.
    pub async fn cleanup_stale(&self) -> anyhow::Result<usize> {
        if !self.enabled {
            return Ok(0);
        }

        let output = tokio::process::Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(&self.project_root)
            .output()
            .await?;

        if !output.status.success() {
            return Ok(0);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut cleaned = 0usize;

        // Parse porcelain output: blocks separated by blank lines.
        // Each block has: worktree <path>, HEAD <sha>, branch refs/heads/<name>
        for block in stdout.split("\n\n") {
            let mut wt_path = None;
            let mut branch = None;

            for line in block.lines() {
                if let Some(p) = line.strip_prefix("worktree ") {
                    wt_path = Some(PathBuf::from(p));
                }
                if let Some(b) = line.strip_prefix("branch refs/heads/") {
                    branch = Some(b.to_string());
                }
            }

            // Only clean up flok-managed worktrees under our base path
            if let (Some(path), Some(br)) = (wt_path, branch) {
                if br.starts_with("flok/") && path.starts_with(&self.worktree_base) {
                    let session_id = br.strip_prefix("flok/").unwrap_or(&br);
                    tracing::info!(
                        session_id,
                        path = %path.display(),
                        "cleaning stale worktree"
                    );
                    if let Err(e) = self.remove_worktree_impl(session_id, &path, &br).await {
                        tracing::warn!(session_id, error = %e, "failed to clean stale worktree");
                    } else {
                        cleaned += 1;
                    }
                }
            }
        }

        Ok(cleaned)
    }

    /// Internal helper: remove a worktree directory and its branch.
    async fn remove_worktree_impl(
        &self,
        session_id: &str,
        worktree_path: &Path,
        branch_name: &str,
    ) -> anyhow::Result<()> {
        // Remove the worktree (force to handle uncommitted changes)
        let output = tokio::process::Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(worktree_path)
            .current_dir(&self.project_root)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // If the worktree is already gone, that's fine
            if !stderr.contains("is not a working tree") {
                tracing::warn!(
                    session_id,
                    stderr = %stderr,
                    "git worktree remove failed, attempting manual cleanup"
                );
                // Force-remove the directory as a fallback
                if worktree_path.exists() {
                    tokio::fs::remove_dir_all(worktree_path).await.ok();
                }
                // Prune stale worktree references
                let _ = tokio::process::Command::new("git")
                    .args(["worktree", "prune"])
                    .current_dir(&self.project_root)
                    .output()
                    .await;
            }
        }

        // Delete the temporary branch (force-delete since it may not be merged)
        let _ = tokio::process::Command::new("git")
            .args(["branch", "-D", branch_name])
            .current_dir(&self.project_root)
            .output()
            .await;

        tracing::debug!(session_id, "worktree removed");
        Ok(())
    }
}

impl std::fmt::Debug for WorktreeManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorktreeManager")
            .field("project_root", &self.project_root)
            .field("worktree_base", &self.worktree_base)
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Git helpers
// ---------------------------------------------------------------------------

/// Resolve HEAD to a commit SHA.
async fn resolve_head(project_root: &Path) -> anyhow::Result<String> {
    let output = tokio::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(project_root)
        .output()
        .await?;

    if !output.status.success() {
        return Err(anyhow::anyhow!("failed to resolve HEAD"));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// List files changed in a worktree since the base commit.
///
/// Returns relative file paths.
async fn changed_files_in_worktree(
    worktree_path: &Path,
    base_commit: &str,
) -> anyhow::Result<Vec<String>> {
    // Include both committed changes and uncommitted modifications
    let output = tokio::process::Command::new("git")
        .args(["diff", "--name-only", base_commit])
        .current_dir(worktree_path)
        .output()
        .await?;

    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect())
}

/// Check if a specific file was modified in the main worktree since a commit.
async fn file_changed_since(
    project_root: &Path,
    base_commit: &str,
    file_path: &str,
) -> anyhow::Result<bool> {
    let output = tokio::process::Command::new("git")
        .args(["diff", "--name-only", base_commit, "--", file_path])
        .current_dir(project_root)
        .output()
        .await?;

    // If the file appears in the diff output, it was changed
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().any(|l| !l.is_empty()))
}

/// Copy a file from the worktree to the main working tree.
///
/// Creates parent directories as needed. Handles file deletion (if the
/// file exists in worktree but was deleted, remove from main).
async fn copy_file(
    worktree_path: &Path,
    project_root: &Path,
    relative_path: &str,
) -> anyhow::Result<()> {
    let src = worktree_path.join(relative_path);
    let dst = project_root.join(relative_path);

    if src.exists() {
        // Copy file from worktree to main
        if let Some(parent) = dst.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::copy(&src, &dst).await?;
    } else {
        // File was deleted in worktree — remove from main
        if dst.exists() {
            tokio::fs::remove_file(&dst).await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn worktree_disabled_for_non_git() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = WorktreeManager::new("test-project", dir.path().to_path_buf());
        assert!(!mgr.is_enabled());
    }

    #[tokio::test]
    async fn worktree_enabled_for_git_repo() {
        let dir = tempfile::tempdir().unwrap();
        init_test_repo(dir.path()).await;

        let mgr = WorktreeManager::new("test-project", dir.path().to_path_buf());
        assert!(mgr.is_enabled());
    }

    #[tokio::test]
    async fn create_and_remove_worktree() {
        let dir = tempfile::tempdir().unwrap();
        // Set up a git repo with one commit
        init_test_repo(dir.path()).await;

        let mgr = WorktreeManager::new("test-project", dir.path().to_path_buf());
        assert!(mgr.is_enabled());

        // Create a worktree
        let info = mgr.create("test-session-1").await.unwrap();
        assert!(info.path.exists());
        assert_eq!(info.branch, "flok/test-session-1");
        assert!(!info.base_commit.is_empty());

        // The worktree should have the same files as the main repo
        assert!(info.path.join("README.md").exists());

        // Remove the worktree
        mgr.remove(&info).await.unwrap();
        assert!(!info.path.exists());
    }

    #[tokio::test]
    async fn merge_clean_changes() {
        let dir = tempfile::tempdir().unwrap();
        init_test_repo(dir.path()).await;

        let mgr = WorktreeManager::new("test-project", dir.path().to_path_buf());
        let info = mgr.create("merge-test").await.unwrap();

        // Make a change in the worktree
        tokio::fs::write(info.path.join("new_file.txt"), "hello from agent").await.unwrap();

        // Stage and commit in the worktree so git diff picks it up
        let _ = tokio::process::Command::new("git")
            .args(["add", "new_file.txt"])
            .current_dir(&info.path)
            .output()
            .await;
        let _ = tokio::process::Command::new("git")
            .args(["commit", "-m", "agent work", "--no-gpg-sign"])
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .current_dir(&info.path)
            .output()
            .await;

        // Merge back to main
        let result = mgr.merge(&info).await.unwrap();
        assert!(matches!(result, MergeResult::Clean { files_applied: 1 }));

        // The file should now exist in the main repo
        assert!(dir.path().join("new_file.txt").exists());
        let content = tokio::fs::read_to_string(dir.path().join("new_file.txt")).await.unwrap();
        assert_eq!(content, "hello from agent");

        // Cleanup
        mgr.remove(&info).await.unwrap();
    }

    #[tokio::test]
    async fn merge_nothing_to_merge() {
        let dir = tempfile::tempdir().unwrap();
        init_test_repo(dir.path()).await;

        let mgr = WorktreeManager::new("test-project", dir.path().to_path_buf());
        let info = mgr.create("empty-test").await.unwrap();

        // Don't change anything
        let result = mgr.merge(&info).await.unwrap();
        assert!(matches!(result, MergeResult::NothingToMerge));

        mgr.remove(&info).await.unwrap();
    }

    #[tokio::test]
    async fn merge_detects_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        init_test_repo(dir.path()).await;

        let mgr = WorktreeManager::new("test-project", dir.path().to_path_buf());
        let info = mgr.create("conflict-test").await.unwrap();

        // Modify README.md in the worktree
        tokio::fs::write(info.path.join("README.md"), "# modified by agent").await.unwrap();
        let _ = tokio::process::Command::new("git")
            .args(["add", "README.md"])
            .current_dir(&info.path)
            .output()
            .await;
        let _ = tokio::process::Command::new("git")
            .args(["commit", "-m", "agent edit", "--no-gpg-sign"])
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .current_dir(&info.path)
            .output()
            .await;

        // Also modify README.md in the main repo
        tokio::fs::write(dir.path().join("README.md"), "# modified by user").await.unwrap();
        let _ = tokio::process::Command::new("git")
            .args(["add", "README.md"])
            .current_dir(dir.path())
            .output()
            .await;
        let _ = tokio::process::Command::new("git")
            .args(["commit", "-m", "user edit", "--no-gpg-sign"])
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .current_dir(dir.path())
            .output()
            .await;

        // Merge — should detect conflict
        let result = mgr.merge(&info).await.unwrap();
        match result {
            MergeResult::Conflict { conflicts, .. } => {
                assert!(conflicts.contains(&"README.md".to_string()));
            }
            other => panic!("expected conflict, got {other:?}"),
        }

        mgr.remove(&info).await.unwrap();
    }

    #[tokio::test]
    async fn cleanup_stale_finds_nothing_in_clean_repo() {
        let dir = tempfile::tempdir().unwrap();
        init_test_repo(dir.path()).await;

        let mgr = WorktreeManager::new("test-project", dir.path().to_path_buf());
        let cleaned = mgr.cleanup_stale().await.unwrap();
        assert_eq!(cleaned, 0);
    }

    /// Helper: run a git command in a test directory.
    async fn git(path: &Path, args: &[&str]) {
        let output = tokio::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .await
            .expect("git command failed");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Helper: initialize a test git repository with one commit.
    async fn init_test_repo(path: &Path) {
        git(path, &["init"]).await;
        git(path, &["checkout", "-b", "main"]).await;
        tokio::fs::write(path.join("README.md"), "# test repo").await.unwrap();
        git(path, &["add", "."]).await;
        git(path, &["commit", "-m", "initial commit", "--no-gpg-sign"]).await;
    }
}
