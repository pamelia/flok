//! Tool registry — holds all available tools and provides lookup/schema generation.

use std::collections::HashMap;
use std::sync::Arc;

use crate::provider::ToolDefinition;

use super::Tool;

/// Registry of all available tools.
#[derive(Clone)]
pub struct ToolRegistry {
    tools: HashMap<&'static str, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self { tools: HashMap::new() }
    }

    /// Register a tool.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name(), tool);
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    /// Generate tool definitions for the LLM (used in the completion request).
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|tool| ToolDefinition {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                input_schema: tool.parameters_schema(),
            })
            .collect()
    }

    /// List all registered tool names.
    pub fn names(&self) -> Vec<&'static str> {
        self.tools.keys().copied().collect()
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
            .field("tools", &self.tools.keys().collect::<Vec<_>>())
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
}
