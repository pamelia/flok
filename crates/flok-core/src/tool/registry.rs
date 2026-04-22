//! Tool registry — holds all available tools and provides lookup/schema generation.

use std::collections::HashMap;
use std::sync::Arc;

use crate::provider::ToolDefinition;

use super::Tool;

#[derive(Clone)]
struct DynamicToolEntry {
    description: String,
    tool: Arc<dyn Tool>,
}

/// Registry of all available tools.
#[derive(Clone)]
pub struct ToolRegistry {
    builtins: HashMap<String, Arc<dyn Tool>>,
    dynamic_tools: HashMap<String, DynamicToolEntry>,
}

impl ToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self { builtins: HashMap::new(), dynamic_tools: HashMap::new() }
    }

    /// Register a built-in tool.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.builtins.insert(tool.name().to_string(), tool);
    }

    /// Register a dynamic tool under an explicit name.
    pub fn register_dynamic(
        &mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        tool: Arc<dyn Tool>,
    ) {
        let name = name.into();
        if self.builtins.contains_key(&name) {
            tracing::warn!(tool = %name, "refusing to shadow built-in tool with dynamic tool");
            return;
        }
        self.dynamic_tools.insert(name, DynamicToolEntry { description: description.into(), tool });
    }

    /// Replace all dynamic tools belonging to a namespace like `github`.
    ///
    /// This removes previously registered tools whose name starts with
    /// `{namespace}_` and then inserts the provided replacements.
    pub fn replace_dynamic_namespace(
        &mut self,
        namespace: &str,
        tools: Vec<(String, String, Arc<dyn Tool>)>,
    ) {
        let prefix = format!("{namespace}_");
        self.dynamic_tools.retain(|name, _| !name.starts_with(&prefix));
        for (name, description, tool) in tools {
            self.register_dynamic(name, description, tool);
        }
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.builtins.get(name).or_else(|| self.dynamic_tools.get(name).map(|entry| &entry.tool))
    }

    /// Generate tool definitions for the LLM (used in the completion request).
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        let mut defs: Vec<_> = self
            .builtins
            .iter()
            .map(|tool| ToolDefinition {
                name: tool.0.clone(),
                description: tool.1.description().to_string(),
                input_schema: tool.1.parameters_schema(),
            })
            .chain(self.dynamic_tools.iter().map(|tool| ToolDefinition {
                name: tool.0.clone(),
                description: tool.1.description.clone(),
                input_schema: tool.1.tool.parameters_schema(),
            }))
            .collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    /// List all registered tool names.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<_> =
            self.builtins.keys().cloned().chain(self.dynamic_tools.keys().cloned()).collect();
        names.sort();
        names
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("builtins", &self.builtins.keys().collect::<Vec<_>>())
            .field("dynamic_tools", &self.dynamic_tools.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ReadTool;

    #[test]
    fn register_and_lookup_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(ReadTool));

        assert!(registry.get("read").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn tool_definitions_include_schema() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(ReadTool));

        let defs = registry.tool_definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "read");
        assert!(!defs[0].description.is_empty());
    }

    #[test]
    fn register_dynamic_tool_uses_owned_name() {
        let mut registry = ToolRegistry::new();
        registry.register_dynamic("github_list_repos", "List repositories", Arc::new(ReadTool));

        assert!(registry.get("github_list_repos").is_some());
        assert_eq!(registry.names(), vec!["github_list_repos".to_string()]);
        let defs = registry.tool_definitions();
        assert_eq!(defs[0].name, "github_list_repos");
        assert_eq!(defs[0].description, "List repositories");
    }

    #[test]
    fn replace_dynamic_namespace_only_updates_target_namespace() {
        let mut registry = ToolRegistry::new();
        registry.register_dynamic("github_list_repos", "List repositories", Arc::new(ReadTool));
        registry.register_dynamic("filesystem_read_file", "Read file", Arc::new(ReadTool));

        registry.replace_dynamic_namespace(
            "github",
            vec![("github_list_issues".to_string(), "List issues".to_string(), Arc::new(ReadTool))],
        );

        assert!(registry.get("github_list_repos").is_none());
        assert!(registry.get("github_list_issues").is_some());
        assert!(registry.get("filesystem_read_file").is_some());
    }

    #[test]
    fn register_dynamic_does_not_shadow_builtin_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(ReadTool));
        registry.register_dynamic("read", "shadow read", Arc::new(ReadTool));

        assert_eq!(registry.names(), vec!["read".to_string()]);
        assert_eq!(registry.tool_definitions().len(), 1);
    }
}
