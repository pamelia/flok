//! # Agent Teams
//!
//! Infrastructure for coordinating multiple sub-agents working on a shared
//! objective. A team has a lead agent (the one that created it), member
//! agents (background sub-agents), a task board, and message channels.
//!
//! Teams enable the code-review and self-review-loop patterns where
//! specialist agents work in parallel and report findings back to a lead.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{mpsc, Mutex};

// ---------------------------------------------------------------------------
// Team types
// ---------------------------------------------------------------------------

/// A unique team identifier.
pub type TeamId = String;

/// A unique task identifier within a team.
pub type TaskId = String;

/// A message sent between team members.
#[derive(Debug, Clone)]
pub struct TeamMessage {
    /// Who sent this message.
    pub from: String,
    /// Who should receive it ("lead" or a specific agent name).
    pub to: String,
    /// The message content.
    pub content: String,
}

/// A task on the team's shared task board.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TeamTask {
    /// Unique task ID (e.g., "t1", "t2").
    pub id: TaskId,
    /// Short subject line.
    pub subject: String,
    /// Detailed description.
    pub description: String,
    /// Current status.
    pub status: TaskStatus,
    /// Agent name that owns this task (if assigned).
    pub owner: Option<String>,
}

/// Status of a team task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::InProgress => write!(f, "in_progress"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

/// A team of coordinating agents.
pub struct Team {
    /// Team identifier.
    pub id: TeamId,
    /// Human-readable team name.
    pub name: String,
    /// Message inboxes keyed by agent name. Each agent polls its inbox.
    /// The lead agent's inbox is keyed as "lead".
    inboxes: HashMap<String, mpsc::UnboundedSender<TeamMessage>>,
    /// Receivers stored so agents can claim them. Once taken, the entry is None.
    receivers: Mutex<HashMap<String, Option<mpsc::UnboundedReceiver<TeamMessage>>>>,
    /// Shared task board.
    tasks: Mutex<Vec<TeamTask>>,
    /// Counter for generating task IDs.
    task_counter: AtomicU64,
    /// Whether this team has been disbanded.
    pub disbanded: std::sync::atomic::AtomicBool,
}

impl Team {
    /// Create a new team with a lead agent inbox.
    fn new(id: TeamId, name: String) -> Self {
        let (lead_tx, lead_rx) = mpsc::unbounded_channel();
        let mut inboxes = HashMap::new();
        inboxes.insert("lead".to_string(), lead_tx);
        let mut receivers = HashMap::new();
        receivers.insert("lead".to_string(), Some(lead_rx));

        Self {
            id,
            name,
            inboxes,
            receivers: Mutex::new(receivers),
            tasks: Mutex::new(Vec::new()),
            task_counter: AtomicU64::new(1),
            disbanded: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Register a new member agent, creating their message inbox.
    pub async fn add_member(&mut self, agent_name: &str) {
        let (tx, rx) = mpsc::unbounded_channel();
        self.inboxes.insert(agent_name.to_string(), tx);
        self.receivers.lock().await.insert(agent_name.to_string(), Some(rx));
    }

    /// Send a message to an agent in this team.
    ///
    /// Returns an error if the recipient doesn't exist or the channel is closed.
    pub fn send_message(&self, msg: TeamMessage) -> anyhow::Result<()> {
        let tx = self
            .inboxes
            .get(&msg.to)
            .ok_or_else(|| anyhow::anyhow!("no agent '{}' in team '{}'", msg.to, self.name))?;
        tx.send(msg).map_err(|_| anyhow::anyhow!("message channel closed for agent"))
    }

    /// Take the message receiver for an agent (can only be taken once).
    pub async fn take_receiver(
        &self,
        agent_name: &str,
    ) -> Option<mpsc::UnboundedReceiver<TeamMessage>> {
        self.receivers.lock().await.get_mut(agent_name)?.take()
    }

    /// Receive all pending messages for an agent (non-blocking drain).
    pub async fn drain_messages(&self, agent_name: &str) -> Vec<TeamMessage> {
        let mut guard = self.receivers.lock().await;
        let Some(Some(rx)) = guard.get_mut(agent_name) else {
            return Vec::new();
        };
        let mut msgs = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            msgs.push(msg);
        }
        msgs
    }

    /// Create a new task on the task board.
    pub async fn create_task(&self, subject: String, description: String) -> TeamTask {
        let id = format!("t{}", self.task_counter.fetch_add(1, Ordering::Relaxed));
        let task = TeamTask {
            id: id.clone(),
            subject,
            description,
            status: TaskStatus::Pending,
            owner: None,
        };
        self.tasks.lock().await.push(task.clone());
        task
    }

    /// Update a task's status and/or owner.
    pub async fn update_task(
        &self,
        task_id: &str,
        status: Option<TaskStatus>,
        owner: Option<String>,
        description: Option<String>,
    ) -> anyhow::Result<TeamTask> {
        let mut tasks = self.tasks.lock().await;
        let task = tasks
            .iter_mut()
            .find(|t| t.id == task_id)
            .ok_or_else(|| anyhow::anyhow!("task '{task_id}' not found"))?;

        if let Some(s) = status {
            task.status = s;
        }
        if let Some(o) = owner {
            task.owner = Some(o);
        }
        if let Some(d) = description {
            task.description = d;
        }

        Ok(task.clone())
    }

    /// Get a specific task by ID.
    pub async fn get_task(&self, task_id: &str) -> Option<TeamTask> {
        self.tasks.lock().await.iter().find(|t| t.id == task_id).cloned()
    }

    /// List all tasks.
    pub async fn list_tasks(&self) -> Vec<TeamTask> {
        self.tasks.lock().await.clone()
    }
}

impl std::fmt::Debug for Team {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Team")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("members", &self.inboxes.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Team registry
// ---------------------------------------------------------------------------

/// Global registry of active teams.
///
/// Shared across all tools and the session engine. Teams are created by
/// the `team_create` tool and disbanded by `team_delete`.
#[derive(Debug, Clone, Default)]
pub struct TeamRegistry {
    teams: Arc<DashMap<TeamId, Arc<Team>>>,
}

impl TeamRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self { teams: Arc::new(DashMap::new()) }
    }

    /// Create a new team and return its ID.
    pub fn create_team(&self, name: &str) -> Arc<Team> {
        let id = ulid::Ulid::new().to_string();
        let team = Arc::new(Team::new(id.clone(), name.to_string()));
        self.teams.insert(id, Arc::clone(&team));
        team
    }

    /// Get a team by ID.
    pub fn get(&self, team_id: &str) -> Option<Arc<Team>> {
        self.teams.get(team_id).map(|r| Arc::clone(r.value()))
    }

    /// Access the underlying teams map for mutation (e.g., adding members).
    ///
    /// This is needed because `Arc<Team>` requires exclusive access for
    /// mutation. The pattern is: remove from map, mutate, re-insert.
    pub fn teams_mut(&self) -> &DashMap<TeamId, Arc<Team>> {
        &self.teams
    }

    /// Re-insert a team after mutation.
    pub fn reinsert(&self, team_id: TeamId, team: Arc<Team>) {
        self.teams.insert(team_id, team);
    }

    /// Delete (disband) a team.
    pub fn delete(&self, team_id: &str) -> bool {
        if let Some((_, team)) = self.teams.remove(team_id) {
            team.disbanded.store(true, std::sync::atomic::Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_team_and_send_message() {
        let registry = TeamRegistry::new();
        let team = registry.create_team("test-team");

        // Send a message to the lead
        team.send_message(TeamMessage {
            from: "agent-1".into(),
            to: "lead".into(),
            content: "hello from agent-1".into(),
        })
        .unwrap();

        // Lead drains messages
        let msgs = team.drain_messages("lead").await;
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "hello from agent-1");
    }

    #[tokio::test]
    async fn add_member_and_message() {
        let registry = TeamRegistry::new();
        let team = registry.create_team("test-team");

        // Need mutable access to add member — get via Arc::get_mut or recreate
        // In practice, members are added before the team Arc is shared.
        // For testing, we use the registry's raw access.
        let team_id = team.id.clone();
        drop(team);

        // Add member by removing from registry, mutating, re-inserting
        let (_, mut team_owned) = registry.teams.remove(&team_id).unwrap();
        let team_mut = Arc::get_mut(&mut team_owned).unwrap();
        team_mut.add_member("reviewer-1").await;
        registry.teams.insert(team_id.clone(), team_owned);

        let team = registry.get(&team_id).unwrap();

        // Send to member
        team.send_message(TeamMessage {
            from: "lead".into(),
            to: "reviewer-1".into(),
            content: "please review PR #42".into(),
        })
        .unwrap();

        let msgs = team.drain_messages("reviewer-1").await;
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "please review PR #42");
    }

    #[tokio::test]
    async fn task_board_crud() {
        let registry = TeamRegistry::new();
        let team = registry.create_team("review-team");

        // Create tasks
        let t1 =
            team.create_task("Review auth module".into(), "Check for SQL injection".into()).await;
        let t2 = team.create_task("Review API routes".into(), "Check error handling".into()).await;

        assert_eq!(t1.id, "t1");
        assert_eq!(t2.id, "t2");
        assert_eq!(t1.status, TaskStatus::Pending);

        // Update task
        let updated = team
            .update_task(&t1.id, Some(TaskStatus::InProgress), Some("reviewer-1".into()), None)
            .await
            .unwrap();
        assert_eq!(updated.status, TaskStatus::InProgress);
        assert_eq!(updated.owner.as_deref(), Some("reviewer-1"));

        // List tasks
        let all = team.list_tasks().await;
        assert_eq!(all.len(), 2);

        // Get specific task
        let fetched = team.get_task("t2").await.unwrap();
        assert_eq!(fetched.subject, "Review API routes");
    }

    #[test]
    fn delete_team() {
        let registry = TeamRegistry::new();
        let team = registry.create_team("ephemeral");
        let id = team.id.clone();

        assert!(registry.get(&id).is_some());
        assert!(registry.delete(&id));
        assert!(registry.get(&id).is_none());
        assert!(team.disbanded.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[tokio::test]
    async fn send_to_nonexistent_agent_fails() {
        let registry = TeamRegistry::new();
        let team = registry.create_team("test");

        let result = team.send_message(TeamMessage {
            from: "lead".into(),
            to: "ghost".into(),
            content: "hello?".into(),
        });

        assert!(result.is_err());
    }
}
