//! # Agent Teams
//!
//! Infrastructure for coordinating multiple sub-agents working on a shared
//! objective. A team has a lead agent (the one that created it), member
//! agents (background sub-agents), a task board, and message channels.
//!
//! Teams enable the code-review and self-review-loop patterns where
//! specialist agents work in parallel and report findings back to a lead.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PersistedTeam {
    id: TeamId,
    name: String,
    members: Vec<String>,
    tasks: Vec<TeamTask>,
    task_counter: u64,
}

#[derive(Debug, Clone)]
struct TeamStore {
    project_root: PathBuf,
}

impl TeamStore {
    fn new(project_root: PathBuf) -> Self {
        Self { project_root }
    }

    fn load_teams(&self) -> anyhow::Result<Vec<PersistedTeam>> {
        let dir = self.teams_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut teams = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(std::ffi::OsStr::to_str) != Some("json") {
                continue;
            }

            let content = std::fs::read_to_string(path)?;
            let team: PersistedTeam = serde_json::from_str(&content)?;
            teams.push(team);
        }

        Ok(teams)
    }

    fn save(&self, team: &PersistedTeam) -> anyhow::Result<()> {
        std::fs::create_dir_all(self.teams_dir())?;
        let path = self.team_path(&team.id);
        let tmp_path = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(team)?;
        std::fs::write(&tmp_path, json)?;
        std::fs::rename(tmp_path, path)?;
        Ok(())
    }

    fn delete(&self, team_id: &str) -> anyhow::Result<()> {
        let path = self.team_path(team_id);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    fn team_path(&self, team_id: &str) -> PathBuf {
        self.teams_dir().join(format!("{team_id}.json"))
    }

    fn teams_dir(&self) -> PathBuf {
        self.project_root.join(".flok").join("teams")
    }
}

/// A task on the team's shared task board.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
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
    /// Optional JSON-backed persistence for durable orchestration state.
    store: Option<Arc<TeamStore>>,
}

impl Team {
    /// Create a new team with a lead agent inbox.
    fn new(id: TeamId, name: String, store: Option<Arc<TeamStore>>) -> Self {
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
            store,
        }
    }

    fn from_persisted(team: PersistedTeam, store: Option<Arc<TeamStore>>) -> Self {
        let mut inboxes = HashMap::new();
        let mut receivers = HashMap::new();
        for member in &team.members {
            let (tx, rx) = mpsc::unbounded_channel();
            inboxes.insert(member.clone(), tx);
            receivers.insert(member.clone(), Some(rx));
        }

        Self {
            id: team.id,
            name: team.name,
            inboxes,
            receivers: Mutex::new(receivers),
            tasks: Mutex::new(team.tasks),
            task_counter: AtomicU64::new(team.task_counter.max(1)),
            disbanded: std::sync::atomic::AtomicBool::new(false),
            store,
        }
    }

    /// Register a new member agent, creating their message inbox.
    pub async fn add_member(&mut self, agent_name: &str) -> anyhow::Result<()> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.inboxes.insert(agent_name.to_string(), tx);
        self.receivers.lock().await.insert(agent_name.to_string(), Some(rx));
        self.persist_current_state().await
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
    pub async fn create_task(
        &self,
        subject: String,
        description: String,
    ) -> anyhow::Result<TeamTask> {
        let id = format!("t{}", self.task_counter.fetch_add(1, Ordering::Relaxed));
        let task = TeamTask {
            id: id.clone(),
            subject,
            description,
            status: TaskStatus::Pending,
            owner: None,
        };
        let snapshot = {
            let mut tasks = self.tasks.lock().await;
            tasks.push(task.clone());
            self.snapshot_from_tasks(tasks.clone())
        };
        self.persist_snapshot(&snapshot)?;
        Ok(task)
    }

    /// Update a task's status and/or owner.
    pub async fn update_task(
        &self,
        task_id: &str,
        status: Option<TaskStatus>,
        owner: Option<String>,
        description: Option<String>,
    ) -> anyhow::Result<TeamTask> {
        let (task, snapshot) = {
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

            (task.clone(), self.snapshot_from_tasks(tasks.clone()))
        };

        self.persist_snapshot(&snapshot)?;
        Ok(task)
    }

    /// Get a specific task by ID.
    pub async fn get_task(&self, task_id: &str) -> Option<TeamTask> {
        self.tasks.lock().await.iter().find(|t| t.id == task_id).cloned()
    }

    /// List all tasks.
    pub async fn list_tasks(&self) -> Vec<TeamTask> {
        self.tasks.lock().await.clone()
    }

    async fn persist_current_state(&self) -> anyhow::Result<()> {
        let tasks = self.tasks.lock().await.clone();
        self.persist_snapshot(&self.snapshot_from_tasks(tasks))
    }

    fn snapshot_from_tasks(&self, tasks: Vec<TeamTask>) -> PersistedTeam {
        let mut members: Vec<_> = self.inboxes.keys().cloned().collect();
        members.sort();
        PersistedTeam {
            id: self.id.clone(),
            name: self.name.clone(),
            members,
            tasks,
            task_counter: self.task_counter.load(Ordering::Relaxed),
        }
    }

    fn persist_snapshot(&self, snapshot: &PersistedTeam) -> anyhow::Result<()> {
        if let Some(store) = &self.store {
            store.save(snapshot)?;
        }
        Ok(())
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
    store: Option<Arc<TeamStore>>,
}

impl TeamRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self { teams: Arc::new(DashMap::new()), store: None }
    }

    /// Create a registry backed by `.flok/teams` under the project root.
    pub fn new_with_project_root(project_root: PathBuf) -> anyhow::Result<Self> {
        let store = Arc::new(TeamStore::new(project_root));
        let registry = Self { teams: Arc::new(DashMap::new()), store: Some(Arc::clone(&store)) };

        for team in store.load_teams()? {
            let id = team.id.clone();
            registry
                .teams
                .insert(id, Arc::new(Team::from_persisted(team, Some(Arc::clone(&store)))));
        }

        Ok(registry)
    }

    /// Create a new team and return its ID.
    pub fn create_team(&self, name: &str) -> anyhow::Result<Arc<Team>> {
        let id = ulid::Ulid::new().to_string();
        let team = Arc::new(Team::new(id.clone(), name.to_string(), self.store.clone()));
        if let Some(store) = &self.store {
            store.save(&team.snapshot_from_tasks(Vec::new()))?;
        }
        self.teams.insert(id, Arc::clone(&team));
        Ok(team)
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
    pub fn delete(&self, team_id: &str) -> anyhow::Result<bool> {
        if let Some((_, team)) = self.teams.remove(team_id) {
            team.disbanded.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(store) = &self.store {
                store.delete(team_id)?;
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_team_and_send_message() {
        let registry = TeamRegistry::new();
        let team = registry.create_team("test-team").unwrap();

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
        let team = registry.create_team("test-team").unwrap();

        // Need mutable access to add member — get via Arc::get_mut or recreate
        // In practice, members are added before the team Arc is shared.
        // For testing, we use the registry's raw access.
        let team_id = team.id.clone();
        drop(team);

        // Add member by removing from registry, mutating, re-inserting
        let (_, mut team_owned) = registry.teams.remove(&team_id).unwrap();
        let team_mut = Arc::get_mut(&mut team_owned).unwrap();
        team_mut.add_member("reviewer-1").await.unwrap();
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
        let team = registry.create_team("review-team").unwrap();

        // Create tasks
        let t1 = team
            .create_task("Review auth module".into(), "Check for SQL injection".into())
            .await
            .unwrap();
        let t2 = team
            .create_task("Review API routes".into(), "Check error handling".into())
            .await
            .unwrap();

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
        let team = registry.create_team("ephemeral").unwrap();
        let id = team.id.clone();

        assert!(registry.get(&id).is_some());
        assert!(registry.delete(&id).unwrap());
        assert!(registry.get(&id).is_none());
        assert!(team.disbanded.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[tokio::test]
    async fn send_to_nonexistent_agent_fails() {
        let registry = TeamRegistry::new();
        let team = registry.create_team("test").unwrap();

        let result = team.send_message(TeamMessage {
            from: "lead".into(),
            to: "ghost".into(),
            content: "hello?".into(),
        });

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn persistent_registry_restores_team_board_and_members() {
        let temp = tempfile::tempdir().unwrap();
        let registry = TeamRegistry::new_with_project_root(temp.path().to_path_buf()).unwrap();
        let team = registry.create_team("review-team").unwrap();
        let team_id = team.id.clone();
        drop(team);

        let (_, mut team_owned) = registry.teams.remove(&team_id).unwrap();
        let team_mut = Arc::get_mut(&mut team_owned).unwrap();
        team_mut.add_member("reviewer-1").await.unwrap();
        registry.teams.insert(team_id.clone(), team_owned);

        let team = registry.get(&team_id).unwrap();
        team.create_task("Review auth".into(), "Look for regressions".into()).await.unwrap();
        team.update_task("t1", Some(TaskStatus::Completed), Some("reviewer-1".into()), None)
            .await
            .unwrap();

        let reloaded = TeamRegistry::new_with_project_root(temp.path().to_path_buf()).unwrap();
        let restored = reloaded.get(&team_id).unwrap();
        let restored_task = restored.get_task("t1").await.unwrap();

        assert_eq!(restored_task.status, TaskStatus::Completed);
        assert_eq!(restored_task.owner.as_deref(), Some("reviewer-1"));

        restored
            .send_message(TeamMessage {
                from: "lead".into(),
                to: "reviewer-1".into(),
                content: "ping".into(),
            })
            .unwrap();
        let msgs = restored.drain_messages("reviewer-1").await;
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "ping");
    }

    #[test]
    fn persistent_delete_removes_team_file() {
        let temp = tempfile::tempdir().unwrap();
        let registry = TeamRegistry::new_with_project_root(temp.path().to_path_buf()).unwrap();
        let team = registry.create_team("ephemeral").unwrap();
        let team_id = team.id.clone();
        let team_path = temp.path().join(".flok").join("teams").join(format!("{team_id}.json"));

        assert!(team_path.exists());
        assert!(registry.delete(&team_id).unwrap());
        assert!(!team_path.exists());
    }
}
