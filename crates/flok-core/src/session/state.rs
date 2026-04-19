//! Application state — the top-level container that holds everything together.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use flok_db::Db;

use crate::bus::Bus;
use crate::config::FlokConfig;
use crate::lsp::LspManager;
use crate::provider::{Provider, ProviderRegistry};
use crate::snapshot::SnapshotManager;
use crate::token::CostTracker;
use crate::tool::{PermissionManager, ToolContext, ToolRegistry};

/// Shared flag for plan/build mode. `true` = plan mode (read-only).
///
/// Wrapped in `Arc` so both TUI and engine can read/write it.
#[derive(Debug, Clone)]
pub struct PlanMode(Arc<AtomicBool>);

impl PlanMode {
    /// Create a new plan mode flag, starting in build mode.
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    /// Whether plan mode is active (read-only).
    pub fn is_plan(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Toggle between plan and build mode. Returns the new state.
    pub fn toggle(&self) -> bool {
        let old = self.0.fetch_xor(true, Ordering::Relaxed);
        !old
    }

    /// Set plan mode explicitly.
    pub fn set(&self, plan: bool) {
        self.0.store(plan, Ordering::Relaxed);
    }
}

impl Default for PlanMode {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared application state.
///
/// The provider is stored in an `Arc` so it can be sent to spawned tasks
/// independently of the `Db` (which is `!Send` due to `rusqlite`).
pub struct AppState {
    pub db: Db,
    pub config: FlokConfig,
    pub provider: Arc<dyn Provider>,
    pub provider_registry: Arc<ProviderRegistry>,
    pub tools: ToolRegistry,
    pub bus: Bus,
    pub permissions: PermissionManager,
    pub cost_tracker: CostTracker,
    pub plan_mode: PlanMode,
    pub project_root: PathBuf,
    pub project_id: String,
    pub snapshot: Arc<SnapshotManager>,
    pub lsp: Arc<LspManager>,
}

impl AppState {
    /// Create a new application state.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Db,
        config: FlokConfig,
        provider: Arc<dyn Provider>,
        provider_registry: Arc<ProviderRegistry>,
        tools: ToolRegistry,
        bus: Bus,
        permissions: PermissionManager,
        cost_tracker: CostTracker,
        plan_mode: PlanMode,
        project_root: PathBuf,
        project_id: String,
        snapshot: Arc<SnapshotManager>,
        lsp: Arc<LspManager>,
    ) -> Self {
        Self {
            db,
            config,
            provider,
            provider_registry,
            tools,
            bus,
            permissions,
            cost_tracker,
            plan_mode,
            project_root,
            project_id,
            snapshot,
            lsp,
        }
    }

    /// Create a `ToolContext` for tool execution.
    ///
    /// The `cancel` token is shared with the session engine so that
    /// cancellation propagates into running tools.
    pub fn tool_context(
        &self,
        session_id: &str,
        cancel: tokio_util::sync::CancellationToken,
    ) -> ToolContext {
        ToolContext {
            project_root: self.project_root.clone(),
            session_id: session_id.to_string(),
            agent: "primary".to_string(),
            cancel,
            lsp: Some(Arc::clone(&self.lsp)),
        }
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("project_root", &self.project_root)
            .field("project_id", &self.project_id)
            .finish_non_exhaustive()
    }
}
