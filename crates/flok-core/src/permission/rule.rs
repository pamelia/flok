//! Permission rule types and wildcard matching.

use serde::{Deserialize, Serialize};

/// The action to take when a permission rule matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionAction {
    /// Allow the operation without prompting.
    Allow,
    /// Deny the operation without prompting.
    Deny,
    /// Ask the user for permission.
    Ask,
}

/// A single permission rule.
///
/// Rules are matched against a `(permission, pattern)` pair using wildcard
/// matching. In a ruleset, the **last matching rule wins**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRule {
    /// The permission type to match (e.g., `"bash"`, `"edit"`, `"*"`).
    pub permission: String,
    /// The pattern to match within that permission type.
    /// For bash: a command pattern like `"git commit *"`.
    /// For file tools: a file path pattern like `"*.env"`.
    pub pattern: String,
    /// The action to take when this rule matches.
    pub action: PermissionAction,
}

impl PermissionRule {
    /// Create a new permission rule.
    pub fn new(
        permission: impl Into<String>,
        pattern: impl Into<String>,
        action: PermissionAction,
    ) -> Self {
        Self { permission: permission.into(), pattern: pattern.into(), action }
    }
}

/// Match a string against a wildcard pattern.
///
/// Supports:
/// - `*` matches zero or more of any character
/// - `?` matches exactly one character
///
/// All other characters are matched literally (case-sensitive).
///
/// # Examples
///
/// ```
/// use flok_core::permission::rule::wildcard_match;
///
/// assert!(wildcard_match("*", "anything"));
/// assert!(wildcard_match("git *", "git commit -m fix"));
/// assert!(wildcard_match("*.env", "foo.env"));
/// assert!(wildcard_match("*.env.*", "foo.env.local"));
/// assert!(!wildcard_match("*.env", "foo.env.local"));
/// assert!(wildcard_match("npm install *", "npm install express"));
/// assert!(!wildcard_match("npm install *", "npm publish"));
/// ```
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    // DP-based wildcard matching (efficient, no regex compilation)
    let pat: Vec<char> = pattern.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();
    let m = pat.len();
    let n = text_chars.len();

    let mut dp = vec![vec![false; n + 1]; m + 1];
    dp[0][0] = true;

    // Leading '*' patterns match empty text
    for i in 1..=m {
        if pat[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
    }

    for i in 1..=m {
        for j in 1..=n {
            if pat[i - 1] == '*' {
                // '*' matches zero chars (dp[i-1][j]) or one more char (dp[i][j-1])
                dp[i][j] = dp[i - 1][j] || dp[i][j - 1];
            } else if pat[i - 1] == '?' || pat[i - 1] == text_chars[j - 1] {
                dp[i][j] = dp[i - 1][j - 1];
            }
        }
    }

    dp[m][n]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_match_star_matches_everything() {
        assert!(wildcard_match("*", ""));
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("*", "hello world"));
    }

    #[test]
    fn wildcard_match_exact() {
        assert!(wildcard_match("hello", "hello"));
        assert!(!wildcard_match("hello", "world"));
        assert!(!wildcard_match("hello", "hello world"));
    }

    #[test]
    fn wildcard_match_trailing_star() {
        assert!(wildcard_match("git *", "git commit -m fix"));
        assert!(wildcard_match("git *", "git status"));
        assert!(!wildcard_match("git *", "npm install"));
    }

    #[test]
    fn wildcard_match_leading_star() {
        assert!(wildcard_match("*.env", "foo.env"));
        assert!(wildcard_match("*.env", ".env"));
        assert!(!wildcard_match("*.env", "foo.env.local"));
    }

    #[test]
    fn wildcard_match_middle_star() {
        assert!(wildcard_match("*.env.*", "foo.env.local"));
        assert!(wildcard_match("*.env.*", ".env.production"));
        assert!(!wildcard_match("*.env.*", "foo.env"));
    }

    #[test]
    fn wildcard_match_question_mark() {
        assert!(wildcard_match("?.txt", "a.txt"));
        assert!(!wildcard_match("?.txt", "ab.txt"));
        assert!(!wildcard_match("?.txt", ".txt"));
    }

    #[test]
    fn wildcard_match_complex_patterns() {
        assert!(wildcard_match("npm install *", "npm install express"));
        assert!(!wildcard_match("npm install *", "npm publish foo"));
        assert!(wildcard_match("git commit *", "git commit -m 'initial'"));
        assert!(!wildcard_match("git commit *", "git push origin main"));
    }

    #[test]
    fn wildcard_match_empty_pattern_and_text() {
        assert!(wildcard_match("", ""));
        assert!(!wildcard_match("", "nonempty"));
    }

    #[test]
    fn wildcard_match_multiple_stars() {
        assert!(wildcard_match("*.*", "file.txt"));
        assert!(wildcard_match("*.*", "path/to/file.txt"));
        assert!(!wildcard_match("*.*", "noextension"));
    }

    #[test]
    fn wildcard_match_permission_type() {
        // Permission type matching (used by evaluate)
        assert!(wildcard_match("*", "bash"));
        assert!(wildcard_match("*", "edit"));
        assert!(wildcard_match("bash", "bash"));
        assert!(!wildcard_match("bash", "edit"));
    }
}
