//! The `question` tool — asks the user a question with selectable options.
//!
//! The tool sends a request through a channel to the TUI, which displays
//! a dialog. The user selects an option, and the response is returned to
//! the LLM.

use tokio::sync::{mpsc, oneshot};

use super::{Tool, ToolContext, ToolOutput};

/// A question request sent to the TUI.
#[derive(Debug)]
pub struct QuestionRequest {
    /// The question to ask.
    pub question: String,
    /// Available options to choose from.
    pub options: Vec<String>,
    /// Whether the user can type a custom answer.
    pub allow_custom: bool,
    /// Channel to send the answer back.
    pub response_tx: oneshot::Sender<String>,
}

/// Asks the user a question with selectable options.
pub struct QuestionTool {
    /// Channel to send question requests to the TUI.
    request_tx: mpsc::UnboundedSender<QuestionRequest>,
}

impl QuestionTool {
    /// Create a new question tool connected to the TUI.
    pub fn new(request_tx: mpsc::UnboundedSender<QuestionRequest>) -> Self {
        Self { request_tx }
    }
}

#[async_trait::async_trait]
impl Tool for QuestionTool {
    fn name(&self) -> &'static str {
        "question"
    }

    fn description(&self) -> &'static str {
        "Ask the user a question with selectable options. Use this when you need \
         clarification, a decision, or a preference from the user. Returns the \
         user's selected option as text."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["question", "options"],
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the user"
                },
                "options": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Available options for the user to choose from"
                },
                "allow_custom": {
                    "type": "boolean",
                    "description": "Whether the user can type a custom answer (default: true)"
                }
            }
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let question = args["question"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: question"))?
            .to_string();

        let options: Vec<String> = args["options"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: options"))?
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();

        if options.is_empty() {
            return Ok(ToolOutput::error("options array must not be empty"));
        }

        let allow_custom = args["allow_custom"].as_bool().unwrap_or(true);

        let (response_tx, response_rx) = oneshot::channel();

        let request = QuestionRequest { question, options, allow_custom, response_tx };

        if self.request_tx.send(request).is_err() {
            return Ok(ToolOutput::error("TUI not available to ask questions"));
        }

        match response_rx.await {
            Ok(answer) => Ok(ToolOutput::success(answer)),
            Err(_) => Ok(ToolOutput::error("Question was dismissed")),
        }
    }
}
