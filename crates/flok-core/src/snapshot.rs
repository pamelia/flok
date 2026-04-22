//! # Snapshot System
//!
//! A shadow Git repository that tracks workspace state independently of the
//! user's real Git repo. Snapshots are lightweight tree objects (not commits)
//! that capture the full working tree at a point in time.
//!
//! Based on the design from `OpenCode`: each snapshot is a `git write-tree`
//! hash. Restoration uses `git read-tree` + `git checkout-index`. Revert
//! uses per-file `git checkout <hash> -- <file>` with proper handling of
//! files that were created after the snapshot (they get deleted on revert).
//!
//! ## Shadow repo location
//!
//! ```text
//! <data_dir>/flok/snapshot/<project_id>/<sha1(worktree)>/
//! ```
//!
//! The shadow repo uses `--git-dir` and `--work-tree` flags so it never
//! places a `.git` directory inside the user's workspace.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tokio::sync::Mutex;

/// Maximum file size to include in snapshots (2 MB).
const LARGE_FILE_LIMIT: u64 = 2 * 1024 * 1024;

/// A patch recording which files changed relative to a snapshot.
#[derive(Debug, Clone)]
pub struct Patch {
    /// The tree hash this patch is relative to.
    pub hash: String,
    /// Absolute paths of files that changed.
    pub files: Vec<String>,
}

/// A full file diff with before/after content.
#[derive(Debug, Clone)]
pub struct FileDiff {
    /// Relative file path.
    pub file: String,
    /// Content before.
    pub before: String,
    /// Content after.
    pub after: String,
    /// Lines added.
    pub additions: u64,
    /// Lines deleted.
    pub deletions: u64,
    /// Change status.
    pub status: DiffStatus,
}

/// Status of a file in a diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffStatus {
    Added,
    Deleted,
    Modified,
}

/// Result of a git command execution.
struct GitResult {
    code: i32,
    stdout: String,
    stderr: String,
}

/// The snapshot manager for a single project workspace.
///
/// Internally holds the path to the shadow git directory and the worktree,
/// plus a mutex to serialize git operations.
pub struct SnapshotManager {
    /// Path to the shadow `.git` directory.
    gitdir: PathBuf,
    /// Path to the user's workspace (the worktree).
    worktree: PathBuf,
    /// Whether the project uses git (snapshots are disabled for non-git projects).
    enabled: bool,
    /// Mutex to serialize all git operations on this shadow repo.
    lock: Mutex<()>,
}

impl SnapshotManager {
    /// Create a new snapshot manager for a project.
    ///
    /// `project_id` is the database project ID.
    /// `worktree` is the absolute path to the workspace root.
    ///
    /// The shadow git repo will be created at:
    /// `<data_dir>/flok/snapshot/<project_id>/<hash(worktree)>/`
    pub fn new(project_id: &str, worktree: PathBuf) -> Self {
        let worktree_hash = hash_path(&worktree);
        let fallback_base = worktree.join(".flok").join("snapshot").join(project_id);
        let snapshot_base = if cfg!(test) {
            let _ = std::fs::create_dir_all(&fallback_base);
            fallback_base
        } else {
            select_writable_storage_dir(
                directories::BaseDirs::new()
                    .map(|dirs| dirs.data_dir().join("flok").join("snapshot").join(project_id)),
                fallback_base,
            )
        };
        let gitdir = snapshot_base.join(worktree_hash);

        // Only enable if the worktree has a .git directory (is a git project)
        let enabled = worktree.join(".git").exists();

        Self { gitdir, worktree, enabled, lock: Mutex::new(()) }
    }

    /// Whether snapshots are enabled for this project.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Take a snapshot of the current workspace state.
    ///
    /// Returns the tree hash, or `None` if snapshots are disabled.
    ///
    /// # Errors
    ///
    /// Returns an error if git operations fail.
    pub async fn track(&self) -> anyhow::Result<Option<String>> {
        if !self.enabled {
            return Ok(None);
        }
        let _guard = self.lock.lock().await;

        // Lazy-initialize the shadow repo if it doesn't exist
        if !self.gitdir.exists() {
            self.init_shadow_repo().await?;
        }

        // Stage all changes
        self.add_files().await?;

        // Create a tree object from the current index
        let result = self.git(&["write-tree"]).await?;
        if result.code != 0 {
            tracing::warn!(
                exitcode = result.code,
                stderr = %result.stderr,
                "snapshot write-tree failed"
            );
            return Ok(None);
        }

        let hash = result.stdout.trim().to_string();
        tracing::debug!(hash = %hash, "snapshot tracked");
        Ok(Some(hash))
    }

    /// Get the list of files that changed since a snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error if git operations fail.
    pub async fn patch(&self, hash: &str) -> anyhow::Result<Patch> {
        if !self.enabled {
            return Ok(Patch { hash: hash.to_string(), files: vec![] });
        }
        let _guard = self.lock.lock().await;

        self.add_files().await?;

        let result = self
            .git_quoted(&["diff", "--cached", "--no-ext-diff", "--name-only", hash, "--", "."])
            .await?;

        if result.code != 0 {
            tracing::warn!(hash = %hash, exitcode = result.code, "snapshot diff failed");
            return Ok(Patch { hash: hash.to_string(), files: vec![] });
        }

        let files = result
            .stdout
            .trim()
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|rel| self.worktree.join(rel).to_string_lossy().replace('\\', "/"))
            .collect();

        Ok(Patch { hash: hash.to_string(), files })
    }

    /// Restore the workspace to a snapshot state.
    ///
    /// Uses `git read-tree` + `git checkout-index` to restore files.
    /// Note: this does NOT delete files that were created after the snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error if git operations fail.
    pub async fn restore(&self, snapshot: &str) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let _guard = self.lock.lock().await;

        tracing::info!(snapshot = %snapshot, "restoring snapshot");

        let result = self.git_core(&["read-tree", snapshot]).await?;
        if result.code != 0 {
            anyhow::bail!("failed to read-tree snapshot {snapshot}: {}", result.stderr);
        }

        let checkout = self.git_core(&["checkout-index", "-a", "-f"]).await?;
        if checkout.code != 0 {
            anyhow::bail!("failed to checkout-index snapshot {snapshot}: {}", checkout.stderr);
        }

        Ok(())
    }

    /// Revert specific files to their state in given snapshots.
    ///
    /// For each patch, checks out the files from the snapshot's tree hash.
    /// If a file didn't exist at the snapshot time, it is deleted.
    /// Files appearing in multiple patches use the first patch's hash (earliest snapshot).
    ///
    /// # Errors
    ///
    /// Returns an error if git operations fail critically.
    pub async fn revert(&self, patches: &[Patch]) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let _guard = self.lock.lock().await;

        let mut seen = HashSet::new();
        for patch in patches {
            for file in &patch.files {
                if !seen.insert(file.clone()) {
                    continue;
                }
                tracing::debug!(file = %file, hash = %patch.hash, "reverting file");

                let result = self.git_core(&["checkout", &patch.hash, "--", file]).await?;

                if result.code != 0 {
                    // Check if the file existed in the snapshot
                    let rel = pathdiff_relative(&self.worktree, Path::new(file));
                    let tree = self.git_core(&["ls-tree", &patch.hash, "--", &rel]).await?;

                    if tree.code == 0 && !tree.stdout.trim().is_empty() {
                        tracing::info!(
                            file = %file,
                            "file existed in snapshot but checkout failed, keeping"
                        );
                    } else {
                        tracing::info!(file = %file, "file did not exist in snapshot, deleting");
                        let _ = tokio::fs::remove_file(file).await;
                    }
                }
            }
        }

        Ok(())
    }

    /// Get a unified diff string between the current state and a snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error if git operations fail.
    pub async fn diff(&self, hash: &str) -> anyhow::Result<String> {
        if !self.enabled {
            return Ok(String::new());
        }
        let _guard = self.lock.lock().await;

        self.add_files().await?;

        let result =
            self.git_quoted(&["diff", "--cached", "--no-ext-diff", hash, "--", "."]).await?;

        if result.code != 0 {
            tracing::warn!(hash = %hash, exitcode = result.code, "snapshot diff failed");
            return Ok(String::new());
        }

        Ok(result.stdout.trim().to_string())
    }

    /// Get structured diffs with before/after content between two snapshots.
    ///
    /// # Errors
    ///
    /// Returns an error if git operations fail.
    pub async fn diff_full(&self, from: &str, to: &str) -> anyhow::Result<Vec<FileDiff>> {
        if !self.enabled {
            return Ok(vec![]);
        }
        let _guard = self.lock.lock().await;

        // Get file statuses
        let statuses = self
            .git_quoted(&[
                "diff",
                "--no-ext-diff",
                "--name-status",
                "--no-renames",
                from,
                to,
                "--",
                ".",
            ])
            .await?;

        let mut status_map = std::collections::HashMap::new();
        for line in statuses.stdout.trim().lines() {
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split('\t');
            if let (Some(code), Some(file)) = (parts.next(), parts.next()) {
                let status = if code.starts_with('A') {
                    DiffStatus::Added
                } else if code.starts_with('D') {
                    DiffStatus::Deleted
                } else {
                    DiffStatus::Modified
                };
                status_map.insert(file.to_string(), status);
            }
        }

        // Get numstat for additions/deletions
        let numstat = self
            .git_quoted(&[
                "diff",
                "--no-ext-diff",
                "--no-renames",
                "--numstat",
                from,
                to,
                "--",
                ".",
            ])
            .await?;

        let mut result = Vec::new();
        for line in numstat.stdout.trim().lines() {
            if line.is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 3 {
                continue;
            }
            let (adds, dels, file) = (parts[0], parts[1], parts[2]);
            let binary = adds == "-" && dels == "-";

            let (before, after) = if binary {
                (String::new(), String::new())
            } else {
                let before_result = self.git_cfg(&["show", &format!("{from}:{file}")]).await?;
                let after_result = self.git_cfg(&["show", &format!("{to}:{file}")]).await?;
                (before_result.stdout, after_result.stdout)
            };

            let additions = if binary { 0 } else { adds.parse().unwrap_or(0) };
            let deletions = if binary { 0 } else { dels.parse().unwrap_or(0) };

            result.push(FileDiff {
                file: file.to_string(),
                before,
                after,
                additions,
                deletions,
                status: status_map.get(file).copied().unwrap_or(DiffStatus::Modified),
            });
        }

        Ok(result)
    }

    /// Run garbage collection on the shadow repo.
    ///
    /// Prunes objects older than 7 days.
    ///
    /// # Errors
    ///
    /// Returns an error if git gc fails.
    pub async fn cleanup(&self) -> anyhow::Result<()> {
        if !self.enabled || !self.gitdir.exists() {
            return Ok(());
        }
        let _guard = self.lock.lock().await;

        let result = self.git(&["gc", "--prune=7.days"]).await?;
        if result.code != 0 {
            tracing::warn!(
                exitcode = result.code,
                stderr = %result.stderr,
                "snapshot gc failed"
            );
        } else {
            tracing::info!("snapshot gc completed");
        }

        Ok(())
    }

    // ---- internal helpers ----

    /// Initialize the shadow git repository.
    async fn init_shadow_repo(&self) -> anyhow::Result<()> {
        tokio::fs::create_dir_all(&self.gitdir).await?;

        // git init with GIT_DIR and GIT_WORK_TREE
        let output = tokio::process::Command::new("git")
            .arg("init")
            .env("GIT_DIR", &self.gitdir)
            .env("GIT_WORK_TREE", &self.worktree)
            .output()
            .await?;

        if !output.status.success() {
            anyhow::bail!("git init failed: {}", String::from_utf8_lossy(&output.stderr));
        }

        // Configure the shadow repo
        for (key, value) in [
            ("core.autocrlf", "false"),
            ("core.longpaths", "true"),
            ("core.symlinks", "true"),
            ("core.fsmonitor", "false"),
        ] {
            self.git(&["config", key, value]).await?;
        }

        // Sync excludes from the real repo
        self.sync_excludes(&[]).await?;

        tracing::info!(gitdir = %self.gitdir.display(), "shadow repo initialized");
        Ok(())
    }

    /// Sync exclude rules from the real repo's `.git/info/exclude` into the
    /// shadow repo, plus any dynamically discovered large file exclusions.
    async fn sync_excludes(&self, large_files: &[String]) -> anyhow::Result<()> {
        // Read the real repo's exclude file
        let real_exclude = self.worktree.join(".git").join("info").join("exclude");
        let base_excludes = if real_exclude.exists() {
            tokio::fs::read_to_string(&real_exclude).await.unwrap_or_default()
        } else {
            String::new()
        };

        // Build the combined exclude content
        let mut lines: Vec<String> = vec![];
        let trimmed = base_excludes.trim_end();
        if !trimmed.is_empty() {
            lines.push(trimmed.to_string());
        }
        lines.push("/.flok/".to_string());
        for file in large_files {
            lines.push(format!("/{}", file.replace('\\', "/")));
        }

        let content =
            if lines.is_empty() { String::new() } else { format!("{}\n", lines.join("\n")) };

        // Write to shadow repo's info/exclude
        let exclude_dir = self.gitdir.join("info");
        tokio::fs::create_dir_all(&exclude_dir).await?;
        tokio::fs::write(exclude_dir.join("exclude"), content).await?;

        Ok(())
    }

    /// Stage all changed and untracked files into the shadow repo index.
    ///
    /// Also discovers large files (>2MB) and adds them to the excludes.
    async fn add_files(&self) -> anyhow::Result<()> {
        // First sync with empty large file list
        self.sync_excludes(&[]).await?;

        // Find tracked files that changed and untracked files
        let diff = self.git_quoted(&["diff-files", "--name-only", "-z", "--", "."]).await?;
        let other = self
            .git_quoted(&["ls-files", "--others", "--exclude-standard", "-z", "--", "."])
            .await?;

        if diff.code != 0 || other.code != 0 {
            tracing::warn!(
                diff_code = diff.code,
                diff_stderr = %diff.stderr,
                other_code = other.code,
                other_stderr = %other.stderr,
                "failed to list snapshot files"
            );
            return Ok(());
        }

        let tracked: Vec<&str> = diff.stdout.split('\0').filter(|s| !s.is_empty()).collect();
        let untracked: Vec<&str> = other.stdout.split('\0').filter(|s| !s.is_empty()).collect();
        let mut all: Vec<String> = Vec::with_capacity(tracked.len() + untracked.len());
        let mut seen = HashSet::new();
        for item in tracked.into_iter().chain(untracked) {
            if seen.insert(item.to_string()) {
                all.push(item.to_string());
            }
        }

        if all.is_empty() {
            return Ok(());
        }

        // Find large files and exclude them
        let mut large = Vec::new();
        for item in &all {
            let full = self.worktree.join(item);
            if let Ok(meta) = tokio::fs::metadata(&full).await {
                if meta.is_file() && meta.len() > LARGE_FILE_LIMIT {
                    large.push(item.clone());
                }
            }
        }

        if !large.is_empty() {
            self.sync_excludes(&large).await?;
        }

        // Stage everything
        let result = self.git_cfg(&["add", "--sparse", "."]).await?;
        if result.code != 0 {
            tracing::warn!(
                exitcode = result.code,
                stderr = %result.stderr,
                "failed to add snapshot files"
            );
        }

        Ok(())
    }

    /// Run a git command with `--git-dir` and `--work-tree` flags.
    async fn git(&self, args: &[&str]) -> anyhow::Result<GitResult> {
        let output = tokio::process::Command::new("git")
            .arg("--git-dir")
            .arg(&self.gitdir)
            .arg("--work-tree")
            .arg(&self.worktree)
            .args(args)
            .current_dir(&self.worktree)
            .output()
            .await?;

        Ok(GitResult {
            code: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    /// Run git with `--git-dir`, `--work-tree`, and core config overrides.
    /// Equivalent to `OpenCode`'s `cfg` args: `-c core.autocrlf=false -c core.longpaths=true -c core.symlinks=true`
    async fn git_cfg(&self, args: &[&str]) -> anyhow::Result<GitResult> {
        let output = tokio::process::Command::new("git")
            .args([
                "-c",
                "core.autocrlf=false",
                "-c",
                "core.longpaths=true",
                "-c",
                "core.symlinks=true",
            ])
            .arg("--git-dir")
            .arg(&self.gitdir)
            .arg("--work-tree")
            .arg(&self.worktree)
            .args(args)
            .current_dir(&self.worktree)
            .output()
            .await?;

        Ok(GitResult {
            code: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    /// Run git with core config + `core.quotepath=false` (for file listing commands).
    async fn git_quoted(&self, args: &[&str]) -> anyhow::Result<GitResult> {
        let output = tokio::process::Command::new("git")
            .args([
                "-c",
                "core.autocrlf=false",
                "-c",
                "core.longpaths=true",
                "-c",
                "core.symlinks=true",
                "-c",
                "core.quotepath=false",
            ])
            .arg("--git-dir")
            .arg(&self.gitdir)
            .arg("--work-tree")
            .arg(&self.worktree)
            .args(args)
            .current_dir(&self.worktree)
            .output()
            .await?;

        Ok(GitResult {
            code: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    /// Run git with only core config (no `--work-tree` override — relies on
    /// the repo's configured worktree). Used for commands like `read-tree`
    /// and `checkout-index` that operate on the index.
    async fn git_core(&self, args: &[&str]) -> anyhow::Result<GitResult> {
        let output = tokio::process::Command::new("git")
            .args(["-c", "core.longpaths=true", "-c", "core.symlinks=true"])
            .arg("--git-dir")
            .arg(&self.gitdir)
            .arg("--work-tree")
            .arg(&self.worktree)
            .args(args)
            .current_dir(&self.worktree)
            .output()
            .await?;

        Ok(GitResult {
            code: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
}

fn select_writable_storage_dir(primary: Option<PathBuf>, fallback: PathBuf) -> PathBuf {
    if let Some(primary) = primary {
        if std::fs::create_dir_all(&primary).is_ok() {
            return primary;
        }
        tracing::warn!(
            path = %primary.display(),
            fallback = %fallback.display(),
            "snapshot storage directory unavailable, using project-local fallback"
        );
    }

    let _ = std::fs::create_dir_all(&fallback);
    fallback
}

impl std::fmt::Debug for SnapshotManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnapshotManager")
            .field("gitdir", &self.gitdir)
            .field("worktree", &self.worktree)
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

/// Hash a path to a short, filesystem-safe hex string.
///
/// Uses a simple djb2-style hash producing a 16-character hex string.
fn hash_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    let mut hash: u64 = 5381;
    for byte in s.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u64::from(byte));
    }
    format!("{hash:016x}")
}

/// Compute a relative path from `base` to `target`.
///
/// Simple implementation: strips the base prefix if present, otherwise
/// returns the target's file name.
fn pathdiff_relative(base: &Path, target: &Path) -> String {
    match target.strip_prefix(base) {
        Ok(p) => p.to_string_lossy().replace('\\', "/"),
        Err(_) => target.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_path_deterministic() {
        let p = Path::new("/home/user/project");
        let h1 = hash_path(p);
        let h2 = hash_path(p);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 16, "hash should be 16 hex chars");
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_path_different_for_different_paths() {
        let h1 = hash_path(Path::new("/a"));
        let h2 = hash_path(Path::new("/b"));
        assert_ne!(h1, h2);
    }

    #[test]
    fn pathdiff_strips_prefix() {
        let base = Path::new("/home/user/project");
        let target = Path::new("/home/user/project/src/main.rs");
        assert_eq!(pathdiff_relative(base, target), "src/main.rs");
    }

    #[test]
    fn pathdiff_fallback_to_filename() {
        let base = Path::new("/home/user/project");
        let target = Path::new("/other/path/file.rs");
        assert_eq!(pathdiff_relative(base, target), "file.rs");
    }

    #[tokio::test]
    async fn track_disabled_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        // No .git directory — snapshots disabled
        let mgr = SnapshotManager::new("test-project", dir.path().to_path_buf());
        assert!(!mgr.is_enabled());
        let result = mgr.track().await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn patch_disabled_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SnapshotManager::new("test-project", dir.path().to_path_buf());
        let patch = mgr.patch("abc123").await.unwrap();
        assert!(patch.files.is_empty());
        assert_eq!(patch.hash, "abc123");
    }

    #[tokio::test]
    async fn track_returns_hash_in_git_repo() {
        let dir = tempfile::tempdir().unwrap();

        // Initialize a real git repo
        let status = tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        assert!(status.status.success());

        // Configure git user for commits
        let _ = tokio::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .await;
        let _ = tokio::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .await;

        // Create a file
        tokio::fs::write(dir.path().join("hello.txt"), "hello world").await.unwrap();

        let mgr = SnapshotManager::new("test-project", dir.path().to_path_buf());
        assert!(mgr.is_enabled());

        let hash = mgr.track().await.unwrap();
        assert!(hash.is_some());
        let hash = hash.unwrap();
        assert!(!hash.is_empty());
        // Git tree hashes are 40 hex characters
        assert_eq!(hash.len(), 40);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn track_idempotent() {
        let dir = tempfile::tempdir().unwrap();

        let status = tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        assert!(status.status.success());

        tokio::fs::write(dir.path().join("file.txt"), "content").await.unwrap();

        let mgr = SnapshotManager::new("test-project", dir.path().to_path_buf());
        let h1 = mgr.track().await.unwrap().unwrap();
        let h2 = mgr.track().await.unwrap().unwrap();
        assert_eq!(h1, h2, "same content should produce same tree hash");
    }

    #[tokio::test]
    async fn patch_detects_changes() {
        let dir = tempfile::tempdir().unwrap();

        let status = tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        assert!(status.status.success());

        // Create initial file and snapshot
        tokio::fs::write(dir.path().join("file.txt"), "initial").await.unwrap();
        let mgr = SnapshotManager::new("test-project", dir.path().to_path_buf());
        let h1 = mgr.track().await.unwrap().unwrap();

        // Modify the file
        tokio::fs::write(dir.path().join("file.txt"), "modified").await.unwrap();

        let patch = mgr.patch(&h1).await.unwrap();
        assert!(!patch.files.is_empty(), "should detect the changed file");
        assert!(
            patch.files.iter().any(|f| f.contains("file.txt")),
            "should include file.txt in changed files"
        );
    }

    #[tokio::test]
    async fn restore_reverts_content() {
        let dir = tempfile::tempdir().unwrap();

        let status = tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        assert!(status.status.success());

        let file = dir.path().join("file.txt");

        // Create and snapshot
        tokio::fs::write(&file, "original").await.unwrap();
        let mgr = SnapshotManager::new("test-project", dir.path().to_path_buf());
        let snapshot = mgr.track().await.unwrap().unwrap();

        // Modify the file
        tokio::fs::write(&file, "changed").await.unwrap();
        let content = tokio::fs::read_to_string(&file).await.unwrap();
        assert_eq!(content, "changed");

        // Restore
        mgr.restore(&snapshot).await.unwrap();
        let content = tokio::fs::read_to_string(&file).await.unwrap();
        assert_eq!(content, "original");
    }

    #[tokio::test]
    async fn revert_deletes_new_files() {
        let dir = tempfile::tempdir().unwrap();

        let status = tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        assert!(status.status.success());

        // Snapshot with one file
        tokio::fs::write(dir.path().join("old.txt"), "old").await.unwrap();
        let mgr = SnapshotManager::new("test-project", dir.path().to_path_buf());
        let snapshot = mgr.track().await.unwrap().unwrap();

        // Add a new file
        let new_file = dir.path().join("new.txt");
        tokio::fs::write(&new_file, "new content").await.unwrap();
        assert!(new_file.exists());

        // Revert the new file — it shouldn't exist in the snapshot, so it gets deleted
        let patch = Patch { hash: snapshot, files: vec![new_file.to_string_lossy().to_string()] };
        mgr.revert(&[patch]).await.unwrap();

        assert!(!new_file.exists(), "new file should be deleted on revert");
    }
}
