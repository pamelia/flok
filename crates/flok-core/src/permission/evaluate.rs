//! Rule evaluation engine.
//!
//! Evaluates a `(permission, pattern)` pair against layered rulesets using
//! **last-match-wins** semantics. This means later rulesets (e.g., session
//! approvals) can override earlier ones (e.g., defaults).

use super::rule::{wildcard_match, PermissionAction, PermissionRule};

/// Evaluate a permission check against layered rulesets.
///
/// Flattens all rulesets into a single list and finds the **last** rule
/// where both `permission` and `pattern` match using wildcard matching.
/// If no rule matches, defaults to [`PermissionAction::Ask`].
///
/// # Arguments
///
/// * `permission` — The permission type being checked (e.g., `"bash"`, `"edit"`)
/// * `pattern` — The specific pattern within that type (e.g., `"git commit -m fix"`, `"src/main.rs"`)
/// * `rulesets` — One or more rulesets to evaluate against, in precedence order
///   (later rulesets override earlier ones)
///
/// # Examples
///
/// ```
/// use flok_core::permission::{evaluate, PermissionAction, PermissionRule};
///
/// let defaults = vec![
///     PermissionRule::new("*", "*", PermissionAction::Allow),
///     PermissionRule::new("bash", "rm -rf *", PermissionAction::Deny),
/// ];
///
/// // "git status" matches the wildcard allow rule
/// assert_eq!(evaluate("bash", "git status", &[&defaults]), PermissionAction::Allow);
///
/// // "rm -rf /" matches the deny rule (last match wins, and deny comes after allow)
/// assert_eq!(evaluate("bash", "rm -rf /tmp", &[&defaults]), PermissionAction::Deny);
/// ```
pub fn evaluate(
    permission: &str,
    pattern: &str,
    rulesets: &[&[PermissionRule]],
) -> PermissionAction {
    rulesets
        .iter()
        .flat_map(|rs| rs.iter())
        .rfind(|rule| {
            wildcard_match(&rule.permission, permission) && wildcard_match(&rule.pattern, pattern)
        })
        .map_or(PermissionAction::Ask, |rule| rule.action)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(perm: &str, pat: &str, action: PermissionAction) -> PermissionRule {
        PermissionRule::new(perm, pat, action)
    }

    #[test]
    fn no_rules_defaults_to_ask() {
        let empty: Vec<PermissionRule> = vec![];
        assert_eq!(evaluate("bash", "ls", &[&empty]), PermissionAction::Ask);
    }

    #[test]
    fn wildcard_allow_all() {
        let rules = vec![rule("*", "*", PermissionAction::Allow)];
        assert_eq!(evaluate("bash", "anything", &[&rules]), PermissionAction::Allow);
        assert_eq!(evaluate("edit", "file.rs", &[&rules]), PermissionAction::Allow);
    }

    #[test]
    fn last_match_wins() {
        let rules = vec![
            rule("bash", "*", PermissionAction::Allow),
            rule("bash", "rm *", PermissionAction::Deny),
        ];
        assert_eq!(evaluate("bash", "git status", &[&rules]), PermissionAction::Allow);
        assert_eq!(evaluate("bash", "rm -rf /", &[&rules]), PermissionAction::Deny);
    }

    #[test]
    fn layered_rulesets_later_overrides_earlier() {
        let defaults = vec![rule("bash", "*", PermissionAction::Ask)];
        let config = vec![rule("bash", "*", PermissionAction::Allow)];
        assert_eq!(evaluate("bash", "ls", &[&defaults, &config]), PermissionAction::Allow);
    }

    #[test]
    fn session_rules_override_config() {
        let defaults = vec![rule("*", "*", PermissionAction::Allow)];
        let config = vec![rule("bash", "docker *", PermissionAction::Ask)];
        let session = vec![rule("bash", "docker build *", PermissionAction::Allow)];

        // docker run still asks (config rule)
        assert_eq!(
            evaluate("bash", "docker run nginx", &[&defaults, &config, &session]),
            PermissionAction::Ask,
        );
        // docker build is allowed (session override)
        assert_eq!(
            evaluate("bash", "docker build .", &[&defaults, &config, &session]),
            PermissionAction::Allow,
        );
    }

    #[test]
    fn permission_type_matching() {
        let rules = vec![
            rule("*", "*", PermissionAction::Allow),
            rule("external_directory", "*", PermissionAction::Ask),
        ];
        assert_eq!(evaluate("bash", "ls", &[&rules]), PermissionAction::Allow);
        assert_eq!(evaluate("external_directory", "/etc/passwd", &[&rules]), PermissionAction::Ask,);
    }

    #[test]
    fn env_file_rules() {
        let rules = vec![
            rule("read", "*", PermissionAction::Allow),
            rule("read", "*.env", PermissionAction::Ask),
            rule("read", "*.env.*", PermissionAction::Ask),
            rule("read", "*.env.example", PermissionAction::Allow),
        ];

        assert_eq!(evaluate("read", "src/main.rs", &[&rules]), PermissionAction::Allow);
        assert_eq!(evaluate("read", ".env", &[&rules]), PermissionAction::Ask);
        assert_eq!(evaluate("read", "app.env", &[&rules]), PermissionAction::Ask);
        assert_eq!(evaluate("read", ".env.local", &[&rules]), PermissionAction::Ask);
        assert_eq!(evaluate("read", ".env.example", &[&rules]), PermissionAction::Allow);
    }

    #[test]
    fn specific_command_patterns() {
        let rules = vec![
            rule("bash", "*", PermissionAction::Allow),
            rule("bash", "rm -rf *", PermissionAction::Deny),
            rule("bash", "git push --force *", PermissionAction::Ask),
        ];

        assert_eq!(evaluate("bash", "echo hello", &[&rules]), PermissionAction::Allow);
        assert_eq!(evaluate("bash", "rm -rf /", &[&rules]), PermissionAction::Deny);
        assert_eq!(
            evaluate("bash", "git push --force origin main", &[&rules]),
            PermissionAction::Ask,
        );
        // Regular git push is fine
        assert_eq!(evaluate("bash", "git push origin main", &[&rules]), PermissionAction::Allow);
    }
}
