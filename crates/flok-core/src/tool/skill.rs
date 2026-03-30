//! The `skill` tool — loads skill instructions from a file.
//!
//! Skills are markdown files that provide specialized instructions and
//! workflows for specific tasks. They're loaded from `.flok/skills/`
//! in the project root or from a global skills directory.

use std::path::Path;

use super::{Tool, ToolContext, ToolOutput};

/// Load a skill's instructions from a file.
pub struct SkillTool;

#[async_trait::async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &'static str {
        "skill"
    }

    fn description(&self) -> &'static str {
        "Load a specialized skill that provides domain-specific instructions and workflows. \
         Skills are markdown files in .flok/skills/ directory."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The name of the skill to load (e.g., 'code-review', 'spec-review')"
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

        // Search paths in order
        let search_paths = [
            ctx.project_root.join(".flok").join("skills").join(name),
            ctx.project_root.join(".flok").join("skills").join(format!("{name}.md")),
            ctx.project_root.join(".flok").join("skills").join(name).join("SKILL.md"),
        ];

        for path in &search_paths {
            if path.exists() && path.is_file() {
                return load_skill(path).await;
            }
        }

        // Try global skills directory
        if let Some(config_dir) =
            directories::BaseDirs::new().map(|d| d.config_dir().join("flok").join("skills"))
        {
            let global_paths =
                [config_dir.join(format!("{name}.md")), config_dir.join(name).join("SKILL.md")];
            for path in &global_paths {
                if path.exists() && path.is_file() {
                    return load_skill(path).await;
                }
            }
        }

        // List available skills
        let available = list_available_skills(&ctx.project_root);
        if available.is_empty() {
            Ok(ToolOutput::error(format!(
                "Skill '{name}' not found. No skills found in .flok/skills/"
            )))
        } else {
            Ok(ToolOutput::error(format!(
                "Skill '{name}' not found. Available skills: {}",
                available.join(", ")
            )))
        }
    }
}

async fn load_skill(path: &Path) -> anyhow::Result<ToolOutput> {
    let content = tokio::fs::read_to_string(path).await?;
    // Cap at 20KB
    if content.len() > 20_000 {
        Ok(ToolOutput::success(format!("{}\n\n... (skill truncated at 20KB)", &content[..20_000])))
    } else {
        Ok(ToolOutput::success(content))
    }
}

fn list_available_skills(project_root: &Path) -> Vec<String> {
    let mut skills = Vec::new();
    let skills_dir = project_root.join(".flok").join("skills");
    if let Ok(entries) = std::fs::read_dir(skills_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let name = name.strip_suffix(".md").unwrap_or(&name).to_string();
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

    #[tokio::test]
    async fn skill_not_found() {
        let tool = SkillTool;
        let ctx = ToolContext {
            project_root: PathBuf::from("/nonexistent"),
            session_id: "test".into(),
            agent: "test".into(),
            cancel: tokio_util::sync::CancellationToken::new(),
        };
        let args = serde_json::json!({"name": "nonexistent-skill"});
        let result = tool.execute(args, &ctx).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn skill_rejects_path_traversal() {
        let tool = SkillTool;
        let ctx = ToolContext {
            project_root: PathBuf::from("/tmp"),
            session_id: "test".into(),
            agent: "test".into(),
            cancel: tokio_util::sync::CancellationToken::new(),
        };
        let args = serde_json::json!({"name": "../../../etc/passwd"});
        let result = tool.execute(args, &ctx).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Invalid"));
    }
}
