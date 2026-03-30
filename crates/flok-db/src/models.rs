//! Data models for database rows.

use serde::{Deserialize, Serialize};

/// A project tracked by flok.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub path: String,
    pub created_at: String,
}

/// A conversation session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub project_id: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub model_id: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Message role in a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Role {
    /// Convert to the database string representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Role {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "system" => Ok(Self::System),
            "user" => Ok(Self::User),
            "assistant" => Ok(Self::Assistant),
            other => Err(format!("invalid role: {other}")),
        }
    }
}

/// A raw message row from the database.
///
/// The `parts` field is stored as JSON text in `SQLite`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRow {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub parts: String,
    pub created_at: String,
}

/// A parsed message with typed parts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: Role,
    pub parts: Vec<MessagePart>,
    pub created_at: String,
}

/// A part of a message. Messages are multi-part to support interleaved
/// text, tool calls, and tool results.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MessagePart {
    /// Plain text content.
    #[serde(rename = "text")]
    Text { text: String },

    /// A tool call made by the assistant.
    #[serde(rename = "tool_call")]
    ToolCall { tool_call_id: String, name: String, arguments: String },

    /// A tool result returned to the assistant.
    #[serde(rename = "tool_result")]
    ToolResult { tool_call_id: String, content: String, is_error: bool },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_round_trip() {
        for role in [Role::System, Role::User, Role::Assistant] {
            let s = role.as_str();
            let parsed: Role = s.parse().unwrap();
            assert_eq!(parsed, role);
        }
    }

    #[test]
    fn role_invalid_string_returns_error() {
        let result: Result<Role, _> = "invalid".parse();
        assert!(result.is_err());
    }

    #[test]
    fn message_part_serialization_round_trip() {
        let parts = vec![
            MessagePart::Text { text: "hello".into() },
            MessagePart::ToolCall {
                tool_call_id: "tc_1".into(),
                name: "read".into(),
                arguments: r#"{"path":"foo.rs"}"#.into(),
            },
            MessagePart::ToolResult {
                tool_call_id: "tc_1".into(),
                content: "file contents".into(),
                is_error: false,
            },
        ];

        let json = serde_json::to_string(&parts).unwrap();
        let parsed: Vec<MessagePart> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 3);
    }
}
