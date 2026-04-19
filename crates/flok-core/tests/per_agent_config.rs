use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use flok_core::agent;
use flok_core::bus::Bus;
use flok_core::config::{AgentConfig, FlokConfig, WorktreeConfig};
use flok_core::provider::{CompletionRequest, Provider, ProviderRegistry, StreamEvent};
use flok_core::team::TeamRegistry;
use flok_core::tool::{TaskTool, Tool, ToolContext, ToolRegistry};
use flok_core::worktree::WorktreeManager;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy)]
enum ProviderBehavior {
    Text(&'static str),
    Status(u16),
}

#[derive(Debug)]
struct RecordingProvider {
    name: &'static str,
    behavior: ProviderBehavior,
    seen_models: Mutex<Vec<String>>,
    seen_systems: Mutex<Vec<String>>,
    calls: Mutex<u32>,
}

impl RecordingProvider {
    fn new(name: &'static str, behavior: ProviderBehavior) -> Self {
        Self {
            name,
            behavior,
            seen_models: Mutex::new(Vec::new()),
            seen_systems: Mutex::new(Vec::new()),
            calls: Mutex::new(0),
        }
    }

    fn seen_models(&self) -> Vec<String> {
        self.seen_models.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    fn seen_systems(&self) -> Vec<String> {
        self.seen_systems.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    fn call_count(&self) -> u32 {
        *self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait::async_trait]
impl Provider for RecordingProvider {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        tx: mpsc::UnboundedSender<StreamEvent>,
    ) -> anyhow::Result<()> {
        self.seen_models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(request.model);
        self.seen_systems
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(request.system);
        *self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner) += 1;

        match self.behavior {
            ProviderBehavior::Text(text) => {
                let _ = tx.send(StreamEvent::TextDelta(text.to_string()));
                let _ = tx.send(StreamEvent::Done);
            }
            ProviderBehavior::Status(status) => {
                let _ = tx.send(StreamEvent::Error(format!("HTTP {status}: synthetic failure")));
            }
        }

        Ok(())
    }
}

fn parse_agents_config(toml_str: &str) -> std::collections::HashMap<String, AgentConfig> {
    toml::from_str::<FlokConfig>(toml_str).expect("config parses").agents
}

fn test_context(project_root: PathBuf) -> ToolContext {
    ToolContext {
        project_root,
        session_id: "session-1".to_string(),
        agent: "lead".to_string(),
        cancel: tokio_util::sync::CancellationToken::new(),
        lsp: None,
        output_compression: flok_core::config::OutputCompressionConfig::default(),
    }
}

fn make_task_tool(
    provider_registry: Arc<ProviderRegistry>,
    default_provider: &str,
    default_model_id: &str,
    agents_config: std::collections::HashMap<String, AgentConfig>,
    project_root: PathBuf,
) -> TaskTool {
    TaskTool::new(
        provider_registry,
        default_provider.to_string(),
        default_model_id.to_string(),
        agents_config,
        Arc::new(ToolRegistry::new()),
        Bus::new(16),
        project_root.clone(),
        Arc::new(WorktreeManager::new("test-project", project_root)),
        WorktreeConfig { enabled: false, ..WorktreeConfig::default() },
        TeamRegistry::new(),
    )
}

#[tokio::test]
async fn agent_config_overrides_default_model() {
    let anthropic = Arc::new(RecordingProvider::new("anthropic", ProviderBehavior::Text("ok")));
    let anthropic_dyn: Arc<dyn Provider> = anthropic.clone();
    let mut registry = ProviderRegistry::new();
    registry.insert("anthropic", anthropic_dyn, Some("anthropic/claude-sonnet-4-6".into()), 3);

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let project_root = std::fs::canonicalize(temp_dir.path()).expect("canonical project root");
    let tool = make_task_tool(
        Arc::new(registry),
        "anthropic",
        "anthropic/claude-sonnet-4-6",
        parse_agents_config(
            r#"
            [agents.explore]
            model = "haiku"
            "#,
        ),
        project_root.clone(),
    );

    let output = tool
        .execute(
            serde_json::json!({
                "description": "agent override",
                "prompt": "respond once",
                "subagent_type": "explore"
            }),
            &test_context(project_root),
        )
        .await
        .expect("task succeeds");

    assert!(!output.is_error);
    assert_eq!(anthropic.seen_models(), vec!["anthropic/claude-haiku-4-5-20251001".to_string()]);
}

#[tokio::test]
async fn agent_fallback_chain_replaces_provider_chain() {
    let anthropic = Arc::new(RecordingProvider::new("anthropic", ProviderBehavior::Status(529)));
    let openai = Arc::new(RecordingProvider::new("openai", ProviderBehavior::Text("openai ok")));
    let minimax = Arc::new(RecordingProvider::new("minimax", ProviderBehavior::Text("minimax ok")));
    let anthropic_dyn: Arc<dyn Provider> = anthropic.clone();
    let openai_dyn: Arc<dyn Provider> = openai.clone();
    let minimax_dyn: Arc<dyn Provider> = minimax.clone();

    let mut registry = ProviderRegistry::new();
    registry.set_fallback_chains(
        [("anthropic".to_string(), vec!["minimax".to_string()])].into_iter().collect(),
    );
    registry.insert("anthropic", anthropic_dyn, Some("anthropic/claude-sonnet-4-6".into()), 3);
    registry.insert("openai", openai_dyn, Some("openai/gpt-5.4".into()), 3);
    registry.insert("minimax", minimax_dyn, Some("minimax/MiniMax-M2.7".into()), 3);

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let project_root = std::fs::canonicalize(temp_dir.path()).expect("canonical project root");
    let tool = make_task_tool(
        Arc::new(registry),
        "anthropic",
        "anthropic/claude-sonnet-4-6",
        parse_agents_config(
            r#"
            [agents.general]
            fallback_models = ["openai/gpt-5.4"]
            "#,
        ),
        project_root.clone(),
    );

    let output = tool
        .execute(
            serde_json::json!({
                "description": "agent fallback",
                "prompt": "respond once",
                "subagent_type": "general"
            }),
            &test_context(project_root),
        )
        .await
        .expect("task succeeds");

    assert!(!output.is_error);
    assert_eq!(output.content, "openai ok");
    assert_eq!(anthropic.call_count(), 1);
    assert_eq!(openai.call_count(), 1);
    assert_eq!(minimax.call_count(), 0);
}

#[tokio::test]
async fn agent_prompt_append_appears_in_system_prompt() {
    let anthropic = Arc::new(RecordingProvider::new("anthropic", ProviderBehavior::Text("ok")));
    let anthropic_dyn: Arc<dyn Provider> = anthropic.clone();
    let mut registry = ProviderRegistry::new();
    registry.insert("anthropic", anthropic_dyn, Some("anthropic/claude-sonnet-4-6".into()), 3);

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let project_root = std::fs::canonicalize(temp_dir.path()).expect("canonical project root");
    let append = "Pay extra attention to operational risks.";
    let tool = make_task_tool(
        Arc::new(registry),
        "anthropic",
        "anthropic/claude-sonnet-4-6",
        parse_agents_config(&format!(
            r#"
            [agents.explore]
            prompt_append = "{append}"
            "#
        )),
        project_root.clone(),
    );

    let output = tool
        .execute(
            serde_json::json!({
                "description": "prompt append",
                "prompt": "respond once",
                "subagent_type": "explore"
            }),
            &test_context(project_root),
        )
        .await
        .expect("task succeeds");

    assert!(!output.is_error);
    let system = anthropic.seen_systems().into_iter().next().expect("system recorded");
    assert!(system.contains(
        agent::get_subagent("explore").expect("agent exists").system_prompt.expect("prompt exists")
    ));
    assert!(system.contains(append));
}

#[tokio::test]
async fn task_model_param_overrides_agent_config() {
    let anthropic = Arc::new(RecordingProvider::new("anthropic", ProviderBehavior::Text("ok")));
    let anthropic_dyn: Arc<dyn Provider> = anthropic.clone();
    let mut registry = ProviderRegistry::new();
    registry.insert("anthropic", anthropic_dyn, Some("anthropic/claude-sonnet-4-6".into()), 3);

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let project_root = std::fs::canonicalize(temp_dir.path()).expect("canonical project root");
    let tool = make_task_tool(
        Arc::new(registry),
        "anthropic",
        "anthropic/claude-sonnet-4-6",
        parse_agents_config(
            r#"
            [agents.explore]
            model = "haiku"
            "#,
        ),
        project_root.clone(),
    );

    let output = tool
        .execute(
            serde_json::json!({
                "description": "explicit model",
                "prompt": "respond once",
                "subagent_type": "explore",
                "model": "opus"
            }),
            &test_context(project_root),
        )
        .await
        .expect("task succeeds");

    assert!(!output.is_error);
    assert_eq!(anthropic.seen_models(), vec!["anthropic/claude-opus-4-6".to_string()]);
}

#[tokio::test]
async fn unknown_agent_in_config_does_not_error() {
    let anthropic = Arc::new(RecordingProvider::new("anthropic", ProviderBehavior::Text("ok")));
    let anthropic_dyn: Arc<dyn Provider> = anthropic.clone();
    let mut registry = ProviderRegistry::new();
    registry.insert("anthropic", anthropic_dyn, Some("anthropic/claude-sonnet-4-6".into()), 3);

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let project_root = std::fs::canonicalize(temp_dir.path()).expect("canonical project root");
    let tool = make_task_tool(
        Arc::new(registry),
        "anthropic",
        "anthropic/claude-sonnet-4-6",
        parse_agents_config(
            r#"
            [agents.nonexistent]
            model = "opus"

            [agents.explore]
            model = "haiku"
            "#,
        ),
        project_root.clone(),
    );

    let output = tool
        .execute(
            serde_json::json!({
                "description": "unknown agent config",
                "prompt": "respond once",
                "subagent_type": "explore"
            }),
            &test_context(project_root),
        )
        .await
        .expect("task succeeds");

    assert!(!output.is_error);
    assert_eq!(anthropic.seen_models(), vec!["anthropic/claude-haiku-4-5-20251001".to_string()]);
}
