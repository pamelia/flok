//! # Built-in Skills
//!
//! Skills that ship with flok and are compiled into the binary via `include_str!`.
//! Users can override any built-in skill by placing a file with the same name
//! in their project's `.flok/skills/` directory or the global skills directory.
//!
//! This means flok works great right out of the box — no plugins, no setup,
//! no shopping for extensions. Just install and go.

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
];

/// Look up a built-in skill by name.
pub fn get_builtin_skill(name: &str) -> Option<&'static BuiltinSkill> {
    BUILTIN_SKILLS.iter().find(|s| s.name == name)
}

/// List all built-in skill names.
pub fn builtin_skill_names() -> Vec<&'static str> {
    BUILTIN_SKILLS.iter().map(|s| s.name).collect()
}
