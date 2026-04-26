//! Typed execution plans persisted in the workspace.
//!
//! Plans are stored under flok's generated per-project state directory so they can be
//! reviewed, diffed, resumed, and executed later.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// Unique identifier for an execution plan.
pub type PlanId = String;

/// Unique identifier for a step within a plan.
pub type StepId = String;

/// A dependency edge in the execution DAG.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Dependency {
    pub prerequisite: StepId,
    pub dependent: StepId,
}

/// Persisted execution plan.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionPlan {
    pub id: PlanId,
    pub session_id: String,
    pub title: String,
    pub description: String,
    pub steps: Vec<PlanStep>,
    pub dependencies: Vec<Dependency>,
    pub status: PlanStatus,
    #[serde(default)]
    pub active_run_id: Option<String>,
    #[serde(default)]
    pub runs: Vec<PlanRun>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A single unit of work in an execution plan.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanStep {
    pub id: StepId,
    pub title: String,
    pub description: String,
    pub affected_files: Vec<PathBuf>,
    #[serde(default)]
    pub planned_file_hashes: Vec<PlanFileHash>,
    pub agent_type: String,
    pub estimated_tokens: Option<u64>,
    pub status: StepStatus,
    pub checkpoint: Option<Checkpoint>,
}

/// File content fingerprint captured when a plan is created.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanFileHash {
    pub path: PathBuf,
    pub hash: Option<String>,
    pub existed: bool,
}

/// Top-level plan lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Draft,
    Approved,
    Executing,
    Paused,
    Completed,
    Failed,
    Cancelled,
    RolledBack,
}

/// Per-step lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Running,
    Completed,
    Failed(String),
    Skipped,
    RolledBack,
}

/// Durable runtime state for an approved plan execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanRun {
    pub id: String,
    pub status: PlanRunStatus,
    pub steps: Vec<PlanRunStep>,
    pub resume_point: Option<PlanResumePoint>,
    pub failure: Option<PlanFailure>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Durable plan execution lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanRunStatus {
    Draft,
    Approved,
    Executing,
    Paused,
    Failed,
    Completed,
    Cancelled,
    RolledBack,
}

/// Durable execution metadata for a single step within a run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanRunStep {
    pub step_id: StepId,
    pub status: StepStatus,
    pub checkpoint: Option<Checkpoint>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub retry_count: u32,
    pub failure: Option<PlanFailure>,
}

/// The current durable point from which a run can be resumed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanResumePoint {
    pub ready_step_ids: Vec<StepId>,
    pub blocked_step_ids: Vec<StepId>,
}

/// Structured failure data stored with plan runs and run steps.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanFailure {
    pub step_id: Option<StepId>,
    pub reason: String,
    pub recorded_at: DateTime<Utc>,
}

/// File that changed after the plan was created.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StalePlanFile {
    pub path: PathBuf,
    pub planned_hash: Option<String>,
    pub current_hash: Option<String>,
    pub planned_existed: bool,
    pub current_existed: bool,
}

/// Rollback checkpoint captured for a step.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Checkpoint {
    pub step_id: StepId,
    pub snapshot: CheckpointData,
    pub created_at: DateTime<Utc>,
}

/// Snapshot payload used for future rollback execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CheckpointData {
    WorkspaceSnapshot { hash: String },
    FileSnapshots(Vec<FileSnapshot>),
}

/// Raw file snapshot fallback.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileSnapshot {
    pub path: PathBuf,
    pub content: Vec<u8>,
    pub existed: bool,
}

/// User/tool supplied data used to create a new plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewExecutionPlan {
    pub session_id: String,
    pub title: String,
    pub description: String,
    pub steps: Vec<NewPlanStep>,
    pub dependencies: Vec<Dependency>,
}

/// User/tool supplied data used to create a plan step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPlanStep {
    pub id: Option<StepId>,
    pub title: String,
    pub description: String,
    pub affected_files: Vec<PathBuf>,
    pub agent_type: String,
    pub estimated_tokens: Option<u64>,
}

/// Mutations supported by the initial plan update flow.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanPatch {
    pub plan_status: Option<PlanStatus>,
    pub step_id: Option<StepId>,
    pub step_status: Option<StepStatus>,
    pub checkpoint: Option<Checkpoint>,
    pub run_status: Option<PlanRunStatus>,
    pub failure: Option<PlanFailure>,
}

/// Errors returned by plan persistence and validation.
#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("plan '{0}' not found")]
    NotFound(String),
    #[error("plan validation failed: {0}")]
    Validation(String),
    #[error("plan io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("plan json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Store for JSON-backed execution plans.
#[derive(Debug, Clone)]
pub struct PlanStore {
    project_root: PathBuf,
}

impl PlanStore {
    /// Create a store rooted at the given project root.
    #[must_use]
    pub fn new(project_root: PathBuf) -> Self {
        Self { project_root }
    }

    /// Create and persist a new plan.
    pub fn create_plan(&self, new_plan: NewExecutionPlan) -> Result<ExecutionPlan, PlanError> {
        let now = Utc::now();
        let steps = new_plan
            .steps
            .into_iter()
            .map(|step| PlanStep {
                planned_file_hashes: step
                    .affected_files
                    .iter()
                    .map(|path| fingerprint_file(&self.project_root, path))
                    .collect(),
                id: step.id.unwrap_or_else(|| Ulid::new().to_string()),
                title: step.title,
                description: step.description,
                affected_files: step.affected_files,
                agent_type: step.agent_type,
                estimated_tokens: step.estimated_tokens,
                status: StepStatus::Pending,
                checkpoint: None,
            })
            .collect();

        let plan = ExecutionPlan {
            id: Ulid::new().to_string(),
            session_id: new_plan.session_id,
            title: new_plan.title,
            description: new_plan.description,
            steps,
            dependencies: new_plan.dependencies,
            status: PlanStatus::Draft,
            active_run_id: None,
            runs: Vec::new(),
            created_at: now,
            updated_at: now,
        };

        validate_plan(&plan)?;
        self.save_plan(&plan)?;
        Ok(plan)
    }

    /// Persist an existing plan.
    pub fn save_plan(&self, plan: &ExecutionPlan) -> Result<(), PlanError> {
        validate_plan(plan)?;
        std::fs::create_dir_all(self.plans_dir())?;
        let path = self.plan_path(&plan.id);
        let tmp_path = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(plan)?;
        std::fs::write(&tmp_path, json)?;
        std::fs::rename(tmp_path, path)?;
        Ok(())
    }

    /// Load a plan by ID.
    pub fn load_plan(&self, plan_id: &str) -> Result<ExecutionPlan, PlanError> {
        let path = self.plan_path(plan_id);
        if !path.exists() {
            return Err(PlanError::NotFound(plan_id.to_string()));
        }
        let content = std::fs::read_to_string(path)?;
        let plan: ExecutionPlan = serde_json::from_str(&content)?;
        validate_plan(&plan)?;
        Ok(plan)
    }

    /// List plans sorted by most recently updated first.
    pub fn list_plans(&self) -> Result<Vec<ExecutionPlan>, PlanError> {
        let dir = self.plans_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut plans = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(std::ffi::OsStr::to_str) != Some("json") {
                continue;
            }
            let content = std::fs::read_to_string(&path)?;
            let plan: ExecutionPlan = serde_json::from_str(&content)?;
            if validate_plan(&plan).is_ok() {
                plans.push(plan);
            }
        }

        plans.sort_by_key(|plan| std::cmp::Reverse(plan.updated_at));
        Ok(plans)
    }

    /// Apply a status/checkpoint patch to an existing plan.
    pub fn apply_patch(&self, plan_id: &str, patch: PlanPatch) -> Result<ExecutionPlan, PlanError> {
        let mut plan = self.load_plan(plan_id)?;

        if let Some(plan_status) = patch.plan_status {
            plan.status = plan_status;
        }

        if let Some(run_status) = patch.run_status {
            if let Some(run) = plan.active_run_mut() {
                if matches!(
                    run_status,
                    PlanRunStatus::Completed | PlanRunStatus::Cancelled | PlanRunStatus::RolledBack
                ) {
                    run.completed_at = Some(Utc::now());
                }
                run.status = run_status;
                run.updated_at = Utc::now();
            }
        }

        if patch.step_status.is_some() || patch.checkpoint.is_some() {
            let Some(step_id) = patch.step_id else {
                return Err(PlanError::Validation(
                    "step_id is required when updating a step".to_string(),
                ));
            };

            let (updated_status, updated_checkpoint) = {
                let step =
                    plan.steps.iter_mut().find(|step| step.id == step_id).ok_or_else(|| {
                        PlanError::Validation(format!("unknown step '{step_id}'"))
                    })?;

                if let Some(step_status) = patch.step_status {
                    step.status = step_status;
                }
                if let Some(checkpoint) = patch.checkpoint {
                    if checkpoint.step_id != step.id {
                        return Err(PlanError::Validation(format!(
                            "checkpoint step_id '{}' does not match target step '{}'",
                            checkpoint.step_id, step.id
                        )));
                    }
                    step.checkpoint = Some(checkpoint);
                }

                (step.status.clone(), step.checkpoint.clone())
            };

            if let Some(run_step) = plan.active_run_step_mut(&step_id) {
                run_step.status = updated_status.clone();
                match &updated_status {
                    StepStatus::Running => {
                        if run_step.started_at.is_none() {
                            run_step.started_at = Some(Utc::now());
                        }
                    }
                    StepStatus::Completed => {
                        run_step.completed_at = Some(Utc::now());
                        run_step.failure = None;
                    }
                    StepStatus::Failed(reason) => {
                        run_step.failure = Some(PlanFailure {
                            step_id: Some(step_id.clone()),
                            reason: reason.clone(),
                            recorded_at: Utc::now(),
                        });
                    }
                    StepStatus::Pending | StepStatus::Skipped | StepStatus::RolledBack => {}
                }
                if let Some(checkpoint) = updated_checkpoint {
                    run_step.checkpoint = Some(checkpoint);
                }
            }
        }

        if let Some(failure) = patch.failure {
            if let Some(run) = plan.active_run_mut() {
                run.failure = Some(failure);
                run.updated_at = Utc::now();
            }
        }

        plan.refresh_active_resume_point();

        plan.updated_at = Utc::now();
        self.save_plan(&plan)?;
        Ok(plan)
    }

    /// Start a new durable run or return the current active non-terminal run.
    pub fn ensure_active_run(&self, plan_id: &str) -> Result<ExecutionPlan, PlanError> {
        let mut plan = self.load_plan(plan_id)?;
        plan.ensure_active_run();
        plan.updated_at = Utc::now();
        self.save_plan(&plan)?;
        Ok(plan)
    }

    /// Return affected files whose content differs from the plan-created fingerprint.
    pub fn stale_files_for_step(&self, step: &PlanStep) -> Vec<StalePlanFile> {
        step.planned_file_hashes
            .iter()
            .filter_map(|planned| {
                let current = fingerprint_file(&self.project_root, &planned.path);
                (planned.hash != current.hash || planned.existed != current.existed).then_some(
                    StalePlanFile {
                        path: planned.path.clone(),
                        planned_hash: planned.hash.clone(),
                        current_hash: current.hash,
                        planned_existed: planned.existed,
                        current_existed: current.existed,
                    },
                )
            })
            .collect()
    }

    /// Absolute path for a persisted plan file.
    #[must_use]
    pub fn plan_path(&self, plan_id: &str) -> PathBuf {
        self.plans_dir().join(format!("{plan_id}.json"))
    }

    fn plans_dir(&self) -> PathBuf {
        crate::config::project_state_dir(&self.project_root).join("plans")
    }
}

impl ExecutionPlan {
    /// Ensure this plan has an active durable run and return its ID.
    pub fn ensure_active_run(&mut self) -> String {
        if let Some(active_run_id) = &self.active_run_id {
            if self.runs.iter().any(|run| {
                run.id == *active_run_id
                    && !matches!(
                        run.status,
                        PlanRunStatus::Completed
                            | PlanRunStatus::Failed
                            | PlanRunStatus::Cancelled
                            | PlanRunStatus::RolledBack
                    )
            }) {
                return active_run_id.clone();
            }
        }

        let now = Utc::now();
        let run_id = Ulid::new().to_string();
        let run = PlanRun {
            id: run_id.clone(),
            status: PlanRunStatus::Approved,
            steps: self
                .steps
                .iter()
                .map(|step| PlanRunStep {
                    step_id: step.id.clone(),
                    status: step.status.clone(),
                    checkpoint: step.checkpoint.clone(),
                    started_at: None,
                    completed_at: None,
                    retry_count: 0,
                    failure: None,
                })
                .collect(),
            resume_point: None,
            failure: None,
            created_at: now,
            updated_at: now,
            started_at: None,
            completed_at: None,
        };

        self.active_run_id = Some(run_id.clone());
        self.runs.push(run);
        self.refresh_active_resume_point();
        run_id
    }

    fn active_run_mut(&mut self) -> Option<&mut PlanRun> {
        let active_run_id = self.active_run_id.as_ref()?;
        self.runs.iter_mut().find(|run| run.id == *active_run_id)
    }

    fn active_run_step_mut(&mut self, step_id: &str) -> Option<&mut PlanRunStep> {
        self.active_run_mut()?.steps.iter_mut().find(|run_step| run_step.step_id == step_id)
    }

    fn refresh_active_resume_point(&mut self) {
        let ready_step_ids = ready_step_ids(self);
        let blocked_step_ids = blocked_step_ids(self);
        if let Some(run) = self.active_run_mut() {
            run.resume_point = Some(PlanResumePoint { ready_step_ids, blocked_step_ids });
            if matches!(run.status, PlanRunStatus::Executing) && run.started_at.is_none() {
                run.started_at = Some(Utc::now());
            }
            run.updated_at = Utc::now();
        }
    }
}

/// Return the pending step IDs whose dependencies are already complete.
#[must_use]
pub fn ready_step_ids(plan: &ExecutionPlan) -> Vec<StepId> {
    plan.steps
        .iter()
        .filter(|step| matches!(step.status, StepStatus::Pending))
        .filter(|step| dependencies_completed(plan, &step.id))
        .map(|step| step.id.clone())
        .collect()
}

fn blocked_step_ids(plan: &ExecutionPlan) -> Vec<StepId> {
    plan.steps
        .iter()
        .filter(|step| matches!(step.status, StepStatus::Pending))
        .filter(|step| !dependencies_completed(plan, &step.id))
        .map(|step| step.id.clone())
        .collect()
}

fn dependencies_completed(plan: &ExecutionPlan, step_id: &str) -> bool {
    plan.dependencies.iter().filter(|dependency| dependency.dependent == step_id).all(
        |dependency| {
            plan.steps
                .iter()
                .find(|candidate| candidate.id == dependency.prerequisite)
                .is_some_and(|candidate| matches!(candidate.status, StepStatus::Completed))
        },
    )
}

fn fingerprint_file(project_root: &std::path::Path, path: &std::path::Path) -> PlanFileHash {
    let full_path = if path.is_absolute() { path.to_path_buf() } else { project_root.join(path) };

    match std::fs::read(&full_path) {
        Ok(content) => PlanFileHash {
            path: path.to_path_buf(),
            hash: Some(blake3::hash(&content).to_hex().to_string()),
            existed: true,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            PlanFileHash { path: path.to_path_buf(), hash: None, existed: false }
        }
        Err(_) => PlanFileHash { path: path.to_path_buf(), hash: None, existed: false },
    }
}

fn validate_plan(plan: &ExecutionPlan) -> Result<(), PlanError> {
    if plan.title.trim().is_empty() {
        return Err(PlanError::Validation("title must not be empty".to_string()));
    }
    if plan.steps.is_empty() {
        return Err(PlanError::Validation("plan must contain at least one step".to_string()));
    }

    let mut seen_step_ids = HashSet::new();
    for step in &plan.steps {
        if step.title.trim().is_empty() {
            return Err(PlanError::Validation(format!("step '{}' must have a title", step.id)));
        }
        if step.agent_type.trim().is_empty() {
            return Err(PlanError::Validation(format!(
                "step '{}' must have an agent_type",
                step.id
            )));
        }
        if !seen_step_ids.insert(step.id.clone()) {
            return Err(PlanError::Validation(format!("duplicate step id '{}'", step.id)));
        }
    }

    let mut indegree: HashMap<&str, usize> =
        plan.steps.iter().map(|step| (step.id.as_str(), 0usize)).collect();
    let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();

    for dependency in &plan.dependencies {
        if !indegree.contains_key(dependency.prerequisite.as_str()) {
            return Err(PlanError::Validation(format!(
                "dependency references unknown prerequisite step '{}'",
                dependency.prerequisite
            )));
        }
        if !indegree.contains_key(dependency.dependent.as_str()) {
            return Err(PlanError::Validation(format!(
                "dependency references unknown dependent step '{}'",
                dependency.dependent
            )));
        }
        if dependency.prerequisite == dependency.dependent {
            return Err(PlanError::Validation(format!(
                "step '{}' cannot depend on itself",
                dependency.prerequisite
            )));
        }

        *indegree
            .get_mut(dependency.dependent.as_str())
            .expect("validated dependent key exists") += 1;
        edges
            .entry(dependency.prerequisite.as_str())
            .or_default()
            .push(dependency.dependent.as_str());
    }

    let mut queue: VecDeque<&str> = indegree
        .iter()
        .filter_map(|(step_id, degree)| (*degree == 0).then_some(*step_id))
        .collect();
    let mut visited = 0usize;

    while let Some(step_id) = queue.pop_front() {
        visited += 1;
        if let Some(children) = edges.get(step_id) {
            for child in children {
                if let Some(degree) = indegree.get_mut(child) {
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(child);
                    }
                }
            }
        }
    }

    if visited != plan.steps.len() {
        return Err(PlanError::Validation(
            "dependencies contain a cycle; plan must be a DAG".to_string(),
        ));
    }

    Ok(())
}

/// Render a compact human-readable plan summary.
#[must_use]
pub fn summarize_plan(plan: &ExecutionPlan) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out, "Plan {} [{}]", plan.id, plan_status_label(&plan.status));
    let _ = writeln!(out, "Title: {}", plan.title);
    if !plan.description.trim().is_empty() {
        let _ = writeln!(out, "Description: {}", plan.description);
    }
    let _ = writeln!(out, "Steps: {}", plan.steps.len());
    for (idx, step) in plan.steps.iter().enumerate() {
        let files = if step.affected_files.is_empty() {
            "-".to_string()
        } else {
            step.affected_files
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let _ = writeln!(
            out,
            "{}. {} [{}] agent={} files={}",
            idx + 1,
            step.title,
            step_status_label(&step.status),
            step.agent_type,
            files
        );
    }
    out.trim_end().to_string()
}

fn plan_status_label(status: &PlanStatus) -> &'static str {
    match status {
        PlanStatus::Draft => "draft",
        PlanStatus::Approved => "approved",
        PlanStatus::Executing => "executing",
        PlanStatus::Paused => "paused",
        PlanStatus::Completed => "completed",
        PlanStatus::Failed => "failed",
        PlanStatus::Cancelled => "cancelled",
        PlanStatus::RolledBack => "rolled_back",
    }
}

fn step_status_label(status: &StepStatus) -> &'static str {
    match status {
        StepStatus::Pending => "pending",
        StepStatus::Running => "running",
        StepStatus::Completed => "completed",
        StepStatus::Failed(_) => "failed",
        StepStatus::Skipped => "skipped",
        StepStatus::RolledBack => "rolled_back",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, PlanStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = PlanStore::new(dir.path().to_path_buf());
        (dir, store)
    }

    fn sample_plan() -> NewExecutionPlan {
        NewExecutionPlan {
            session_id: "session-1".to_string(),
            title: "Refactor auth".to_string(),
            description: "Move auth verification behind a JWT abstraction.".to_string(),
            steps: vec![
                NewPlanStep {
                    id: Some("step-a".to_string()),
                    title: "Add JWT module".to_string(),
                    description: "Create auth/jwt.rs".to_string(),
                    affected_files: vec![PathBuf::from("src/auth/jwt.rs")],
                    agent_type: "build".to_string(),
                    estimated_tokens: Some(2000),
                },
                NewPlanStep {
                    id: Some("step-b".to_string()),
                    title: "Wire middleware".to_string(),
                    description: "Switch middleware to use the new verifier".to_string(),
                    affected_files: vec![PathBuf::from("src/middleware/auth.rs")],
                    agent_type: "build".to_string(),
                    estimated_tokens: Some(1500),
                },
            ],
            dependencies: vec![Dependency {
                prerequisite: "step-a".to_string(),
                dependent: "step-b".to_string(),
            }],
        }
    }

    #[test]
    fn create_and_load_plan_round_trips() {
        let (_dir, store) = store();
        let plan = store.create_plan(sample_plan()).expect("create plan");
        let loaded = store.load_plan(&plan.id).expect("load plan");
        assert_eq!(loaded.title, "Refactor auth");
        assert_eq!(loaded.steps.len(), 2);
        assert_eq!(loaded.dependencies.len(), 1);
        assert!(store.plan_path(&plan.id).exists());
    }

    #[test]
    fn list_plans_returns_saved_plans() {
        let (_dir, store) = store();
        let first = store.create_plan(sample_plan()).expect("first");
        let second = store
            .create_plan(NewExecutionPlan { title: "Second plan".to_string(), ..sample_plan() })
            .expect("second");
        let plans = store.list_plans().expect("list");
        assert_eq!(plans.len(), 2);
        let ids: Vec<String> = plans.into_iter().map(|plan| plan.id).collect();
        assert!(ids.contains(&first.id));
        assert!(ids.contains(&second.id));
    }

    #[test]
    fn apply_patch_updates_step_status_and_plan_status() {
        let (_dir, store) = store();
        let plan = store.create_plan(sample_plan()).expect("create");
        let plan = store.ensure_active_run(&plan.id).expect("run");
        let updated = store
            .apply_patch(
                &plan.id,
                PlanPatch {
                    plan_status: Some(PlanStatus::Executing),
                    step_id: Some("step-a".to_string()),
                    step_status: Some(StepStatus::Completed),
                    checkpoint: None,
                    ..PlanPatch::default()
                },
            )
            .expect("patch");

        assert_eq!(updated.status, PlanStatus::Executing);
        assert!(matches!(updated.steps[0].status, StepStatus::Completed));
        let run = updated.runs.first().expect("run persisted");
        assert!(matches!(run.steps[0].status, StepStatus::Completed));
    }

    #[test]
    fn ensure_active_run_persists_resume_metadata() {
        let (_dir, store) = store();
        let plan = store.create_plan(sample_plan()).expect("create");
        let with_run = store.ensure_active_run(&plan.id).expect("run");

        assert!(with_run.active_run_id.is_some());
        assert_eq!(with_run.runs.len(), 1);
        let run = with_run.runs.first().expect("run");
        assert_eq!(run.steps.len(), 2);
        assert_eq!(
            run.resume_point.as_ref().expect("resume point").ready_step_ids,
            vec!["step-a".to_string()]
        );
        assert_eq!(
            run.resume_point.as_ref().expect("resume point").blocked_step_ids,
            vec!["step-b".to_string()]
        );
    }

    #[test]
    fn stale_files_for_step_detects_changed_planned_files() {
        let (dir, store) = store();
        std::fs::create_dir_all(dir.path().join("src/auth")).expect("mkdir");
        std::fs::write(dir.path().join("src/auth/jwt.rs"), "old").expect("write old");

        let plan = store
            .create_plan(NewExecutionPlan {
                steps: vec![NewPlanStep {
                    id: Some("step-a".to_string()),
                    title: "Edit JWT module".to_string(),
                    description: "Change auth/jwt.rs".to_string(),
                    affected_files: vec![PathBuf::from("src/auth/jwt.rs")],
                    agent_type: "build".to_string(),
                    estimated_tokens: None,
                }],
                dependencies: Vec::new(),
                ..sample_plan()
            })
            .expect("create");

        std::fs::write(dir.path().join("src/auth/jwt.rs"), "new").expect("write new");
        let stale = store.stale_files_for_step(&plan.steps[0]);

        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].path, PathBuf::from("src/auth/jwt.rs"));
        assert_ne!(stale[0].planned_hash, stale[0].current_hash);
    }

    #[test]
    fn create_plan_rejects_cycles() {
        let (_dir, store) = store();
        let err = store
            .create_plan(NewExecutionPlan {
                dependencies: vec![
                    Dependency {
                        prerequisite: "step-a".to_string(),
                        dependent: "step-b".to_string(),
                    },
                    Dependency {
                        prerequisite: "step-b".to_string(),
                        dependent: "step-a".to_string(),
                    },
                ],
                ..sample_plan()
            })
            .expect_err("cycle should fail");

        assert!(matches!(err, PlanError::Validation(msg) if msg.contains("cycle")));
    }

    #[test]
    fn summarize_plan_lists_steps() {
        let (_dir, store) = store();
        let plan = store.create_plan(sample_plan()).expect("create");
        let summary = summarize_plan(&plan);
        assert!(summary.contains("Refactor auth"));
        assert!(summary.contains("Add JWT module"));
        assert!(summary.contains("pending"));
    }
}
