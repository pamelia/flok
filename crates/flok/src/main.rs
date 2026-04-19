//! # flok
//!
//! An AI coding agent for the terminal. This is the binary entry point
//! that wires together the database, core engine, and TUI.

mod cli;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use secrecy::{ExposeSecret, SecretString};
use tracing_subscriber::{fmt, EnvFilter};

use flok_core::bus::Bus;
use flok_core::config;
use flok_core::lsp::LspManager;
use flok_core::provider::{
    AnthropicProvider, MiniMaxProvider, ProviderRegistry, DEFAULT_PERMITS_PER_PROVIDER,
};
use flok_core::session::{AppState, SessionEngine};
use flok_core::snapshot::SnapshotManager;
use flok_core::team::TeamRegistry;
use flok_core::tool::{
    AgentMemoryTool, BashTool, CodeReviewTool, EditTool, FastApplyTool, GlobTool, GrepTool,
    LspDiagnosticsTool, LspFindReferencesTool, LspGotoDefinitionTool, LspSymbolsTool, PlanTool,
    QuestionTool, ReadTool, SendMessageTool, SkillTool, TaskTool, TeamCreateTool, TeamDeleteTool,
    TeamTaskTool, TodoList, TodoWriteTool, ToolRegistry, WebfetchTool, WriteTool,
};
use flok_core::worktree::WorktreeManager;
use tokio::sync::mpsc;

fn main() -> Result<()> {
    let args = cli::Args::parse_args();

    // Initialize tracing.
    // --debug: write structured logs to /tmp/flok.log at debug level.
    // Otherwise: quiet mode, stderr only, controlled by RUST_LOG env var.
    if args.debug {
        let log_path = std::path::PathBuf::from("/tmp/flok.log");
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("failed to open log file: {}", log_path.display()))?;

        fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug")),
            )
            .with_target(true)
            .with_thread_ids(true)
            .with_writer(log_file)
            .init();

        tracing::info!("debug logging enabled, writing to {}", log_path.display());
    } else {
        fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .with_target(false)
            .with_writer(std::io::stderr)
            .init();
    }

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
            cli::Command::Auth { .. } => return run_auth(cmd),
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

    // Resolve model: --model flag > config.model > provider default_model > "sonnet".
    let model_id =
        flok_core::provider::resolve_default_model(args.model.as_deref(), &config, "sonnet");

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
    let provider_registry = build_provider_registry(&config)?;

    // Shared state for interactive tools
    let todo_list = TodoList::new();
    let (question_tx, question_rx) = mpsc::unbounded_channel();

    // LSP manager for rust-analyzer integration
    let lsp = Arc::new(LspManager::new(project_root.clone(), config.lsp.clone()));

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

    // Create bus (needs to be before team/task tools which use it)
    let bus = Bus::new(512);

    // Create team registry for multi-agent coordination
    let team_registry = TeamRegistry::new();
    tools.register(Arc::new(TeamCreateTool::new(team_registry.clone(), bus.clone())));
    tools.register(Arc::new(TeamDeleteTool::new(team_registry.clone())));
    tools.register(Arc::new(TeamTaskTool::new(team_registry.clone())));
    tools.register(Arc::new(SendMessageTool::new(team_registry.clone())));

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
        Arc::clone(&provider_registry),
        flok_core::provider::ModelRegistry::provider_name(&model_id).to_string(),
        model_id.clone(),
        base_tools,
        bus.clone(),
        project_root.clone(),
        Arc::clone(&worktree_mgr),
        config.worktree.clone(),
        team_registry,
    )));

    if lsp.tools_enabled() {
        tools.register(Arc::new(LspDiagnosticsTool::new(Arc::clone(&lsp))));
        tools.register(Arc::new(LspGotoDefinitionTool::new(Arc::clone(&lsp))));
        tools.register(Arc::new(LspFindReferencesTool::new(Arc::clone(&lsp))));
        tools.register(Arc::new(LspSymbolsTool::new(Arc::clone(&lsp))));
    }

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
            provider_registry,
            tools,
            bus.clone(),
            permissions,
            cost_tracker,
            plan_mode,
            project_root,
            project_id,
            snapshot,
            Arc::clone(&lsp),
        );
        return run_non_interactive(state, model_id, &prompt).await;
    }

    // Interactive mode — create permission channel for TUI prompts
    let (perm_tx, perm_rx) = mpsc::unbounded_channel();
    let mut permissions = flok_core::tool::PermissionManager::new(perm_tx);

    // Load config-provided permission rules
    if !config.permission.is_empty() {
        let config_rules = flok_core::config::permission_config_to_rules(&config.permission);
        if !config_rules.is_empty() {
            tracing::info!(count = config_rules.len(), "loaded permission rules from config");
            permissions.set_config_rules(config_rules);
        }
    }

    // Load persisted permission rules from database
    match db.list_permission_rules(&project_id) {
        Ok(rows) => {
            let rules: Vec<flok_core::permission::PermissionRule> = rows
                .into_iter()
                .filter_map(|row| {
                    let action = match row.action.as_str() {
                        "allow" => flok_core::permission::PermissionAction::Allow,
                        "deny" => flok_core::permission::PermissionAction::Deny,
                        "ask" => flok_core::permission::PermissionAction::Ask,
                        _ => return None,
                    };
                    Some(flok_core::permission::PermissionRule::new(
                        row.permission,
                        row.pattern,
                        action,
                    ))
                })
                .collect();
            if !rules.is_empty() {
                tracing::info!(count = rules.len(), "loaded persisted permission rules");
                permissions.load_session_rules(rules);
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to load persisted permission rules");
        }
    }
    let cost_tracker = flok_core::token::CostTracker::new(&model_id);
    let state = AppState::new(
        db,
        config,
        provider,
        provider_registry,
        tools,
        bus.clone(),
        permissions,
        cost_tracker,
        plan_mode.clone(),
        project_root,
        project_id,
        snapshot,
        Arc::clone(&lsp),
    );

    run_interactive(
        state,
        model_id,
        bus,
        args.session,
        perm_rx,
        question_rx,
        todo_list,
        plan_mode,
        Arc::clone(&lsp),
    )
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
    _lsp: Arc<LspManager>,
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

    let session_id_for_exit = engine.session_id().to_string();

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
                flok_tui::UiCommand::ShowTree => match engine.session_tree_text() {
                    Ok(text) => {
                        let _ = ui_tx.send(flok_tui::UiEvent::Error(text));
                    }
                    Err(e) => {
                        let _ = ui_tx.send(flok_tui::UiEvent::Error(format!(
                            "Failed to build session tree: {e}"
                        )));
                    }
                },
                flok_tui::UiCommand::ListBranchPoints => match engine.list_branch_points() {
                    Ok(points) => {
                        let _ = ui_tx.send(flok_tui::UiEvent::BranchPoints(points));
                    }
                    Err(e) => {
                        let _ = ui_tx.send(flok_tui::UiEvent::Error(format!(
                            "Failed to list branch points: {e}"
                        )));
                    }
                },
                flok_tui::UiCommand::BranchAt(arg) => {
                    // Resolve the argument: could be a number (from the list) or a message ID
                    let message_id = if let Ok(num) = arg.parse::<usize>() {
                        // User typed a number — resolve to a message ID
                        match engine.list_branch_points() {
                            Ok(points) => points
                                .iter()
                                .find(|(_, idx, _)| *idx == num)
                                .map(|(id, _, _)| id.clone()),
                            Err(e) => {
                                let _ = ui_tx.send(flok_tui::UiEvent::Error(format!(
                                    "Failed to resolve branch point: {e}"
                                )));
                                None
                            }
                        }
                    } else {
                        // Treat as a direct message ID
                        Some(arg)
                    };

                    if let Some(msg_id) = message_id {
                        match engine.branch_at_message(&msg_id).await {
                            Ok(result) => {
                                let msg = format!(
                                    "Branch created: {} messages copied{}. \
                                     Switching to branch session [{:.8}].",
                                    result.messages_copied,
                                    if result.summary_generated {
                                        " + summary generated"
                                    } else {
                                        ""
                                    },
                                    result.session_id,
                                );
                                // Auto-switch to the new branch
                                match engine.switch_session(&result.session_id).await {
                                    Ok(history) => {
                                        let _ = ui_tx.send(flok_tui::UiEvent::Error(msg));
                                        let _ = ui_tx.send(flok_tui::UiEvent::SessionSwitched {
                                            messages: history,
                                        });
                                    }
                                    Err(e) => {
                                        let _ = ui_tx.send(flok_tui::UiEvent::Error(format!(
                                            "{msg}\nBut switch failed: {e}"
                                        )));
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = ui_tx
                                    .send(flok_tui::UiEvent::Error(format!("Branch failed: {e}")));
                            }
                        }
                    } else {
                        let _ = ui_tx.send(flok_tui::UiEvent::Error(
                            "Invalid branch point. Use /branch to list available points."
                                .to_string(),
                        ));
                    }
                }
                flok_tui::UiCommand::SwitchSession(session_id) => {
                    match engine.switch_session(&session_id).await {
                        Ok(history) => {
                            let _ = ui_tx
                                .send(flok_tui::UiEvent::SessionSwitched { messages: history });
                        }
                        Err(e) => {
                            let _ = ui_tx.send(flok_tui::UiEvent::Error(format!(
                                "Session switch failed: {e}"
                            )));
                        }
                    }
                }
                flok_tui::UiCommand::SetLabel(label) => match engine.set_label(&label) {
                    Ok(()) => {
                        let _ =
                            ui_tx.send(flok_tui::UiEvent::Error(format!("Label set: \"{label}\"")));
                    }
                    Err(e) => {
                        let _ = ui_tx
                            .send(flok_tui::UiEvent::Error(format!("Failed to set label: {e}")));
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

    // Print resume hint so the user knows how to get back.
    #[allow(clippy::print_stdout)]
    {
        println!("\nResume this session:\n  flok --session {session_id_for_exit}");
    }

    Ok(())
}

/// Create the appropriate provider based on the model ID prefix.
fn create_provider(
    model_id: &str,
    config: &flok_core::config::FlokConfig,
) -> Result<Arc<dyn flok_core::provider::Provider>> {
    let provider_name = flok_core::provider::ModelRegistry::provider_name(model_id);

    create_provider_for_name(provider_name, config)
}

fn build_provider_registry(
    config: &flok_core::config::FlokConfig,
) -> Result<Arc<ProviderRegistry>> {
    let mut registry = ProviderRegistry::new();
    let mut provider_names: Vec<&String> = config.provider.keys().collect();
    provider_names.sort();

    for provider_name in provider_names {
        let Some(provider_config) = config.provider.get(provider_name) else {
            continue;
        };

        if provider_config.api_key.is_none() {
            tracing::debug!(provider = %provider_name, "skipping provider without API key");
            continue;
        }

        let provider = create_provider_for_name(provider_name, config)?;
        let default_model = provider_config
            .default_model
            .as_deref()
            .map(flok_core::provider::ModelRegistry::resolve);

        registry.insert(
            provider_name.clone(),
            provider,
            default_model,
            DEFAULT_PERMITS_PER_PROVIDER,
        );
    }

    Ok(Arc::new(registry))
}

fn create_provider_for_name(
    provider_name: &str,
    config: &flok_core::config::FlokConfig,
) -> Result<Arc<dyn flok_core::provider::Provider>> {
    match provider_name {
        "anthropic" => {
            let api_key = resolve_api_key("anthropic", config)?;
            let base_url = config.provider.get("anthropic").and_then(|c| c.base_url.clone());
            Ok(Arc::new(AnthropicProvider::new(api_key, base_url)))
        }
        "openai" => {
            let api_key = resolve_api_key("openai", config)?;
            let base_url = config.provider.get("openai").and_then(|c| c.base_url.clone());
            Ok(Arc::new(flok_core::provider::OpenAiProvider::new(api_key, base_url)))
        }
        "deepseek" => {
            let api_key = resolve_api_key("deepseek", config)?;
            let base_url = config
                .provider
                .get("deepseek")
                .and_then(|c| c.base_url.clone())
                .or_else(|| Some("https://api.deepseek.com/v1".to_string()));
            Ok(Arc::new(flok_core::provider::OpenAiProvider::new(api_key, base_url)))
        }
        "minimax" => {
            let api_key = resolve_api_key("minimax", config)?;
            Ok(Arc::new(MiniMaxProvider::new(api_key, None)))
        }
        "google" => Err(anyhow::anyhow!(
            "Google Gemini provider not yet implemented. Use Anthropic or OpenAI."
        )),
        _ => Err(anyhow::anyhow!(
            "Unknown provider '{provider_name}'. \
             Supported providers: anthropic, openai, deepseek, minimax"
        )),
    }
}

/// Resolve an API key from the config file.
///
/// Runtime credentials are read ONLY from the config file — env vars are NOT
/// consulted (per project policy; see AGENTS.md §0.3 and the docs).
fn resolve_api_key(
    config_key: &str,
    config: &flok_core::config::FlokConfig,
) -> Result<SecretString> {
    let provider_config = config.provider.get(config_key).ok_or_else(|| {
        anyhow::anyhow!(
            "No API key found for {config_key}. \
             Run `flok auth login --provider {config_key}` or add \
             `[provider.{config_key}]` `api_key` to ~/.config/flok/flok.toml"
        )
    })?;

    let api_key = provider_config.api_key.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "No API key found for {config_key}. \
             Run `flok auth login --provider {config_key}` or add \
             `[provider.{config_key}]` `api_key` to ~/.config/flok/flok.toml"
        )
    })?;

    if api_key.expose_secret().is_empty() {
        anyhow::bail!(
            "Empty API key for {config_key}. \
             Run `flok auth login --provider {config_key}` or set a non-empty \
             `api_key` in ~/.config/flok/flok.toml"
        );
    }

    Ok(api_key.clone())
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
        println!(
            "\nShorthands: sonnet, opus, haiku, gpt-5.4, chatgpt-5.4, mini, nano, gpt-4.1, flash, pro, deepseek, r1"
        );
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

/// Provider metadata for auth login.
struct ProviderMeta {
    name: &'static str,
    display_name: &'static str,
}

const AUTH_PROVIDERS: &[ProviderMeta] = &[
    ProviderMeta { name: "anthropic", display_name: "Anthropic (Claude)" },
    ProviderMeta { name: "openai", display_name: "OpenAI (GPT-5.4)" },
    ProviderMeta { name: "deepseek", display_name: "DeepSeek (V3 / R1)" },
    ProviderMeta { name: "minimax", display_name: "MiniMax (M2.7)" },
];

/// Run the auth subcommand.
fn run_auth(cmd: &cli::Command) -> Result<()> {
    match cmd {
        cli::Command::Auth { command: cli::AuthCommand::Login { provider } } => {
            run_auth_login(provider.as_ref())
        }
        _ => unreachable!(),
    }
}

/// Interactive auth login.
fn run_auth_login(provider_arg: Option<&String>) -> Result<()> {
    let provider_meta = if let Some(name) = provider_arg {
        AUTH_PROVIDERS.iter().find(|p| p.name == name).with_context(|| {
            format!("unknown provider '{name}' — valid: anthropic, openai, deepseek, minimax")
        })?
    } else {
        let items: Vec<&str> = AUTH_PROVIDERS.iter().map(|p| p.display_name).collect();
        let sel = dialoguer::Select::new()
            .with_prompt("Select a provider")
            .items(&items)
            .default(0)
            .interact()?;
        &AUTH_PROVIDERS[sel]
    };

    let api_key = dialoguer::Password::new()
        .with_prompt(format!("Enter your {} API key", provider_meta.display_name))
        .allow_empty_password(false)
        .interact()?;
    let api_key = api_key.trim().to_string();

    if api_key.is_empty() {
        anyhow::bail!("API key cannot be empty");
    }

    let secret = SecretString::from(api_key);

    let config_path = {
        let dirs = directories::BaseDirs::new().context("cannot determine home directory")?;
        dirs.config_dir().join("flok").join("flok.toml")
    };

    let mut config: flok_core::config::FlokConfig = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        toml::from_str(&content).unwrap_or_default()
    } else {
        flok_core::config::FlokConfig::default()
    };

    // Preserve any existing provider fields (e.g. base_url, default_model) when
    // updating just the api_key.
    let entry = config.provider.entry(provider_meta.name.to_string()).or_default();
    entry.api_key = Some(secret);

    write_auth_config(&config_path, &config)?;

    #[allow(clippy::print_stdout)]
    {
        println!("✓ Saved {} API key to {}", provider_meta.display_name, config_path.display());
    }

    Ok(())
}

/// Write a `FlokConfig` to `path`, creating parent dirs, then (on Unix) chmod 0600.
fn write_auth_config(path: &std::path::Path, config: &flok_core::config::FlokConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(config)?;
    std::fs::write(path, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

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

#[cfg(test)]
mod credential_tests {
    use super::*;
    use flok_core::config::{FlokConfig, ProviderConfig};
    use secrecy::{ExposeSecret, SecretString};
    use std::collections::HashMap;

    fn config_with_key(provider: &str, key: &str) -> FlokConfig {
        let mut provider_map = HashMap::new();
        provider_map.insert(
            provider.to_string(),
            ProviderConfig {
                api_key: Some(SecretString::from(key.to_string())),
                base_url: None,
                default_model: None,
            },
        );
        FlokConfig { provider: provider_map, ..Default::default() }
    }

    #[test]
    fn resolve_api_key_reads_from_config() {
        let config = config_with_key("anthropic", "sk-test-ok");
        let key = resolve_api_key("anthropic", &config).expect("resolved");
        assert_eq!(key.expose_secret(), "sk-test-ok");
    }

    #[test]
    fn resolve_api_key_errors_when_missing() {
        let config = FlokConfig::default();
        let err = resolve_api_key("anthropic", &config).expect_err("expected error");
        let msg = err.to_string();
        assert!(
            msg.contains("flok auth login --provider"),
            "error should direct user to flok auth login, got: {msg}"
        );
        assert!(
            !msg.to_lowercase().contains("environment variable")
                && !msg.to_lowercase().contains("env var")
                && !msg.contains("ANTHROPIC_API_KEY")
                && !msg.contains("OPENAI_API_KEY"),
            "error should not mention env vars, got: {msg}"
        );
    }

    #[test]
    fn resolve_api_key_errors_when_key_empty() {
        let config = config_with_key("anthropic", "");
        let err = resolve_api_key("anthropic", &config).expect_err("expected error for empty key");
        assert!(err.to_string().contains("Empty API key"));
    }

    #[test]
    fn secret_string_debug_is_redacted() {
        let s = SecretString::from("plain-text-xyz".to_string());
        let rendered = format!("{s:?}");
        assert!(!rendered.contains("plain-text-xyz"), "Debug leaked plaintext: {rendered}");
        assert!(rendered.contains("REDACTED"), "Debug missing REDACTED marker: {rendered}");
    }

    #[test]
    fn build_provider_registry_includes_configured_providers_with_keys() {
        let mut provider = HashMap::new();
        provider.insert(
            "anthropic".to_string(),
            ProviderConfig {
                api_key: Some(SecretString::from("sk-ant".to_string())),
                base_url: None,
                default_model: Some("opus-4.7".into()),
            },
        );
        provider.insert(
            "openai".to_string(),
            ProviderConfig {
                api_key: Some(SecretString::from("sk-openai".to_string())),
                base_url: None,
                default_model: Some("gpt-5.4".into()),
            },
        );
        provider.insert(
            "minimax".to_string(),
            ProviderConfig { api_key: None, base_url: None, default_model: Some("minimax".into()) },
        );

        let config = FlokConfig { provider, ..Default::default() };
        let registry = build_provider_registry(&config).expect("provider registry");

        assert_eq!(registry.configured_providers(), vec!["anthropic", "openai"]);
        assert_eq!(registry.default_model("anthropic"), Some("anthropic/claude-opus-4-7"));
        assert_eq!(registry.default_model("openai"), Some("openai/gpt-5.4"));
        assert!(registry.semaphore("anthropic").is_some());
        assert!(registry.semaphore("openai").is_some());
        assert!(registry.semaphore("minimax").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn run_auth_login_sets_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("flok").join("flok.toml");
        let config = config_with_key("anthropic", "sk-permtest");
        write_auth_config(&path, &config).expect("write");
        let mode = std::fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
    }
}
