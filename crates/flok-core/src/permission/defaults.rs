//! Default permission rules.
//!
//! These rules provide OpenCode-like behavior out of the box:
//! - All operations within the project directory are allowed
//! - External directory access requires confirmation
//! - Reading `.env` files requires confirmation
//! - Doom loop detection requires confirmation

use super::rule::{PermissionAction, PermissionRule};

/// Returns the default permission ruleset.
///
/// These rules match `OpenCode`'s behavior: in-project commands auto-execute,
/// external directory access prompts, and sensitive files require confirmation.
///
/// The rules are ordered so that more specific rules come after general ones
/// (last-match-wins semantics).
pub fn default_rules() -> Vec<PermissionRule> {
    use PermissionAction::{Allow, Ask};

    vec![
        // Base: everything allowed within the project
        PermissionRule::new("*", "*", Allow),
        // External directory access: always ask
        PermissionRule::new("external_directory", "*", Ask),
        // Sensitive files: ask before reading .env files
        PermissionRule::new("read", "*.env", Ask),
        PermissionRule::new("read", "*.env.*", Ask),
        PermissionRule::new("read", "*.env.example", Allow), // but .env.example is fine
        // Doom loop detection: ask
        PermissionRule::new("doom_loop", "*", Ask),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::evaluate;

    #[test]
    fn defaults_allow_in_project_bash() {
        let defaults = default_rules();
        assert_eq!(evaluate("bash", "ls -la", &[&defaults]), PermissionAction::Allow,);
        assert_eq!(evaluate("bash", "git status", &[&defaults]), PermissionAction::Allow,);
        assert_eq!(evaluate("bash", "npm run dev", &[&defaults]), PermissionAction::Allow,);
        assert_eq!(
            evaluate("bash", "cargo test --workspace", &[&defaults]),
            PermissionAction::Allow,
        );
    }

    #[test]
    fn defaults_allow_file_operations() {
        let defaults = default_rules();
        assert_eq!(evaluate("read", "src/main.rs", &[&defaults]), PermissionAction::Allow,);
        assert_eq!(evaluate("edit", "src/lib.rs", &[&defaults]), PermissionAction::Allow,);
        assert_eq!(evaluate("write", "src/new_file.rs", &[&defaults]), PermissionAction::Allow,);
    }

    #[test]
    fn defaults_ask_for_external_directory() {
        let defaults = default_rules();
        assert_eq!(
            evaluate("external_directory", "/etc/passwd", &[&defaults]),
            PermissionAction::Ask,
        );
    }

    #[test]
    fn defaults_ask_for_env_files() {
        let defaults = default_rules();
        assert_eq!(evaluate("read", ".env", &[&defaults]), PermissionAction::Ask);
        assert_eq!(evaluate("read", ".env.local", &[&defaults]), PermissionAction::Ask);
        assert_eq!(evaluate("read", "app.env", &[&defaults]), PermissionAction::Ask);
    }

    #[test]
    fn defaults_allow_env_example() {
        let defaults = default_rules();
        assert_eq!(evaluate("read", ".env.example", &[&defaults]), PermissionAction::Allow,);
    }

    #[test]
    fn defaults_ask_for_doom_loop() {
        let defaults = default_rules();
        assert_eq!(evaluate("doom_loop", "bash", &[&defaults]), PermissionAction::Ask,);
    }
}
