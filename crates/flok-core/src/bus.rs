//! # Event Bus
//!
//! A broadcast channel for internal events. The TUI subscribes to bus events
//! to reactively update the display. The session engine, provider system, and
//! tool system all emit events onto the bus.

use tokio::sync::broadcast;

/// Events that flow through the bus.
#[derive(Debug, Clone)]
pub enum BusEvent {
    /// A session was created.
    SessionCreated { session_id: String },

    /// A message was added to a session.
    MessageCreated { session_id: String, message_id: String },

    /// A streaming text delta arrived.
    TextDelta { session_id: String, message_id: String, delta: String },

    /// A streaming reasoning/thinking delta arrived.
    ReasoningDelta { session_id: String, delta: String },

    /// Streaming completed for a message.
    StreamingComplete { session_id: String, message_id: String },

    /// Token usage update from the provider.
    TokenUsage { session_id: String, input_tokens: u64, output_tokens: u64 },

    /// A tool call is being executed.
    ToolCallStarted { session_id: String, tool_name: String, tool_call_id: String },

    /// A tool call completed.
    ToolCallCompleted {
        session_id: String,
        tool_name: String,
        tool_call_id: String,
        is_error: bool,
    },

    /// Context window usage update.
    ContextUsage { session_id: String, used_tokens: u64, max_tokens: u64 },

    /// Cost update from token tracking.
    CostUpdate { session_id: String, total_cost_usd: f64 },

    /// Compression stats update.
    CompressionStats {
        session_id: String,
        /// Number of tool results pruned by T1.
        t1_pruned: u32,
        /// Number of tool results compressed by L2.
        l2_compressed: u32,
    },

    /// A snapshot was taken (before or after tool execution).
    SnapshotCreated {
        session_id: String,
        /// The tree hash of the snapshot.
        snapshot_hash: String,
    },

    /// A snapshot was restored (undo).
    SnapshotRestored { session_id: String, snapshot_hash: String },

    /// Files changed during a tool execution step.
    SnapshotPatch {
        session_id: String,
        /// The snapshot hash this patch is relative to.
        snapshot_hash: String,
        /// Number of files that changed.
        files_changed: usize,
    },

    /// Automatic verification started after file changes.
    VerificationStarted { session_id: String, command: String },

    /// Automatic verification finished.
    VerificationCompleted { session_id: String, command: String, success: bool, summary: String },

    /// The current operation was cancelled by the user.
    Cancelled { session_id: String },

    /// A team was created.
    TeamCreated { session_id: String, team_id: String, team_name: String },

    /// A background agent's result is being injected into the lead's session.
    /// The engine's wait loop persists this as a synthetic user message.
    MessageInjected {
        /// The lead's session ID.
        session_id: String,
        /// The agent that produced this message.
        from_agent: String,
        /// The message content to inject.
        content: String,
    },

    /// A team member completed its work.
    TeamMemberCompleted { session_id: String, team_id: String, agent_name: String },

    /// A team member failed.
    TeamMemberFailed { session_id: String, team_id: String, agent_name: String, error: String },

    /// Runtime provider fallback switched providers after a retriable failure.
    ProviderFallback {
        session_id: String,
        from_provider: String,
        to_provider: String,
        reason: String,
    },

    /// A session was branched (new session created from a branch point).
    SessionBranched { parent_session_id: String, new_session_id: String, from_message_id: String },

    /// The active session was switched (via tree navigation).
    SessionSwitched { from_session_id: String, to_session_id: String },

    /// An error occurred.
    Error { message: String },
}

/// The event bus. Clone-cheap — cloning gives another handle to the same bus.
#[derive(Debug, Clone)]
pub struct Bus {
    tx: broadcast::Sender<BusEvent>,
}

impl Bus {
    /// Create a new bus with the given channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Send an event to all subscribers.
    pub fn send(&self, event: BusEvent) {
        // Ignore error — it means no subscribers are listening
        let _ = self.tx.send(event);
    }

    /// Subscribe to bus events. Returns a receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<BusEvent> {
        self.tx.subscribe()
    }
}

impl Default for Bus {
    fn default() -> Self {
        Self::new(256)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn send_and_receive_event() {
        let bus = Bus::new(16);
        let mut rx = bus.subscribe();

        bus.send(BusEvent::SessionCreated { session_id: "test-123".into() });

        let event = rx.recv().await.unwrap();
        assert!(
            matches!(event, BusEvent::SessionCreated { session_id } if session_id == "test-123")
        );
    }

    #[tokio::test]
    async fn send_with_no_subscribers_does_not_panic() {
        let bus = Bus::new(16);
        // No subscribers — should not panic
        bus.send(BusEvent::Error { message: "test".into() });
    }
}
