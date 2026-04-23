//! # Built-in Skills
//!
//! Skills that ship with flok and are compiled into the binary via `include_str!`.
//! Users can override any built-in skill by placing a file with the same name
//! in their project's `.flok/skills/` directory or the global skills directory.
//!
//! This means flok works great right out of the box — no plugins, no setup,
//! no shopping for extensions. Just install and go.

/// A high-confidence built-in skill match for the current user request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoSkillMatch {
    /// The built-in skill name.
    pub name: &'static str,
    /// Short explanation of why the skill matched.
    pub reason: &'static str,
}

/// A built-in skill definition.
pub struct BuiltinSkill {
    /// The skill name (used for lookup, e.g., "code-review").
    pub name: &'static str,
    /// The skill content (markdown).
    pub content: &'static str,
    /// A short description for listing.
    pub description: &'static str,
}

/// All built-in skills compiled into the binary.
pub static BUILTIN_SKILLS: &[BuiltinSkill] = &[
    BuiltinSkill {
        name: "code-review",
        content: include_str!("code-review.md"),
        description: "Reviews a GitHub PR using a parallel agent team with specialist reviewers",
    },
    BuiltinSkill {
        name: "self-review-loop",
        content: include_str!("self-review-loop.md"),
        description: "Iterative self-review loop: review, fix, re-review until clean",
    },
    BuiltinSkill {
        name: "spec-review",
        content: include_str!("spec-review.md"),
        description: "Parallel spec review using specialist agents",
    },
    BuiltinSkill {
        name: "handle-pr-feedback",
        content: include_str!("handle-pr-feedback.md"),
        description: "Reads PR review comments, applies fixes, replies to each comment",
    },
    BuiltinSkill {
        name: "source-driven-development",
        content: include_str!("source-driven-development.md"),
        description:
            "Grounds framework-specific code decisions in official documentation with cited sources",
    },
];

/// Look up a built-in skill by name.
pub fn get_builtin_skill(name: &str) -> Option<&'static BuiltinSkill> {
    BUILTIN_SKILLS.iter().find(|s| s.name == name)
}

/// List all built-in skill names.
pub fn builtin_skill_names() -> Vec<&'static str> {
    BUILTIN_SKILLS.iter().map(|s| s.name).collect()
}

/// Detect built-in skills that strongly match the current user request.
///
/// This is intent-based routing, not exact command matching. It exists so
/// flok can automatically apply specialized workflows for messages like:
/// - "review my PR"
/// - "please audit this pull request"
/// - "<github pr url> can you review this?"
pub fn detect_builtin_skills(user_text: &str) -> Vec<AutoSkillMatch> {
    let mut matches = Vec::new();
    if detect_code_review_intent(user_text) {
        matches.push(AutoSkillMatch {
            name: "code-review",
            reason: "the request looks like a pull request review",
        });
    }
    matches
}

fn detect_code_review_intent(user_text: &str) -> bool {
    let lower = user_text.to_lowercase();
    let review_signal = contains_any_phrase(
        &lower,
        &[
            "review",
            "audit",
            "critique",
            "look over",
            "find issues",
            "check for regressions",
            "code review",
        ],
    );
    let pr_signal = contains_any_phrase(&lower, &["pull request", "merge request"])
        || tokenize(&lower).iter().any(|token| *token == "pr" || *token == "mr");
    let github_pr_url = lower.contains("github.com/") && lower.contains("/pull/");

    (review_signal && (pr_signal || github_pr_url)) || (github_pr_url && pr_signal)
}

fn contains_any_phrase(text: &str, phrases: &[&str]) -> bool {
    phrases.iter().any(|phrase| text.contains(phrase))
}

fn tokenize(text: &str) -> Vec<&str> {
    text.split(|c: char| !c.is_ascii_alphanumeric()).filter(|token| !token.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_builtin_skills_matches_pr_review_url_first() {
        let matches =
            detect_builtin_skills("https://github.com/pamelia/flok/pull/45 please review my PR");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "code-review");
    }

    #[test]
    fn detect_builtin_skills_matches_pull_request_review_without_url() {
        let matches = detect_builtin_skills("Can you audit this pull request for regressions?");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "code-review");
    }

    #[test]
    fn detect_builtin_skills_ignores_non_review_pr_requests() {
        let matches = detect_builtin_skills("Summarize https://github.com/pamelia/flok/pull/45");
        assert!(matches.is_empty());
    }
}
