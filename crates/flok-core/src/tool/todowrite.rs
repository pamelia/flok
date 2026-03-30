//! The `todowrite` tool — manages a task list for tracking progress.
//!
//! The LLM can create and update a list of tasks. The TUI sidebar displays
//! the current list. Tasks have a status (pending, `in_progress`, completed)
//! and a priority (high, medium, low).

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::{Tool, ToolContext, ToolOutput};

/// A single todo item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    /// Brief description of the task.
    pub content: String,
    /// Current status: `pending`, `in_progress`, `completed`, `cancelled`.
    pub status: String,
    /// Priority: `high`, `medium`, `low`.
    pub priority: String,
}

/// Shared todo list state, accessible by both the tool and the TUI.
#[derive(Debug, Clone)]
pub struct TodoList {
    items: Arc<Mutex<Vec<TodoItem>>>,
}

impl TodoList {
    /// Create a new empty todo list.
    pub fn new() -> Self {
        Self { items: Arc::new(Mutex::new(Vec::new())) }
    }

    /// Get a snapshot of the current items.
    pub fn items(&self) -> Vec<TodoItem> {
        self.items.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// Replace the entire list.
    pub fn set(&self, items: Vec<TodoItem>) {
        let mut list = self.items.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *list = items;
    }
}

impl Default for TodoList {
    fn default() -> Self {
        Self::new()
    }
}

/// Manages a task list for tracking progress.
pub struct TodoWriteTool {
    list: TodoList,
}

impl TodoWriteTool {
    /// Create a new todowrite tool with the given shared list.
    pub fn new(list: TodoList) -> Self {
        Self { list }
    }

    /// Get a reference to the shared todo list (for TUI display).
    pub fn list(&self) -> &TodoList {
        &self.list
    }
}

#[async_trait::async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &'static str {
        "todowrite"
    }

    fn description(&self) -> &'static str {
        "Write or update a task list to track progress. Replaces the entire list \
         with the provided items. Each item has content, status (pending/in_progress/completed/cancelled), \
         and priority (high/medium/low). Use this to plan and track multi-step tasks."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["todos"],
            "properties": {
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["content", "status", "priority"],
                        "properties": {
                            "content": {
                                "type": "string",
                                "description": "Brief description of the task"
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed", "cancelled"],
                                "description": "Current status of the task"
                            },
                            "priority": {
                                "type": "string",
                                "enum": ["high", "medium", "low"],
                                "description": "Priority level"
                            }
                        }
                    },
                    "description": "The complete todo list (replaces existing)"
                }
            }
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let todos = args["todos"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: todos"))?;

        let items: Vec<TodoItem> =
            todos.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect();

        let total = items.len();
        let completed = items.iter().filter(|i| i.status == "completed").count();
        let in_progress = items.iter().filter(|i| i.status == "in_progress").count();

        self.list.set(items);

        Ok(ToolOutput::success(format!(
            "Todo list updated: {total} items ({completed} completed, {in_progress} in progress)"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todo_list_set_and_get() {
        let list = TodoList::new();
        assert!(list.items().is_empty());

        list.set(vec![TodoItem {
            content: "test task".into(),
            status: "pending".into(),
            priority: "high".into(),
        }]);

        let items = list.items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].content, "test task");
    }

    #[tokio::test]
    async fn todowrite_updates_list() {
        let list = TodoList::new();
        let tool = TodoWriteTool::new(list.clone());
        let ctx = ToolContext::test(std::path::PathBuf::from("/tmp"));

        let args = serde_json::json!({
            "todos": [
                {"content": "Build feature", "status": "in_progress", "priority": "high"},
                {"content": "Write tests", "status": "pending", "priority": "medium"},
                {"content": "Update docs", "status": "completed", "priority": "low"}
            ]
        });

        let result = tool.execute(args, &ctx).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("3 items"));
        assert!(result.content.contains("1 completed"));
        assert!(result.content.contains("1 in progress"));

        let items = list.items();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].status, "in_progress");
    }
}
