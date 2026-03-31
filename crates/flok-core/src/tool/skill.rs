//! The `skill` tool — loads skill instructions.
//!
//! Skills are markdown files that provide specialized instructions and
//! workflows for specific tasks. Flok ships with built-in skills compiled
//! into the binary. Users can override any built-in skill by placing a
//! file in `.flok/skills/` (project) or the global skills directory.
//!
//! Search order (first match wins):
//! 1. Project `.flok/skills/<name>` / `.flok/skills/<name>.md` / `.flok/skills/<name>/SKILL.md`
//! 2. Global `~/.config/flok/skills/<name>.md` / `~/.config/flok/skills/<name>/SKILL.md`
//! 3. Built-in skills compiled into the binary

use std::path::Path;

use super::{Tool, ToolContext, ToolOutput};
use crate::skills;

/// Maximum skill content size (20KB).
const MAX_SKILL_SIZE: usize = 20_000;

/// Load a skill's instructions.
pub struct SkillTool;

#[async_trait::async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &'static str {
        "skill"
    }

    fn description(&self) -> &'static str {
        "Load a specialized skill that provides domain-specific instructions and workflows. \
         Flok includes built-in skills (code-review, spec-review, self-review-loop, \
         handle-pr-feedback). Project-local skills in .flok/skills/ override built-ins."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        let builtin_list = skills::BUILTIN_SKILLS
            .iter()
            .map(|s| format!("- **{}**: {}", s.name, s.description))
            .collect::<Vec<_>>()
            .join("\n");

        serde_json::json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": {
                    "type": "string",
                    "description": format!(
                        "The name of the skill to load.\n\nBuilt-in skills:\n{builtin_list}"
                    )
                }
            }
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let name = args["name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: name"))?;

        // Sanitize name — no path traversal
        if name.contains("..") || name.contains('/') || name.contains('\\') {
            return Ok(ToolOutput::error(
                "Invalid skill name: must not contain path separators or '..'",
            ));
        }

        // 1. Search project-local skills (user override)
        let project_paths = [
            ctx.project_root.join(".flok").join("skills").join(name),
            ctx.project_root.join(".flok").join("skills").join(format!("{name}.md")),
            ctx.project_root.join(".flok").join("skills").join(name).join("SKILL.md"),
        ];

        for path in &project_paths {
            if path.exists() && path.is_file() {
                return load_skill_file(path).await;
            }
        }

        // 2. Search global skills directory
        if let Some(config_dir) =
            directories::BaseDirs::new().map(|d| d.config_dir().join("flok").join("skills"))
        {
            let global_paths =
                [config_dir.join(format!("{name}.md")), config_dir.join(name).join("SKILL.md")];
            for path in &global_paths {
                if path.exists() && path.is_file() {
                    return load_skill_file(path).await;
                }
            }
        }

        // 3. Check built-in skills (compiled into the binary)
        if let Some(builtin) = skills::get_builtin_skill(name) {
            return Ok(ToolOutput::success(format!(
                "<skill_content name=\"{name}\">\n{}\n</skill_content>",
                builtin.content
            )));
        }

        // Not found — list available skills
        let available = list_all_skills(&ctx.project_root);
        if available.is_empty() {
            Ok(ToolOutput::error(format!("Skill '{name}' not found.")))
        } else {
            Ok(ToolOutput::error(format!(
                "Skill '{name}' not found. Available skills: {}",
                available.join(", ")
            )))
        }
    }
}

/// Load a skill from a filesystem path.
async fn load_skill_file(path: &Path) -> anyhow::Result<ToolOutput> {
    let content = tokio::fs::read_to_string(path).await?;
    let name = path
        .file_stem()
        .or_else(|| path.parent().and_then(|p| p.file_name()))
        .map_or("unknown", |n| n.to_str().unwrap_or("unknown"));

    if content.len() > MAX_SKILL_SIZE {
        Ok(ToolOutput::success(format!(
            "<skill_content name=\"{name}\">\n{}\n\n... (skill truncated at 20KB)\n</skill_content>",
            &content[..MAX_SKILL_SIZE]
        )))
    } else {
        Ok(ToolOutput::success(format!(
            "<skill_content name=\"{name}\">\n{content}\n</skill_content>"
        )))
    }
}

/// List all available skills (filesystem + built-in).
fn list_all_skills(project_root: &Path) -> Vec<String> {
    let mut skills = Vec::new();

    // Filesystem skills
    let skills_dir = project_root.join(".flok").join("skills");
    if let Ok(entries) = std::fs::read_dir(skills_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let name = name.strip_suffix(".md").unwrap_or(&name).to_string();
            if !skills.contains(&name) {
                skills.push(name);
            }
        }
    }

    // Built-in skills
    for builtin in skills::BUILTIN_SKILLS {
        let name = builtin.name.to_string();
        if !skills.contains(&name) {
            skills.push(name);
        }
    }

    skills.sort();
    skills
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_ctx() -> ToolContext {
        ToolContext {
            project_root: PathBuf::from("/nonexistent"),
            session_id: "test".into(),
            agent: "test".into(),
            cancel: tokio_util::sync::CancellationToken::new(),
        }
    }

    #[tokio::test]
    async fn skill_not_found() {
        let tool = SkillTool;
        let args = serde_json::json!({"name": "nonexistent-skill"});
        let result = tool.execute(args, &test_ctx()).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn skill_rejects_path_traversal() {
        let tool = SkillTool;
        let args = serde_json::json!({"name": "../../../etc/passwd"});
        let result = tool.execute(args, &test_ctx()).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Invalid"));
    }

    #[tokio::test]
    async fn skill_loads_builtin_code_review() {
        let tool = SkillTool;
        let args = serde_json::json!({"name": "code-review"});
        let result = tool.execute(args, &test_ctx()).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("Code Review Skill"));
        assert!(result.content.contains("skill_content"));
    }

    #[tokio::test]
    async fn skill_loads_builtin_spec_review() {
        let tool = SkillTool;
        let args = serde_json::json!({"name": "spec-review"});
        let result = tool.execute(args, &test_ctx()).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("Spec Review Skill"));
    }

    #[tokio::test]
    async fn skill_loads_builtin_self_review() {
        let tool = SkillTool;
        let args = serde_json::json!({"name": "self-review-loop"});
        let result = tool.execute(args, &test_ctx()).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("Self-Review Loop"));
    }

    #[tokio::test]
    async fn skill_loads_builtin_handle_pr_feedback() {
        let tool = SkillTool;
        let args = serde_json::json!({"name": "handle-pr-feedback"});
        let result = tool.execute(args, &test_ctx()).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("Handle PR Feedback"));
    }

    #[tokio::test]
    async fn list_all_includes_builtins() {
        let skills = list_all_skills(std::path::Path::new("/nonexistent"));
        assert!(skills.contains(&"code-review".to_string()));
        assert!(skills.contains(&"spec-review".to_string()));
        assert!(skills.contains(&"self-review-loop".to_string()));
        assert!(skills.contains(&"handle-pr-feedback".to_string()));
    }
}
