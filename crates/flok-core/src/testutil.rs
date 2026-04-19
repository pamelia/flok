//! Test utilities for integration tests.
//!
//! Provides a `TestHarness` that wires up a full `AppState` with a mock
//! provider, in-memory DB, and temp directory so integration tests can
//! exercise the engine's prompt loop with scripted tool calls.

use std::path::PathBuf;
use std::sync::Arc;

use flok_db::Db;
use tempfile::TempDir;

use crate::bus::Bus;
use crate::config::FlokConfig;
use crate::lsp::LspManager;
use crate::provider::mock::{MockProvider, MockTurn};
use crate::provider::ProviderRegistry;
use crate::session::{AppState, PlanMode, SessionEngine};
use crate::snapshot::SnapshotManager;
use crate::token::CostTracker;
use crate::tool::{
    BashTool, EditTool, FastApplyTool, GlobTool, GrepTool, PermissionManager, ReadTool, SkillTool,
    ToolRegistry, WriteTool,
};

/// A self-contained test environment with mock provider and temp filesystem.
///
/// Call [`TestHarness::new`] to create a fresh environment.
pub struct TestHarness {
    /// The session engine (runs the full prompt loop).
    pub engine: SessionEngine,
    /// The mock provider (push turns before calling `send_message`).
    pub mock: Arc<MockProvider>,
    /// The temp directory (destroyed on drop).
    pub dir: TempDir,
    /// Reference to the plan mode flag.
    pub plan_mode: PlanMode,
}

impl Default for TestHarness {
    fn default() -> Self {
        Self::new()
    }
}

impl TestHarness {
    /// Create a new test harness with a fresh environment.
    ///
    /// Sets up: temp dir, in-memory DB, mock provider, auto-approve
    /// permissions, and a `SessionEngine` with the core filesystem tools
    /// registered (read, write, edit, `fast_apply`, bash, grep, glob, skill).
    ///
    /// # Panics
    ///
    /// Panics if setup fails (test infrastructure, not user-facing).
    pub fn new() -> Self {
        let dir = TempDir::new().expect("failed to create temp dir");

        // Canonicalize the path to handle macOS /var -> /private/var symlinks.
        // Tools canonicalize file paths before containment checks, so the
        // project root must also be canonical for the check to pass.
        let canonical_root =
            std::fs::canonicalize(dir.path()).expect("failed to canonicalize temp dir");

        // Create a marker so detect_project_root stops here
        std::fs::create_dir_all(canonical_root.join(".git")).expect("failed to create .git dir");

        let db = Db::open_in_memory().expect("failed to open in-memory DB");
        let project_id = "test-project";
        db.get_or_create_project(project_id, canonical_root.to_str().unwrap())
            .expect("failed to create project");

        let mock = Arc::new(MockProvider::new());
        let provider: Arc<dyn crate::provider::Provider> = Arc::clone(&mock) as _;
        let mut provider_registry = ProviderRegistry::new();
        provider_registry.insert("mock", Arc::clone(&provider), Some("mock/test-model".into()), 3);
        let provider_registry = Arc::new(provider_registry);

        // Register the core tools (no question, todowrite, team, task, webfetch --
        // those need channels or external state that isn't relevant for most tests)
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(ReadTool));
        tools.register(Arc::new(WriteTool));
        tools.register(Arc::new(EditTool));
        tools.register(Arc::new(FastApplyTool));
        tools.register(Arc::new(BashTool));
        tools.register(Arc::new(GrepTool));
        tools.register(Arc::new(GlobTool));
        tools.register(Arc::new(SkillTool));

        let bus = Bus::new(64);
        let permissions = PermissionManager::auto_approve();
        let cost_tracker = CostTracker::new("test-model");
        let plan_mode = PlanMode::new();
        let snapshot = Arc::new(SnapshotManager::new("test-session", canonical_root.clone()));
        let lsp = Arc::new(LspManager::disabled(canonical_root.clone()));

        let state = AppState::new(
            db,
            FlokConfig::default(),
            provider,
            provider_registry,
            tools,
            bus,
            permissions,
            cost_tracker,
            plan_mode.clone(),
            canonical_root,
            project_id.to_string(),
            snapshot,
            lsp,
        );
        let engine = SessionEngine::new(state, "mock/test-model".to_string())
            .expect("failed to create engine");

        Self { engine, mock, dir, plan_mode }
    }

    /// Push a scripted turn onto the mock provider.
    pub fn push_turn(&self, turn: MockTurn) {
        self.mock.push_turn(turn);
    }

    /// Send a message through the engine (runs the full prompt loop).
    pub async fn send_message(
        &mut self,
        text: &str,
    ) -> anyhow::Result<crate::session::SendMessageResult> {
        self.engine.send_message(text).await
    }

    /// Get the canonical project root path.
    fn root(&self) -> PathBuf {
        // Use the canonical path that the engine was constructed with,
        // NOT dir.path() which may differ on macOS due to /var -> /private/var.
        std::fs::canonicalize(self.dir.path()).expect("failed to canonicalize temp dir")
    }

    /// Get the absolute path for a relative path within the temp dir.
    pub fn path(&self, relative: &str) -> String {
        self.root().join(relative).to_string_lossy().to_string()
    }

    /// Get the absolute `PathBuf` for a relative path within the temp dir.
    pub fn pathbuf(&self, relative: &str) -> PathBuf {
        self.root().join(relative)
    }

    /// Check if a file exists in the temp dir.
    pub fn file_exists(&self, relative: &str) -> bool {
        self.root().join(relative).exists()
    }

    /// Read a file from the temp dir.
    ///
    /// # Panics
    ///
    /// Panics if the file doesn't exist or can't be read.
    pub fn read_file(&self, relative: &str) -> String {
        std::fs::read_to_string(self.root().join(relative))
            .unwrap_or_else(|e| panic!("failed to read {relative}: {e}"))
    }

    /// Write a file into the temp dir (creates parent dirs as needed).
    pub fn write_file(&self, relative: &str, content: &str) {
        let path = self.root().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("failed to create parent dirs");
        }
        std::fs::write(&path, content).expect("failed to write file");
    }

    /// Get the project root path.
    pub fn project_root(&self) -> PathBuf {
        self.root()
    }
}
