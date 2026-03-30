//! Agent definitions and registry.
//!
//! Agents are configured with a name, description, system prompt override,
//! and tool permissions. The primary agents are "build" and "plan". Sub-agents
//! include "explore" (fast codebase search) and "general" (multi-step tasks).

/// An agent definition.
#[derive(Debug, Clone)]
pub struct AgentDef {
    /// Unique agent name.
    pub name: &'static str,
    /// Human-readable description (shown to the LLM in the Task tool).
    pub description: &'static str,
    /// Custom system prompt (replaces the default if set).
    pub system_prompt: Option<&'static str>,
    /// Whether this is a sub-agent (spawned by the Task tool).
    pub is_subagent: bool,
}

/// Get the list of available sub-agents for the Task tool.
pub fn subagents() -> Vec<AgentDef> {
    vec![
        AgentDef {
            name: "explore",
            description: "Fast agent specialized for exploring codebases. Use this when you need \
                          to quickly find files by patterns, search code for keywords, or answer \
                          questions about the codebase. Specify thoroughness: \"quick\", \"medium\", \
                          or \"very thorough\".",
            system_prompt: Some(EXPLORE_PROMPT),
            is_subagent: true,
        },
        AgentDef {
            name: "general",
            description: "General-purpose agent for researching complex questions and executing \
                          multi-step tasks. Use this agent to execute multiple units of work in parallel.",
            system_prompt: None,
            is_subagent: true,
        },
    ]
}

/// Get a sub-agent definition by name.
pub fn get_subagent(name: &str) -> Option<AgentDef> {
    subagents().into_iter().find(|a| a.name == name)
}

/// Format the agent list for inclusion in the Task tool description.
pub fn format_agent_list() -> String {
    use std::fmt::Write;
    let mut result = String::new();
    for agent in subagents() {
        let _ = writeln!(result, "- **{}**: {}", agent.name, agent.description);
    }
    result
}

const EXPLORE_PROMPT: &str = r"You are a file search specialist. You excel at thoroughly navigating and exploring codebases.

Your strengths:
- Rapidly finding files using glob patterns
- Searching code and text with powerful regex patterns
- Reading and analyzing file contents

Guidelines:
- Use Glob for broad file pattern matching
- Use Grep for searching file contents with regex
- Use Read when you know the specific file path you need to read
- Use Bash for file operations like listing directory contents
- Adapt your search approach based on the thoroughness level specified by the caller
- Return file paths as absolute paths in your final response
- Do not create any files, or run bash commands that modify the user's system state in any way

Complete the user's search request efficiently and report your findings clearly.";
