//! Structured warm-memory compaction for long sessions.
//!
//! This is the first typed memory slice: when a session grows large, older
//! messages are summarized into a persisted structured artifact and injected
//! back into prompt assembly as a `Compaction` message part.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::Context;
use chrono::{DateTime, Utc};
use flok_db::MessageRow;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::provider::{
    CompactionSummary, MemoryRecallSummary, MessageContent, ProjectMemorySummary,
};

const MIN_MESSAGES_FOR_COMPACTION: usize = 12;
const RECENT_MESSAGES_TO_KEEP: usize = 8;
const MAX_PROGRESS_ITEMS: usize = 6;
const MAX_TODO_ITEMS: usize = 4;
const MAX_CONSTRAINT_ITEMS: usize = 4;
const MAX_REFERENCED_FILES: usize = 8;
const MAX_ITEM_LEN: usize = 160;
const MAX_RECALL_MATCHES: usize = 3;

/// Persisted structured warm-memory artifact for one session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionCompaction {
    pub session_id: String,
    pub covered_message_count: usize,
    pub covered_through_message_id: String,
    pub summary: CompactionSummary,
    pub generated_at: DateTime<Utc>,
}

/// Result of attempting to refresh structured warm memory for a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionRefresh {
    pub compaction: SessionCompaction,
    pub refreshed: bool,
}

/// Result of attempting to refresh project-level memory for a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMemoryRefresh {
    pub summary: ProjectMemorySummary,
    pub refreshed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ProjectMemorySource {
    session_id: String,
    covered_through_message_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ProjectMemoryArtifact {
    sources: Vec<ProjectMemorySource>,
    summary: ProjectMemorySummary,
    generated_at: DateTime<Utc>,
}

/// JSON-backed store for structured session compactions.
#[derive(Debug, Clone)]
pub struct CompactionStore {
    project_root: PathBuf,
}

impl CompactionStore {
    #[must_use]
    pub fn new(project_root: PathBuf) -> Self {
        Self { project_root }
    }

    pub fn refresh_session(
        &self,
        session_id: &str,
        rows: &[MessageRow],
    ) -> anyhow::Result<Option<CompactionRefresh>> {
        let Some((covered_message_count, covered_through_message_id)) = compaction_target(rows)
        else {
            return Ok(None);
        };

        if let Some(existing) = self.load_session(session_id)? {
            if existing.covered_message_count == covered_message_count
                && existing.covered_through_message_id == covered_through_message_id
            {
                return Ok(Some(CompactionRefresh { compaction: existing, refreshed: false }));
            }
        }

        let summary = summarize_rows(&rows[..covered_message_count])?;
        let compaction = SessionCompaction {
            session_id: session_id.to_string(),
            covered_message_count,
            covered_through_message_id,
            summary,
            generated_at: Utc::now(),
        };
        self.save_session(&compaction)?;
        Ok(Some(CompactionRefresh { compaction, refreshed: true }))
    }

    pub fn load_session(&self, session_id: &str) -> anyhow::Result<Option<SessionCompaction>> {
        let path = self.compaction_path(session_id);
        if !path.exists() {
            return Ok(None);
        }

        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let compaction = serde_json::from_str(&content)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        Ok(Some(compaction))
    }

    pub fn save_session(&self, compaction: &SessionCompaction) -> anyhow::Result<()> {
        std::fs::create_dir_all(self.compactions_dir())?;
        let path = self.compaction_path(&compaction.session_id);
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(compaction)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn refresh_project_memory(
        &self,
        current_session_id: &str,
    ) -> anyhow::Result<Option<ProjectMemoryRefresh>> {
        let Some((sources, session_compactions)) =
            self.project_memory_sources(current_session_id)?
        else {
            return Ok(None);
        };

        if let Some(existing) = self.load_project_memory_artifact()? {
            if existing.sources == sources {
                return Ok(Some(ProjectMemoryRefresh {
                    summary: existing.summary,
                    refreshed: false,
                }));
            }
        }

        let mut progress = Vec::new();
        let mut todos = Vec::new();
        let mut constraints = Vec::new();
        let mut referenced_files = Vec::new();
        let mut seen_files = BTreeSet::new();

        for compaction in session_compactions {
            for item in compaction.summary.progress {
                push_unique_limited(&mut progress, item, MAX_PROGRESS_ITEMS);
            }
            for item in compaction.summary.todos {
                push_unique_limited(&mut todos, item, MAX_TODO_ITEMS);
            }
            for item in compaction.summary.constraints {
                push_unique_limited(&mut constraints, item, MAX_CONSTRAINT_ITEMS);
            }
            for item in compaction.summary.referenced_files {
                push_file(&mut seen_files, &mut referenced_files, &item);
            }
        }

        let summary = ProjectMemorySummary {
            source_sessions: sources.len(),
            summary: CompactionSummary {
                goal: format!("Project context gathered from {} prior sessions", sources.len()),
                progress,
                todos,
                constraints,
                referenced_files,
            },
        };
        self.save_project_memory_artifact(&ProjectMemoryArtifact {
            sources,
            summary: summary.clone(),
            generated_at: Utc::now(),
        })?;
        Ok(Some(ProjectMemoryRefresh { summary, refreshed: true }))
    }

    pub fn recall_memory(
        &self,
        current_session_id: &str,
        query: &str,
    ) -> anyhow::Result<Option<MemoryRecallSummary>> {
        let query = compact_text(query);
        let query_tokens = tokenize_for_recall(&query);
        if query_tokens.is_empty() {
            return Ok(None);
        }

        let Some((_, session_compactions)) = self.project_memory_sources(current_session_id)?
        else {
            return Ok(None);
        };

        let mut ranked = session_compactions
            .into_iter()
            .filter_map(|compaction| {
                let score = recall_score(&query_tokens, &compaction.summary);
                if score == 0 {
                    None
                } else {
                    Some((score, compaction))
                }
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|left, right| {
            right.0.cmp(&left.0).then_with(|| left.1.session_id.cmp(&right.1.session_id))
        });
        ranked.truncate(MAX_RECALL_MATCHES);
        if ranked.is_empty() {
            return Ok(None);
        }

        let mut progress = Vec::new();
        let mut todos = Vec::new();
        let mut constraints = Vec::new();
        let mut referenced_files = Vec::new();
        let mut seen_files = BTreeSet::new();
        for (_, compaction) in &ranked {
            push_unique_limited(
                &mut progress,
                format!("{}: {}", compaction.session_id, compaction.summary.goal),
                MAX_PROGRESS_ITEMS,
            );
            for item in &compaction.summary.progress {
                push_unique_limited(&mut progress, item.clone(), MAX_PROGRESS_ITEMS);
            }
            for item in &compaction.summary.todos {
                push_unique_limited(&mut todos, item.clone(), MAX_TODO_ITEMS);
            }
            for item in &compaction.summary.constraints {
                push_unique_limited(&mut constraints, item.clone(), MAX_CONSTRAINT_ITEMS);
            }
            for item in &compaction.summary.referenced_files {
                push_file(&mut seen_files, &mut referenced_files, item);
            }
        }

        Ok(Some(MemoryRecallSummary {
            query,
            matched_sessions: ranked.len(),
            summary: CompactionSummary {
                goal: "Relevant prior session memory retrieved for the current request."
                    .to_string(),
                progress,
                todos,
                constraints,
                referenced_files,
            },
        }))
    }

    #[must_use]
    pub fn compaction_path(&self, session_id: &str) -> PathBuf {
        self.compactions_dir().join(format!("{session_id}.json"))
    }

    fn compactions_dir(&self) -> PathBuf {
        self.project_root.join(".flok").join("compactions")
    }

    fn project_memory_path(&self) -> PathBuf {
        self.compactions_dir().join("project-memory.json")
    }

    fn load_project_memory_artifact(&self) -> anyhow::Result<Option<ProjectMemoryArtifact>> {
        let path = self.project_memory_path();
        if !path.exists() {
            return Ok(None);
        }

        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let artifact = serde_json::from_str(&content)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        Ok(Some(artifact))
    }

    fn save_project_memory_artifact(&self, artifact: &ProjectMemoryArtifact) -> anyhow::Result<()> {
        std::fs::create_dir_all(self.compactions_dir())?;
        let path = self.project_memory_path();
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(artifact)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    fn project_memory_sources(
        &self,
        current_session_id: &str,
    ) -> anyhow::Result<Option<(Vec<ProjectMemorySource>, Vec<SessionCompaction>)>> {
        let compactions_dir = self.compactions_dir();
        if !compactions_dir.exists() {
            return Ok(None);
        }

        let mut sources = Vec::new();
        let mut session_compactions = Vec::new();

        for entry in std::fs::read_dir(&compactions_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(std::ffi::OsStr::to_str) != Some("json") {
                continue;
            }

            let Some(stem) = path.file_stem().and_then(std::ffi::OsStr::to_str) else {
                continue;
            };
            if stem == current_session_id || stem == "project-memory" {
                continue;
            }

            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            let compaction: SessionCompaction = serde_json::from_str(&content)
                .with_context(|| format!("failed to parse {}", path.display()))?;
            sources.push(ProjectMemorySource {
                session_id: compaction.session_id.clone(),
                covered_through_message_id: compaction.covered_through_message_id.clone(),
            });
            session_compactions.push(compaction);
        }

        if sources.is_empty() {
            return Ok(None);
        }

        sources.sort_by(|left, right| left.session_id.cmp(&right.session_id));
        session_compactions.sort_by(|left, right| left.session_id.cmp(&right.session_id));
        Ok(Some((sources, session_compactions)))
    }
}

fn compaction_target(rows: &[MessageRow]) -> Option<(usize, String)> {
    if rows.len() < MIN_MESSAGES_FOR_COMPACTION {
        return None;
    }

    let covered_message_count = rows.len().saturating_sub(RECENT_MESSAGES_TO_KEEP);
    if covered_message_count == 0 {
        return None;
    }

    Some((covered_message_count, rows[covered_message_count - 1].id.clone()))
}

fn summarize_rows(rows: &[MessageRow]) -> anyhow::Result<CompactionSummary> {
    let mut goal = None;
    let mut progress = Vec::new();
    let mut todos = Vec::new();
    let mut constraints = Vec::new();
    let mut referenced_files = Vec::new();
    let mut seen_files = BTreeSet::new();
    let file_pattern = Regex::new(
        r"(?x)
        \b
        [A-Za-z0-9_./-]+
        \.
        (rs|py|pyi|js|jsx|ts|tsx|go|toml|json|ya?ml|md|sql|sh)
        \b",
    )
    .expect("file regex is valid");

    let mut user_texts = Vec::new();
    let mut assistant_texts = Vec::new();

    for row in rows {
        let parts: Vec<MessageContent> = serde_json::from_str(&row.parts)
            .with_context(|| format!("failed to parse message parts for {}", row.id))?;

        for part in parts {
            match part {
                MessageContent::Text { text } => {
                    let normalized = compact_text(&text);
                    if normalized.is_empty() {
                        continue;
                    }
                    extract_file_paths(
                        &normalized,
                        &file_pattern,
                        &mut seen_files,
                        &mut referenced_files,
                    );
                    if row.role == "user" {
                        if goal.is_none() {
                            goal = Some(truncate_item(&normalized));
                        }
                        push_unique_limited(&mut user_texts, truncate_item(&normalized), 12);
                        if looks_like_constraint(&normalized) {
                            push_unique_limited(
                                &mut constraints,
                                truncate_item(&normalized),
                                MAX_CONSTRAINT_ITEMS,
                            );
                        }
                    } else if row.role == "assistant" {
                        push_unique_limited(
                            &mut assistant_texts,
                            truncate_item(&normalized),
                            MAX_PROGRESS_ITEMS * 2,
                        );
                    }
                }
                MessageContent::Compaction { summary } => {
                    if goal.is_none() && !summary.goal.is_empty() {
                        goal = Some(summary.goal);
                    }
                    for item in summary.progress {
                        push_unique_limited(&mut progress, item, MAX_PROGRESS_ITEMS);
                    }
                    for item in summary.todos {
                        push_unique_limited(&mut todos, item, MAX_TODO_ITEMS);
                    }
                    for item in summary.constraints {
                        push_unique_limited(&mut constraints, item, MAX_CONSTRAINT_ITEMS);
                    }
                    for item in summary.referenced_files {
                        push_file(&mut seen_files, &mut referenced_files, &item);
                    }
                }
                MessageContent::ProjectMemory { summary } => {
                    for item in summary.summary.progress {
                        push_unique_limited(&mut progress, item, MAX_PROGRESS_ITEMS);
                    }
                    for item in summary.summary.todos {
                        push_unique_limited(&mut todos, item, MAX_TODO_ITEMS);
                    }
                    for item in summary.summary.constraints {
                        push_unique_limited(&mut constraints, item, MAX_CONSTRAINT_ITEMS);
                    }
                    for item in summary.summary.referenced_files {
                        push_file(&mut seen_files, &mut referenced_files, &item);
                    }
                }
                MessageContent::MemoryRecall { summary } => {
                    for item in summary.summary.progress {
                        push_unique_limited(&mut progress, item, MAX_PROGRESS_ITEMS);
                    }
                    for item in summary.summary.todos {
                        push_unique_limited(&mut todos, item, MAX_TODO_ITEMS);
                    }
                    for item in summary.summary.constraints {
                        push_unique_limited(&mut constraints, item, MAX_CONSTRAINT_ITEMS);
                    }
                    for item in summary.summary.referenced_files {
                        push_file(&mut seen_files, &mut referenced_files, &item);
                    }
                }
                MessageContent::Step { step } => {
                    push_unique_limited(
                        &mut progress,
                        truncate_item(&step.summary),
                        MAX_PROGRESS_ITEMS,
                    );
                }
                MessageContent::ToolUse { name, input, .. } => {
                    push_unique_limited(
                        &mut progress,
                        truncate_item(&summarize_tool_use(&name, &input)),
                        MAX_PROGRESS_ITEMS,
                    );
                    collect_paths_from_json(&input, &mut seen_files, &mut referenced_files);
                }
                MessageContent::ToolResult { content, .. } => {
                    extract_file_paths(
                        &content,
                        &file_pattern,
                        &mut seen_files,
                        &mut referenced_files,
                    );
                }
                MessageContent::Thinking { .. } => {}
            }
        }
    }

    if progress.len() < MAX_PROGRESS_ITEMS {
        for item in assistant_texts {
            push_unique_limited(&mut progress, item, MAX_PROGRESS_ITEMS);
        }
    }

    for item in user_texts.iter().rev().take(MAX_TODO_ITEMS).rev() {
        push_unique_limited(&mut todos, item.clone(), MAX_TODO_ITEMS);
    }

    if progress.is_empty() {
        progress.push("Earlier session context was compacted into warm memory.".to_string());
    }
    if todos.is_empty() {
        todos.push("Continue from the most recent raw messages.".to_string());
    }

    Ok(CompactionSummary {
        goal: goal.unwrap_or_else(|| "Continue the active session goal.".to_string()),
        progress,
        todos,
        constraints,
        referenced_files,
    })
}

fn compact_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn tokenize_for_recall(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '/' && c != '.')
        .filter_map(|token| {
            let token = token.trim().to_ascii_lowercase();
            if token.len() < 3 || COMMON_RECALL_STOPWORDS.contains(&token.as_str()) {
                None
            } else {
                Some(token)
            }
        })
        .collect()
}

fn recall_score(query_tokens: &BTreeSet<String>, summary: &CompactionSummary) -> usize {
    let mut haystack = String::new();
    haystack.push_str(&summary.goal);
    haystack.push('\n');
    haystack.push_str(&summary.progress.join("\n"));
    haystack.push('\n');
    haystack.push_str(&summary.todos.join("\n"));
    haystack.push('\n');
    haystack.push_str(&summary.constraints.join("\n"));
    haystack.push('\n');
    haystack.push_str(&summary.referenced_files.join("\n"));
    let haystack_tokens = tokenize_for_recall(&haystack);
    query_tokens.intersection(&haystack_tokens).count()
}

const COMMON_RECALL_STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "this", "that", "from", "into", "when", "then", "have", "has",
    "had", "your", "about", "after", "before", "should", "would", "could", "there", "their",
    "them", "they", "were", "been", "while", "where", "what", "which", "will", "does", "did",
    "done", "make", "made", "using", "into", "also", "only",
];

fn truncate_item(text: &str) -> String {
    if text.len() <= MAX_ITEM_LEN {
        text.to_string()
    } else {
        format!("{}...", &text[..MAX_ITEM_LEN.saturating_sub(3)])
    }
}

fn summarize_tool_use(name: &str, input: &serde_json::Value) -> String {
    if let Some(path) = first_path_hint(input) {
        format!("Used tool `{name}` on {path}")
    } else {
        format!("Used tool `{name}`")
    }
}

fn first_path_hint(input: &serde_json::Value) -> Option<String> {
    match input {
        serde_json::Value::Object(map) => {
            for key in ["file_path", "path", "paths", "pattern"] {
                if let Some(value) = map.get(key) {
                    if let Some(path) = value.as_str() {
                        return Some(path.to_string());
                    }
                    if let Some(first) = value.as_array().and_then(|items| items.first()) {
                        if let Some(path) = first.as_str() {
                            return Some(path.to_string());
                        }
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn collect_paths_from_json(
    value: &serde_json::Value,
    seen_files: &mut BTreeSet<String>,
    referenced_files: &mut Vec<String>,
) {
    match value {
        serde_json::Value::String(text) => {
            if looks_like_path(text) {
                push_file(seen_files, referenced_files, text);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_paths_from_json(item, seen_files, referenced_files);
            }
        }
        serde_json::Value::Object(map) => {
            for value in map.values() {
                collect_paths_from_json(value, seen_files, referenced_files);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

fn extract_file_paths(
    text: &str,
    pattern: &Regex,
    seen_files: &mut BTreeSet<String>,
    referenced_files: &mut Vec<String>,
) {
    for capture in pattern.find_iter(text) {
        push_file(seen_files, referenced_files, capture.as_str());
    }
}

fn looks_like_path(text: &str) -> bool {
    text.contains('/')
        || std::path::Path::new(text)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "rs" | "py" | "ts"))
}

fn push_file(seen_files: &mut BTreeSet<String>, referenced_files: &mut Vec<String>, path: &str) {
    if referenced_files.len() >= MAX_REFERENCED_FILES {
        return;
    }
    if seen_files.insert(path.to_string()) {
        referenced_files.push(path.to_string());
    }
}

fn push_unique_limited(items: &mut Vec<String>, value: String, limit: usize) {
    if value.is_empty() || items.len() >= limit || items.iter().any(|existing| existing == &value) {
        return;
    }
    items.push(value);
}

fn looks_like_constraint(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    ["must", "do not", "don't", "never", "only", "without"]
        .iter()
        .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, role: &str, parts: &[MessageContent]) -> MessageRow {
        MessageRow {
            id: id.to_string(),
            session_id: "session-1".to_string(),
            role: role.to_string(),
            parts: serde_json::to_string(parts).unwrap(),
            created_at: Utc::now().to_rfc3339(),
        }
    }

    #[test]
    fn refresh_session_persists_summary_when_history_is_long() {
        let dir = tempfile::tempdir().unwrap();
        let store = CompactionStore::new(dir.path().to_path_buf());
        let rows = (0..12)
            .map(|idx| {
                let role = if idx % 2 == 0 { "user" } else { "assistant" };
                row(
                    &format!("m-{idx}"),
                    role,
                    &[MessageContent::Text { text: format!("message {idx} for src/lib.rs") }],
                )
            })
            .collect::<Vec<_>>();

        let compaction = store.refresh_session("session-1", &rows).unwrap().unwrap();

        assert!(compaction.refreshed);
        assert_eq!(compaction.compaction.covered_message_count, 4);
        assert!(store.compaction_path("session-1").exists());
        assert_eq!(compaction.compaction.summary.goal, "message 0 for src/lib.rs");
        assert!(compaction.compaction.summary.referenced_files.contains(&"src/lib.rs".to_string()));
    }

    #[test]
    fn refresh_session_reuses_existing_summary_when_prefix_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let store = CompactionStore::new(dir.path().to_path_buf());
        let rows = (0..12)
            .map(|idx| {
                row(
                    &format!("m-{idx}"),
                    if idx % 2 == 0 { "user" } else { "assistant" },
                    &[MessageContent::Text { text: format!("message {idx}") }],
                )
            })
            .collect::<Vec<_>>();

        let first = store.refresh_session("session-1", &rows).unwrap().unwrap();
        let second = store.refresh_session("session-1", &rows).unwrap().unwrap();

        assert!(first.refreshed);
        assert!(!second.refreshed);
        assert_eq!(first.compaction.generated_at, second.compaction.generated_at);
    }

    #[test]
    fn compaction_summary_renders_structured_prompt_text() {
        let summary = CompactionSummary {
            goal: "Ship the feature".to_string(),
            progress: vec!["Added config loading".to_string()],
            todos: vec!["Finish tests".to_string()],
            constraints: vec!["Do not break the gate".to_string()],
            referenced_files: vec!["src/main.rs".to_string()],
        };

        let rendered = summary.render_for_prompt();
        assert!(rendered.contains("[Compaction Summary]"));
        assert!(rendered.contains("Goal:"));
        assert!(rendered.contains("Progress:"));
        assert!(rendered.contains("Referenced Files:"));
    }

    #[test]
    fn refresh_project_memory_aggregates_other_session_compactions() {
        let dir = tempfile::tempdir().unwrap();
        let store = CompactionStore::new(dir.path().to_path_buf());

        store
            .save_session(&SessionCompaction {
                session_id: "session-a".to_string(),
                covered_message_count: 4,
                covered_through_message_id: "m-3".to_string(),
                summary: CompactionSummary {
                    goal: "Goal A".to_string(),
                    progress: vec!["Implemented parser".to_string()],
                    todos: vec!["Add tests".to_string()],
                    constraints: vec!["Do not break APIs".to_string()],
                    referenced_files: vec!["src/lib.rs".to_string()],
                },
                generated_at: Utc::now(),
            })
            .unwrap();
        store
            .save_session(&SessionCompaction {
                session_id: "session-b".to_string(),
                covered_message_count: 4,
                covered_through_message_id: "m-7".to_string(),
                summary: CompactionSummary {
                    goal: "Goal B".to_string(),
                    progress: vec!["Wired verifier".to_string()],
                    todos: vec!["Document commands".to_string()],
                    constraints: vec!["Keep the gate green".to_string()],
                    referenced_files: vec!["src/main.rs".to_string()],
                },
                generated_at: Utc::now(),
            })
            .unwrap();

        let project_memory = store.refresh_project_memory("session-b").unwrap().unwrap();

        assert!(project_memory.refreshed);
        assert_eq!(project_memory.summary.source_sessions, 1);
        assert!(project_memory.summary.summary.goal.contains("1 prior sessions"));
        assert_eq!(project_memory.summary.summary.progress, vec!["Implemented parser".to_string()]);
        assert_eq!(project_memory.summary.summary.todos, vec!["Add tests".to_string()]);
        assert_eq!(
            project_memory.summary.summary.constraints,
            vec!["Do not break APIs".to_string()]
        );
        assert_eq!(project_memory.summary.summary.referenced_files, vec!["src/lib.rs".to_string()]);
        assert!(store.project_memory_path().exists());
    }

    #[test]
    fn refresh_project_memory_reuses_existing_artifact_when_sources_match() {
        let dir = tempfile::tempdir().unwrap();
        let store = CompactionStore::new(dir.path().to_path_buf());

        store
            .save_session(&SessionCompaction {
                session_id: "session-a".to_string(),
                covered_message_count: 4,
                covered_through_message_id: "m-3".to_string(),
                summary: CompactionSummary {
                    goal: "Goal A".to_string(),
                    progress: vec!["Implemented parser".to_string()],
                    todos: vec!["Add tests".to_string()],
                    constraints: vec!["Do not break APIs".to_string()],
                    referenced_files: vec!["src/lib.rs".to_string()],
                },
                generated_at: Utc::now(),
            })
            .unwrap();

        let first = store.refresh_project_memory("session-z").unwrap().unwrap();
        let first_artifact = store.load_project_memory_artifact().unwrap().unwrap();
        let second = store.refresh_project_memory("session-z").unwrap().unwrap();
        let second_artifact = store.load_project_memory_artifact().unwrap().unwrap();

        assert!(first.refreshed);
        assert!(!second.refreshed);
        assert_eq!(first.summary, second.summary);
        assert_eq!(first_artifact.generated_at, second_artifact.generated_at);
    }

    #[test]
    fn recall_memory_returns_relevant_prior_session() {
        let dir = tempfile::tempdir().unwrap();
        let store = CompactionStore::new(dir.path().to_path_buf());

        store
            .save_session(&SessionCompaction {
                session_id: "session-parser".to_string(),
                covered_message_count: 4,
                covered_through_message_id: "m-3".to_string(),
                summary: CompactionSummary {
                    goal: "Implement parser refactor".to_string(),
                    progress: vec!["Added parser state machine".to_string()],
                    todos: vec!["Wire parser tests".to_string()],
                    constraints: vec!["Do not break lexer".to_string()],
                    referenced_files: vec!["src/parser.rs".to_string()],
                },
                generated_at: Utc::now(),
            })
            .unwrap();
        store
            .save_session(&SessionCompaction {
                session_id: "session-ui".to_string(),
                covered_message_count: 4,
                covered_through_message_id: "m-7".to_string(),
                summary: CompactionSummary {
                    goal: "Adjust TUI colors".to_string(),
                    progress: vec!["Changed footer palette".to_string()],
                    todos: vec!["Tune sidebar spacing".to_string()],
                    constraints: vec!["Keep mobile layout".to_string()],
                    referenced_files: vec!["src/app.rs".to_string()],
                },
                generated_at: Utc::now(),
            })
            .unwrap();

        let recall = store
            .recall_memory("session-current", "finish parser tests in src/parser.rs")
            .unwrap()
            .unwrap();

        assert_eq!(recall.matched_sessions, 1);
        assert_eq!(recall.query, "finish parser tests in src/parser.rs");
        assert!(recall.summary.progress.iter().any(|item| item.contains("session-parser")));
        assert!(recall.summary.todos.contains(&"Wire parser tests".to_string()));
        assert_eq!(recall.summary.referenced_files, vec!["src/parser.rs".to_string()]);
    }
}
