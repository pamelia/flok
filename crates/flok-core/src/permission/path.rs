//! Project boundary detection for bash commands.
//!
//! Extracts file paths from shell commands using tree-sitter-bash and checks
//! whether they fall within the project root. This enables auto-allowing
//! in-project commands while prompting for external directory access.

use std::path::{Path, PathBuf};

/// Commands known to operate on filesystem paths.
///
/// When we see one of these as the first word of a command, we extract
/// subsequent non-flag arguments as potential file paths.
const FILESYSTEM_COMMANDS: &[&str] = &[
    "cat", "cd", "chmod", "chown", "cp", "find", "head", "less", "ln", "ls", "mkdir", "more", "mv",
    "rm", "rmdir", "stat", "tail", "touch", "tree", "du", "df", "file", "wc", "sort", "uniq",
    "tee",
];

/// Check if a resolved path is within the project root.
///
/// Attempts to canonicalize both paths. If canonicalization fails (e.g., the
/// path doesn't exist yet), falls back to lexical prefix checking after
/// normalizing the path.
pub fn is_within_project(path: &Path, project_root: &Path) -> bool {
    // Try canonical comparison first (resolves symlinks)
    if let (Ok(canonical_path), Ok(canonical_root)) =
        (std::fs::canonicalize(path), std::fs::canonicalize(project_root))
    {
        return canonical_path.starts_with(&canonical_root);
    }

    // Fallback: lexical comparison after making absolute
    let abs_path = if path.is_absolute() { path.to_path_buf() } else { project_root.join(path) };

    // Normalize by resolving `.` and `..` lexically
    let normalized = normalize_path(&abs_path);
    let normalized_root = normalize_path(project_root);

    normalized.starts_with(&normalized_root)
}

/// Normalize a path by resolving `.` and `..` components lexically
/// (without touching the filesystem).
fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                components.pop();
            }
            std::path::Component::CurDir => {}
            other => components.push(other),
        }
    }
    components.iter().collect()
}

/// Extract file paths from a bash command string using tree-sitter.
///
/// Parses the command, finds filesystem commands, and returns their
/// path arguments resolved against the given working directory.
pub fn extract_paths_from_command(command: &str, cwd: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // Parse the command with tree-sitter-bash
    let mut parser = tree_sitter::Parser::new();
    let language = tree_sitter_bash::LANGUAGE;
    parser.set_language(&language.into()).ok();

    let Some(tree) = parser.parse(command, None) else {
        // If parsing fails, fall back to simple tokenization
        return extract_paths_simple(command, cwd);
    };

    let root = tree.root_node();
    extract_paths_from_node(root, command.as_bytes(), cwd, &mut paths);

    paths
}

/// Walk the tree-sitter AST looking for command nodes with filesystem commands.
fn extract_paths_from_node(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    cwd: &Path,
    paths: &mut Vec<PathBuf>,
) {
    if node.kind() == "command" {
        extract_paths_from_command_node(node, source, cwd, paths);
    }

    // Recurse into children
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_paths_from_node(child, source, cwd, paths);
    }
}

/// Extract paths from a single command node.
fn extract_paths_from_command_node(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    cwd: &Path,
    paths: &mut Vec<PathBuf>,
) {
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();

    // Find the command name (first "word" or "command_name" child)
    let cmd_name = children.iter().find_map(|child| {
        if child.kind() == "command_name"
            || (child.kind() == "word" && child.start_byte() == node.start_byte())
        {
            child.utf8_text(source).ok()
        } else {
            None
        }
    });

    let Some(cmd_name) = cmd_name else {
        return;
    };

    // Check if this is a filesystem command
    let base_cmd = cmd_name.rsplit('/').next().unwrap_or(cmd_name);
    if !FILESYSTEM_COMMANDS.contains(&base_cmd) {
        return;
    }

    // Extract non-flag arguments as potential paths
    let mut seen_cmd = false;
    for child in &children {
        let Ok(text) = child.utf8_text(source) else {
            continue;
        };

        // Skip the command name itself
        if !seen_cmd {
            if child.kind() == "command_name" || child.kind() == "word" {
                seen_cmd = true;
            }
            continue;
        }

        // Skip flags
        if text.starts_with('-') {
            continue;
        }

        // Skip operator-like nodes
        if child.kind() == "file_redirect" || child.kind() == "heredoc_redirect" {
            continue;
        }

        // This looks like a path argument
        if child.kind() == "word"
            || child.kind() == "string"
            || child.kind() == "raw_string"
            || child.kind() == "concatenation"
        {
            let path_str = text.trim_matches(|c| c == '\'' || c == '"');
            if !path_str.is_empty() {
                let path = if Path::new(path_str).is_absolute() {
                    PathBuf::from(path_str)
                } else {
                    cwd.join(path_str)
                };
                paths.push(path);
            }
        }
    }
}

/// Simple fallback path extraction using whitespace tokenization.
///
/// Used when tree-sitter parsing fails.
fn extract_paths_simple(command: &str, cwd: &Path) -> Vec<PathBuf> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    if tokens.is_empty() {
        return Vec::new();
    }

    let cmd = tokens[0].rsplit('/').next().unwrap_or(tokens[0]);
    if !FILESYSTEM_COMMANDS.contains(&cmd) {
        return Vec::new();
    }

    tokens[1..]
        .iter()
        .filter(|t| !t.starts_with('-'))
        .map(|t| {
            let path_str = t.trim_matches(|c: char| c == '\'' || c == '"');
            if Path::new(path_str).is_absolute() {
                PathBuf::from(path_str)
            } else {
                cwd.join(path_str)
            }
        })
        .collect()
}

/// Check if a bash command references any paths outside the project root.
///
/// Returns `true` if the command contains path arguments that resolve to
/// locations outside the project root, or `false` if all paths (or no paths)
/// are within the project.
pub fn command_touches_external_paths(command: &str, project_root: &Path) -> bool {
    let paths = extract_paths_from_command(command, project_root);
    paths.iter().any(|p| !is_within_project(p, project_root))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_root() -> PathBuf {
        PathBuf::from("/home/user/project")
    }

    #[test]
    fn is_within_project_relative_paths() {
        let root = project_root();
        assert!(is_within_project(Path::new("src/main.rs"), &root));
        assert!(is_within_project(Path::new("./src/main.rs"), &root));
    }

    #[test]
    fn is_within_project_absolute_in_project() {
        let root = project_root();
        assert!(is_within_project(Path::new("/home/user/project/src/main.rs"), &root));
    }

    #[test]
    fn is_within_project_outside_project() {
        let root = project_root();
        assert!(!is_within_project(Path::new("/etc/passwd"), &root));
        assert!(!is_within_project(Path::new("/home/user/other"), &root));
    }

    #[test]
    fn is_within_project_parent_traversal() {
        let root = project_root();
        // ../other escapes the project
        assert!(!is_within_project(Path::new("/home/user/project/../other/file"), &root));
    }

    #[test]
    fn normalize_resolves_dot_dot() {
        assert_eq!(normalize_path(Path::new("/a/b/../c")), PathBuf::from("/a/c"));
        assert_eq!(normalize_path(Path::new("/a/./b/./c")), PathBuf::from("/a/b/c"));
    }

    #[test]
    fn extract_paths_ls() {
        let paths = extract_paths_from_command("ls -la src/", &project_root());
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], project_root().join("src/"));
    }

    #[test]
    fn extract_paths_rm() {
        let paths = extract_paths_from_command("rm -rf /tmp/junk", &project_root());
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], PathBuf::from("/tmp/junk"));
    }

    #[test]
    fn extract_paths_cp_multiple() {
        let paths = extract_paths_from_command("cp src/a.rs src/b.rs", &project_root());
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn extract_paths_non_filesystem_command_returns_empty() {
        let paths = extract_paths_from_command("git status", &project_root());
        assert!(paths.is_empty());

        let paths = extract_paths_from_command("echo hello", &project_root());
        assert!(paths.is_empty());
    }

    #[test]
    fn command_touches_external_rm_tmp() {
        assert!(command_touches_external_paths("rm -rf /tmp/junk", &project_root()));
    }

    #[test]
    fn command_touches_external_ls_in_project() {
        assert!(!command_touches_external_paths("ls -la src/", &project_root()));
    }

    #[test]
    fn command_touches_external_no_paths() {
        // Non-filesystem commands have no paths — not external
        assert!(!command_touches_external_paths("git status", &project_root()));
        assert!(!command_touches_external_paths("cargo test --workspace", &project_root()));
    }

    #[test]
    fn command_touches_external_cd_parent() {
        assert!(command_touches_external_paths("cat /etc/passwd", &project_root()));
    }
}
