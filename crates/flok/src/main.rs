//! # flok
//!
//! An AI coding agent for the terminal. This is the binary entry point
//! that wires together the database, core engine, and TUI.

mod cli;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing_subscriber::{fmt, EnvFilter};

use flok_core::bus::Bus;
use flok_core::config;
use flok_core::provider::AnthropicProvider;
use flok_core::session::{AppState, SessionEngine};
use flok_core::snapshot::SnapshotManager;
use flok_core::team::TeamRegistry;
use flok_core::tool::{
    AgentMemoryTool, BashTool, CodeReviewTool, EditTool, FastApplyTool, GlobTool, GrepTool,
    PlanTool, QuestionTool, ReadTool, SendMessageTool, SkillTool, TaskTool, TeamCreateTool,
    TeamDeleteTool, TeamTaskTool, TodoList, TodoWriteTool, ToolRegistry, WebfetchTool, WriteTool,
};
use flok_core::worktree::WorktreeManager;
use tokio::sync::mpsc;

fn main() -> Result<()> {
    // Initialize tracing (only to file/stderr, not stdout — TUI owns stdout)
    fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    let args = cli::Args::parse_args();
    tracing::debug!(?args, "starting flok");

    // Use current_thread + LocalSet because SessionEngine holds Db which is !Send.
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let local = tokio::task::LocalSet::new();
    runtime.block_on(local.run_until(run(args)))
}

async fn run(args: cli::Args) -> Result<()> {
    // Handle subcommands that don't need full initialization
    if let Some(ref cmd) = args.command {
        match cmd {
            cli::Command::Models => return run_models(),
            cli::Command::Version => return run_version(),
            cli::Command::Sessions { .. } => {} // Needs DB — fall through
        }
    }

    // Ensure XDG directories exist
    config::ensure_directories()?;

    // Resolve project root: walk up from CWD/workdir to find .git, Cargo.toml, etc.
    let start_dir = args
        .workdir
        .unwrap_or_else(|| std::env::current_dir().expect("cannot determine current directory"));
    let start_dir = std::fs::canonicalize(&start_dir)
        .with_context(|| format!("invalid working directory: {}", start_dir.display()))?;
    let project_root = config::detect_project_root(&start_dir);

    // Load config (merges global + project + .flok layers)
    let config = config::load_config(&project_root)?;

    // Resolve model (supports shorthand like "sonnet", "opus", "haiku")
    let model_id = flok_core::provider::ModelRegistry::resolve(
        &args.model.unwrap_or_else(|| "sonnet".to_string()),
    );

    // Initialize database
    let db_dir = data_dir()?.join("db");
    std::fs::create_dir_all(&db_dir)?;
    let db_path = db_dir.join("flok.db");
    let db = flok_db::Db::open(&db_path)
        .with_context(|| format!("failed to open database at {}", db_path.display()))?;

    // Handle sessions subcommand (needs DB)
    if let Some(cli::Command::Sessions { project, limit }) = &args.command {
        return run_sessions(&db, project.as_deref(), *limit);
    }

    // Register project — use the returned project's actual ID (may differ from
    // the proposed ID if the project already exists for this path).
    let proposed_id = ulid::Ulid::new().to_string();
    let project_path = project_root.to_string_lossy().to_string();
    let project = db.get_or_create_project(&proposed_id, &project_path)?;
    let project_id = project.id;

    // Create snapshot manager for workspace checkpoints
    let snapshot = Arc::new(SnapshotManager::new(&project_id, project_root.clone()));
    if snapshot.is_enabled() {
        tracing::info!("snapshot system enabled (git project detected)");
    } else {
        tracing::debug!("snapshot system disabled (no .git directory)");
    }

    // Create provider based on model prefix
    let provider: Arc<dyn flok_core::provider::Provider> = create_provider(&model_id, &config)?;

    // Shared state for interactive tools
    let todo_list = TodoList::new();
    let (question_tx, question_rx) = mpsc::unbounded_channel();

    // Register tools
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(ReadTool));
    tools.register(Arc::new(WriteTool));
    tools.register(Arc::new(EditTool));
    tools.register(Arc::new(FastApplyTool));
    tools.register(Arc::new(BashTool));
    tools.register(Arc::new(GrepTool));
    tools.register(Arc::new(GlobTool));
    tools.register(Arc::new(WebfetchTool::new()));
    tools.register(Arc::new(QuestionTool::new(question_tx)));
    tools.register(Arc::new(TodoWriteTool::new(todo_list.clone())));
    tools.register(Arc::new(SkillTool));
    tools.register(Arc::new(AgentMemoryTool));
    tools.register(Arc::new(PlanTool));
    tools.register(Arc::new(CodeReviewTool::new(Arc::clone(&provider))));

    // Create team registry for multi-agent coordination
    let team_registry = TeamRegistry::new();
    tools.register(Arc::new(TeamCreateTool::new(team_registry.clone())));
    tools.register(Arc::new(TeamDeleteTool::new(team_registry.clone())));
    tools.register(Arc::new(TeamTaskTool::new(team_registry.clone())));
    tools.register(Arc::new(SendMessageTool::new(team_registry.clone())));

    // Create bus (before task tool, which needs it)
    let bus = Bus::new(512);

    // Create worktree manager for agent isolation
    let worktree_mgr = Arc::new(WorktreeManager::new(&project_id, project_root.clone()));
    if worktree_mgr.is_enabled() && config.worktree.enabled {
        tracing::info!("worktree isolation enabled for background agents");
        // Clean up stale worktrees from previous sessions
        match worktree_mgr.cleanup_stale().await {
            Ok(0) => {}
            Ok(n) => tracing::info!(count = n, "cleaned stale worktrees"),
            Err(e) => tracing::warn!(error = %e, "stale worktree cleanup failed"),
        }
    } else {
        tracing::debug!("worktree isolation disabled");
    }

    // Snapshot the base tools for the task tool (before registering task itself)
    let base_tools = Arc::new(tools.clone());
    tools.register(Arc::new(TaskTool::new(
        Arc::clone(&provider),
        base_tools,
        bus.clone(),
        project_root.clone(),
        Arc::clone(&worktree_mgr),
        config.worktree.clone(),
        team_registry,
    )));

    tracing::info!(
        tools = ?tools.names(),
        model = %model_id,
        project = %project_root.display(),
        "flok v{} ready",
        env!("CARGO_PKG_VERSION")
    );

    // Shared plan mode flag — respect --plan CLI flag
    let plan_mode = flok_core::session::PlanMode::new();
    if args.plan {
        plan_mode.set(true);
    }

    // Handle non-interactive mode vs interactive mode
    if let Some(prompt) = args.prompt {
        // Non-interactive: auto-approve all permissions
        let permissions = flok_core::tool::PermissionManager::auto_approve();
        let cost_tracker = flok_core::token::CostTracker::new(&model_id);
        let state = AppState::new(
            db,
            config,
            provider,
            tools,
            bus.clone(),
            permissions,
            cost_tracker,
            plan_mode,
            project_root,
            project_id,
            snapshot,
        );
        return run_non_interactive(state, model_id, &prompt).await;
    }

    // Interactive mode — create permission channel for TUI prompts
    let (perm_tx, perm_rx) = mpsc::unbounded_channel();
    let permissions = flok_core::tool::PermissionManager::new(perm_tx);
    let cost_tracker = flok_core::token::CostTracker::new(&model_id);
    let state = AppState::new(
        db,
        config,
        provider,
        tools,
        bus.clone(),
        permissions,
        cost_tracker,
        plan_mode.clone(),
        project_root,
        project_id,
        snapshot,
    );

    run_interactive(state, model_id, bus, args.session, perm_rx, question_rx, todo_list, plan_mode)
        .await
}

/// Run in non-interactive mode: send a single prompt and print the response.
async fn run_non_interactive(state: AppState, model_id: String, prompt: &str) -> Result<()> {
    let mut engine = SessionEngine::new(state, model_id)?;
    let result = engine.send_message(prompt).await?;
    // In non-interactive mode, print directly to stdout
    #[allow(clippy::print_stdout)]
    {
        match result {
            flok_core::session::SendMessageResult::Complete(response) => {
                println!("{response}");
            }
            flok_core::session::SendMessageResult::Cancelled { partial_text } => {
                if !partial_text.is_empty() {
                    println!("{partial_text}");
                }
                println!("(cancelled)");
            }
        }
    }
    Ok(())
}

/// Run in interactive mode with the TUI.
#[allow(clippy::too_many_arguments)]
async fn run_interactive(
    state: AppState,
    model_id: String,
    bus: Bus,
    resume_session: Option<String>,
    perm_rx: mpsc::UnboundedReceiver<flok_core::tool::PermissionRequest>,
    question_rx: mpsc::UnboundedReceiver<flok_core::tool::QuestionRequest>,
    todo_list: flok_core::tool::TodoList,
    plan_mode: flok_core::session::PlanMode,
) -> Result<()> {
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<flok_tui::UiCommand>();
    let (ui_tx, ui_rx) = mpsc::unbounded_channel::<flok_tui::UiEvent>();
    let bus_rx = bus.subscribe();

    let display_model = flok_core::provider::ModelRegistry::model_name(&model_id).to_string();
    let channels = flok_tui::types::TuiChannels {
        cmd_tx: cmd_tx.clone(),
        ui_rx,
        bus_rx,
        perm_rx,
        question_rx,
        todo_list,
        plan_mode,
        model_name: display_model,
    };

    // Create/resume session engine
    let mut engine = if let Some(ref session_id) = resume_session {
        let e = SessionEngine::resume(state, session_id.clone())?;
        // Load and send historical messages to the TUI
        if let Ok(history) = e.load_display_messages() {
            for (role, content) in history {
                let _ = ui_tx.send(flok_tui::UiEvent::HistoryMessage { role, content });
            }
        }
        e
    } else {
        SessionEngine::new(state, model_id)?
    };

    // Spawn the background session task.
    //
    // The key challenge is that `engine.send_message()` blocks the command
    // loop while streaming + executing tools. We use `tokio::select!` to
    // allow Cancel (and Quit) commands to be processed during that time.
    let session_handle = tokio::task::spawn_local(async move {
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                flok_tui::UiCommand::SendMessage(text) => {
                    // Reset + clone the cancel token BEFORE the mutable borrow
                    // in send_message(). This ensures the cloned token matches
                    // the one used inside the prompt loop.
                    engine.reset_cancel();
                    let cancel = engine.cancel_token();

                    // Pin the future so it can be polled in select!
                    let mut send_fut = std::pin::pin!(engine.send_message(&text));
                    let result = loop {
                        tokio::select! {
                            biased;
                            result = &mut send_fut => break result,
                            Some(inner_cmd) = cmd_rx.recv() => {
                                match inner_cmd {
                                    flok_tui::UiCommand::Cancel => {
                                        cancel.cancel();
                                        // Don't break — let send_message detect
                                        // the token and finish gracefully
                                    }
                                    flok_tui::UiCommand::Quit => {
                                        cancel.cancel();
                                        return;
                                    }
                                    // Ignore other commands while streaming
                                    _ => {}
                                }
                            }
                        }
                    };
                    match result {
                        Ok(flok_core::session::SendMessageResult::Complete(response)) => {
                            let _ = ui_tx.send(flok_tui::UiEvent::AssistantDone(response));
                        }
                        Ok(flok_core::session::SendMessageResult::Cancelled { partial_text }) => {
                            let _ = ui_tx.send(flok_tui::UiEvent::Cancelled(partial_text));
                        }
                        Err(e) => {
                            let _ = ui_tx.send(flok_tui::UiEvent::Error(e.to_string()));
                        }
                    }
                }
                flok_tui::UiCommand::ListSessions => {
                    // Query sessions from the DB and send as a system message
                    match engine.list_sessions_text() {
                        Ok(text) => {
                            let _ = ui_tx.send(flok_tui::UiEvent::Error(text));
                        }
                        Err(e) => {
                            let _ = ui_tx.send(flok_tui::UiEvent::Error(e.to_string()));
                        }
                    }
                }
                flok_tui::UiCommand::SwitchModel(model_id) => {
                    let _ = ui_tx.send(flok_tui::UiEvent::Error(format!(
                        "Model switch to {model_id} — will take effect on next message. (Full mid-session model switching coming soon.)"
                    )));
                }
                flok_tui::UiCommand::Undo => match engine.undo().await {
                    Ok(Some(result)) => {
                        let _ = ui_tx.send(flok_tui::UiEvent::Error(result.message));
                    }
                    Ok(None) => {
                        let _ =
                            ui_tx.send(flok_tui::UiEvent::Error("Nothing to undo.".to_string()));
                    }
                    Err(e) => {
                        let _ = ui_tx.send(flok_tui::UiEvent::Error(format!("Undo failed: {e}")));
                    }
                },
                flok_tui::UiCommand::Redo => match engine.redo().await {
                    Ok(Some(result)) => {
                        let _ = ui_tx.send(flok_tui::UiEvent::Error(result.message));
                    }
                    Ok(None) => {
                        let _ =
                            ui_tx.send(flok_tui::UiEvent::Error("Nothing to redo.".to_string()));
                    }
                    Err(e) => {
                        let _ = ui_tx.send(flok_tui::UiEvent::Error(format!("Redo failed: {e}")));
                    }
                },
                flok_tui::UiCommand::Cancel => {
                    // Cancel received outside of streaming — nothing to cancel
                }
                flok_tui::UiCommand::Quit => break,
            }
        }
    });

    // Run the TUI (this blocks until the user quits)
    // Permission and question prompts are handled directly by the TUI (no forwarding tasks needed)
    flok_tui::run_app(channels).await?;

    // Cleanup
    session_handle.abort();

    Ok(())
}

/// Create the appropriate provider based on the model ID prefix.
fn create_provider(
    model_id: &str,
    config: &flok_core::config::FlokConfig,
) -> Result<Arc<dyn flok_core::provider::Provider>> {
    let provider_name = flok_core::provider::ModelRegistry::provider_name(model_id);

    match provider_name {
        "anthropic" => {
            let api_key = resolve_api_key("ANTHROPIC_API_KEY", "anthropic", config)?;
            Ok(Arc::new(AnthropicProvider::new(api_key, None)))
        }
        "openai" => {
            let api_key = resolve_api_key("OPENAI_API_KEY", "openai", config)?;
            let base_url = config.provider.get("openai").and_then(|c| c.base_url.clone());
            Ok(Arc::new(flok_core::provider::OpenAiProvider::new(api_key, base_url)))
        }
        "deepseek" => {
            let api_key = resolve_api_key("DEEPSEEK_API_KEY", "deepseek", config)?;
            let base_url = config
                .provider
                .get("deepseek")
                .and_then(|c| c.base_url.clone())
                .or_else(|| Some("https://api.deepseek.com/v1".to_string()));
            Ok(Arc::new(flok_core::provider::OpenAiProvider::new(api_key, base_url)))
        }
        "google" => Err(anyhow::anyhow!(
            "Google Gemini provider not yet implemented. Use Anthropic or OpenAI."
        )),
        _ => Err(anyhow::anyhow!(
            "Unknown provider '{provider_name}' for model '{model_id}'. \
             Supported providers: anthropic, openai, deepseek"
        )),
    }
}

/// Resolve an API key from the environment or config.
fn resolve_api_key(
    env_var: &str,
    config_key: &str,
    config: &flok_core::config::FlokConfig,
) -> Result<String> {
    // 1. Environment variable
    if let Ok(key) = std::env::var(env_var) {
        if !key.is_empty() {
            return Ok(key);
        }
    }

    // 2. Config file
    if let Some(provider_config) = config.provider.get(config_key) {
        if let Some(key) = &provider_config.api_key {
            if !key.is_empty() {
                return Ok(key.clone());
            }
        }
    }

    Err(anyhow::anyhow!(
        "No API key found for {config_key}. Set {env_var} environment variable \
         or add [provider.{config_key}] api_key to flok.toml"
    ))
}

/// Get the flok data directory.
fn data_dir() -> Result<PathBuf> {
    let dirs = directories::BaseDirs::new().context("cannot determine home directory")?;
    Ok(dirs.data_dir().join("flok"))
}

// ---------------------------------------------------------------------------
// Subcommand implementations
// ---------------------------------------------------------------------------

/// List available models and their pricing.
#[allow(clippy::unnecessary_wraps)]
fn run_models() -> Result<()> {
    let registry = flok_core::provider::ModelRegistry::builtin();
    let mut models: Vec<&flok_core::provider::ModelInfo> = registry.all();
    models.sort_by_key(|m| m.id);

    #[allow(clippy::print_stdout)]
    {
        println!("{:<45} {:<10} {:>8} {:>8}", "MODEL", "PROVIDER", "IN/M$", "OUT/M$");
        println!("{}", "-".repeat(75));
        for model in &models {
            println!(
                "{:<45} {:<10} {:>8.2} {:>8.2}",
                model.id, model.provider, model.input_cost_per_m, model.output_cost_per_m
            );
        }
        println!("\n{} models available", models.len());
        println!("\nShorthands: sonnet, opus, haiku, gpt-4.1, mini, flash, pro, deepseek, r1");
    }
    Ok(())
}

/// Show version info.
#[allow(clippy::unnecessary_wraps)]
fn run_version() -> Result<()> {
    #[allow(clippy::print_stdout)]
    {
        println!("flok v{}", env!("CARGO_PKG_VERSION"));
        println!("Platform: {}", std::env::consts::OS);
        println!("Arch: {}", std::env::consts::ARCH);
    }
    Ok(())
}

/// List past sessions.
fn run_sessions(db: &flok_db::Db, _project_filter: Option<&str>, limit: usize) -> Result<()> {
    // List all projects first, then sessions for each
    // For simplicity, get sessions for the current project
    let cwd = std::env::current_dir()?;
    let project_root = config::detect_project_root(&cwd);
    let project_path = project_root.to_string_lossy().to_string();

    // Try to find the project
    let project = db.get_or_create_project(&ulid::Ulid::new().to_string(), &project_path)?;
    let sessions = db.list_sessions(&project.id)?;

    #[allow(clippy::print_stdout)]
    {
        if sessions.is_empty() {
            println!("No sessions found for project: {project_path}");
            return Ok(());
        }

        println!("Sessions for: {project_path}");
        println!("{:<30} {:<50} {:<20}", "ID", "TITLE", "UPDATED");
        println!("{}", "-".repeat(100));

        for session in sessions.iter().take(limit) {
            let title = if session.title.is_empty() { "(untitled)" } else { &session.title };
            let title_display =
                if title.len() > 47 { format!("{}...", &title[..47]) } else { title.to_string() };
            println!("{:<30} {:<50} {:<20}", session.id, title_display, session.updated_at);
        }

        let total = sessions.len();
        if total > limit {
            println!("\n... and {} more (use --limit to see more)", total - limit);
        }
        println!("\nResume a session: flok --resume <ID>");
    }
    Ok(())
}
