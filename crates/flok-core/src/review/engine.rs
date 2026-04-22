//! Review engine — orchestrates multi-agent code review.

use std::sync::Arc;

use crate::provider::{CompletionRequest, Message, MessageContent, Provider, StreamEvent};
use crate::review::prompts::ReviewerType;
use crate::review::types::{Finding, FindingKind, FindingPriority, ReviewResult, Verdict};

use tokio::sync::mpsc;

/// The code review engine. Orchestrates specialist reviewer agents to
/// produce a structured review of a code diff.
pub struct ReviewEngine {
    provider: Arc<dyn Provider>,
}

impl ReviewEngine {
    /// Create a new review engine.
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        Self { provider }
    }

    /// Run a code review on the given diff.
    ///
    /// Spawns specialist reviewer sub-agents in parallel, collects their
    /// findings, deduplicates, and produces a structured `ReviewResult`.
    ///
    /// # Arguments
    ///
    /// * `diff` — The unified diff text to review.
    /// * `context` — Optional additional context (PR description, file contents, etc.).
    pub async fn review(&self, diff: &str, context: Option<&str>) -> anyhow::Result<ReviewResult> {
        let changed_lines =
            diff.lines().filter(|l| l.starts_with('+') || l.starts_with('-')).count();
        let reviewers = ReviewerType::select_for_size(changed_lines);

        tracing::info!(
            changed_lines,
            reviewer_count = reviewers.len(),
            reviewers = ?reviewers.iter().map(|r| r.name()).collect::<Vec<_>>(),
            "starting code review"
        );

        // Build the user prompt with diff and context
        let mut user_prompt = String::new();
        if let Some(ctx) = context {
            user_prompt.push_str("## PR Context\n\n");
            user_prompt.push_str(ctx);
            user_prompt.push_str("\n\n");
        }
        user_prompt.push_str("## Diff to Review\n\n```diff\n");
        // Truncate very large diffs to avoid exceeding context limits
        if diff.len() > 50_000 {
            user_prompt.push_str(&diff[..50_000]);
            user_prompt.push_str("\n\n... [diff truncated] ...\n");
        } else {
            user_prompt.push_str(diff);
        }
        user_prompt.push_str("\n```\n\nReview this diff and report your findings in the JSON format specified in your instructions.");

        // Spawn all reviewers concurrently
        let mut handles = Vec::new();
        for reviewer in &reviewers {
            let provider = Arc::clone(&self.provider);
            let system = reviewer.system_prompt().to_string();
            let prompt = user_prompt.clone();
            let reviewer_name = reviewer.name().to_string();

            let handle = tokio::spawn(async move {
                let result = run_single_reviewer(provider, &system, &prompt).await;
                (reviewer_name, result)
            });
            handles.push(handle);
        }

        // Collect results
        let mut all_findings: Vec<Finding> = Vec::new();
        let mut summaries: Vec<String> = Vec::new();

        for handle in handles {
            match handle.await {
                Ok((reviewer_name, Ok(response))) => {
                    match parse_reviewer_response(&response, &reviewer_name) {
                        Ok((mut findings, summary)) => {
                            tracing::debug!(
                                reviewer = %reviewer_name,
                                findings = findings.len(),
                                "reviewer completed"
                            );
                            all_findings.append(&mut findings);
                            if let Some(s) = summary {
                                summaries.push(format!("**{reviewer_name}**: {s}"));
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                reviewer = %reviewer_name,
                                error = %e,
                                "failed to parse reviewer response, using raw text"
                            );
                            // Fall back: treat the whole response as a single finding
                            all_findings.push(Finding {
                                priority: FindingPriority::Medium,
                                kind: FindingKind::Thought,
                                file: None,
                                line: None,
                                title: format!("{reviewer_name} review"),
                                description: truncate_str(&response, 2000),
                                reviewer: reviewer_name,
                            });
                        }
                    }
                }
                Ok((reviewer_name, Err(e))) => {
                    tracing::error!(reviewer = %reviewer_name, error = %e, "reviewer failed");
                }
                Err(e) => {
                    tracing::error!(error = %e, "reviewer task panicked");
                }
            }
        }

        // Sort findings by priority (critical first)
        all_findings.sort_by_key(|f| f.priority);

        // Deduplicate: remove findings with very similar titles from different reviewers
        dedup_findings(&mut all_findings);

        // Determine verdict
        let has_critical = all_findings
            .iter()
            .any(|f| f.priority == FindingPriority::Critical && f.kind.is_actionable());
        let has_high = all_findings
            .iter()
            .any(|f| f.priority == FindingPriority::High && f.kind.is_actionable());
        let verdict =
            if has_critical || has_high { Verdict::RequestChanges } else { Verdict::Approve };

        let summary = if summaries.is_empty() {
            format!("{verdict}: {changed_lines} lines reviewed by {} specialists.", reviewers.len())
        } else {
            summaries.join("\n\n")
        };

        let result = ReviewResult { verdict, findings: all_findings, summary };
        tracing::info!(
            verdict = %result.verdict,
            findings = result.findings.len(),
            critical = result.count_by_priority(FindingPriority::Critical),
            high = result.count_by_priority(FindingPriority::High),
            "code review complete"
        );

        Ok(result)
    }
}

/// Run a single reviewer agent and return its raw text response.
async fn run_single_reviewer(
    provider: Arc<dyn Provider>,
    system: &str,
    prompt: &str,
) -> anyhow::Result<String> {
    let request = CompletionRequest {
        model: String::new(),
        reasoning_effort: None,
        system: system.to_string(),
        messages: vec![Message {
            role: "user".into(),
            content: vec![MessageContent::Text { text: prompt.to_string() }],
        }],
        tools: Vec::new(),
        max_tokens: 4096,
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<StreamEvent>();

    tokio::spawn(async move {
        if let Err(e) = provider.stream(request, tx).await {
            tracing::error!("Reviewer stream error: {e}");
        }
    });

    let mut text = String::new();
    let timeout = std::time::Duration::from_secs(60);

    loop {
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Some(StreamEvent::TextDelta(delta))) => text.push_str(&delta),
            Ok(Some(StreamEvent::Done) | None) => break,
            Ok(Some(StreamEvent::Error(e))) => return Err(anyhow::anyhow!("Reviewer error: {e}")),
            Ok(Some(_)) => {}
            Err(_) => return Err(anyhow::anyhow!("Reviewer timeout")),
        }
    }

    Ok(text)
}

/// Parse a reviewer's JSON response into findings and summary.
fn parse_reviewer_response(
    response: &str,
    reviewer_name: &str,
) -> anyhow::Result<(Vec<Finding>, Option<String>)> {
    // Find JSON block in the response (may be wrapped in markdown code fences)
    let json_str = extract_json_block(response)
        .ok_or_else(|| anyhow::anyhow!("no JSON block found in reviewer response"))?;

    let parsed: serde_json::Value = serde_json::from_str(json_str)?;

    let mut findings = Vec::new();
    if let Some(arr) = parsed["findings"].as_array() {
        for item in arr {
            let priority = match item["priority"].as_str().unwrap_or("medium") {
                "critical" => FindingPriority::Critical,
                "high" => FindingPriority::High,
                "low" => FindingPriority::Low,
                _ => FindingPriority::Medium,
            };
            let kind = match item["kind"].as_str().unwrap_or("suggestion") {
                "bug" => FindingKind::Bug,
                "nitpick" => FindingKind::Nitpick,
                "thought" => FindingKind::Thought,
                "question" => FindingKind::Question,
                "risk" => FindingKind::Risk,
                _ => FindingKind::Suggestion,
            };

            findings.push(Finding {
                priority,
                kind,
                file: item["file"].as_str().map(String::from),
                line: item["line"].as_str().map(String::from),
                title: item["title"].as_str().unwrap_or("Untitled").to_string(),
                description: item["description"].as_str().unwrap_or("").to_string(),
                reviewer: reviewer_name.to_string(),
            });
        }
    }

    let summary = parsed["summary"].as_str().map(String::from);
    Ok((findings, summary))
}

/// Extract a JSON block from text that may contain markdown code fences.
fn extract_json_block(text: &str) -> Option<&str> {
    // Try to find ```json ... ``` block first
    if let Some(start) = text.find("```json") {
        let json_start = start + 7;
        if let Some(end) = text[json_start..].find("```") {
            return Some(text[json_start..json_start + end].trim());
        }
    }

    // Try to find ``` ... ``` block
    if let Some(start) = text.find("```") {
        let code_start = start + 3;
        // Skip language tag if present
        let content_start =
            text[code_start..].find('\n').map_or(code_start, |nl| code_start + nl + 1);
        if let Some(end) = text[content_start..].find("```") {
            return Some(text[content_start..content_start + end].trim());
        }
    }

    // Try to find raw JSON object
    let trimmed = text.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Some(trimmed);
    }

    // Try to find JSON object anywhere in the text
    if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            if end > start {
                return Some(&text[start..=end]);
            }
        }
    }

    None
}

/// Remove near-duplicate findings (same title from different reviewers).
fn dedup_findings(findings: &mut Vec<Finding>) {
    let mut seen_titles: Vec<String> = Vec::new();
    findings.retain(|f| {
        let normalized = f.title.to_lowercase();
        // Check if we've seen a very similar title
        for existing in &seen_titles {
            if titles_similar(existing, &normalized) {
                return false;
            }
        }
        seen_titles.push(normalized);
        true
    });
}

/// Check if two titles are similar enough to be considered duplicates.
fn titles_similar(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }

    // Simple word overlap check
    let a_words: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let b_words: std::collections::HashSet<&str> = b.split_whitespace().collect();

    if a_words.is_empty() || b_words.is_empty() {
        return false;
    }

    let intersection = a_words.intersection(&b_words).count();
    let max_len = a_words.len().max(b_words.len());

    // If >70% word overlap, consider them duplicates
    intersection as f64 / max_len as f64 > 0.7
}

/// Truncate a string to `max_len`, adding "..." if truncated.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_json_from_markdown() {
        let text =
            "Here's my review:\n\n```json\n{\"findings\": [], \"summary\": \"ok\"}\n```\n\nDone!";
        let json = extract_json_block(text).unwrap();
        assert!(json.starts_with('{'));
        let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(parsed["summary"], "ok");
    }

    #[test]
    fn extract_json_raw_object() {
        let text = "{\"findings\": [{\"priority\": \"high\"}], \"summary\": \"issues\"}";
        let json = extract_json_block(text).unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(json).is_ok());
    }

    #[test]
    fn extract_json_embedded_in_text() {
        let text = "My analysis: {\"findings\": [], \"summary\": \"clean\"} end.";
        let json = extract_json_block(text).unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(json).is_ok());
    }

    #[test]
    fn parse_reviewer_response_valid() {
        let response = r#"```json
{
  "findings": [
    {
      "priority": "high",
      "kind": "bug",
      "file": "src/main.rs",
      "line": "42",
      "title": "Off by one",
      "description": "Loop iterates too many times"
    }
  ],
  "summary": "Found one bug"
}
```"#;

        let (findings, summary) = parse_reviewer_response(response, "correctness").unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].priority, FindingPriority::High);
        assert_eq!(findings[0].kind, FindingKind::Bug);
        assert_eq!(findings[0].reviewer, "correctness");
        assert_eq!(summary.unwrap(), "Found one bug");
    }

    #[test]
    fn dedup_removes_similar_titles() {
        let mut findings = vec![
            Finding {
                priority: FindingPriority::High,
                kind: FindingKind::Bug,
                file: None,
                line: None,
                title: "Missing error handling in auth module".into(),
                description: "Desc 1".into(),
                reviewer: "correctness".into(),
            },
            Finding {
                priority: FindingPriority::Medium,
                kind: FindingKind::Suggestion,
                file: None,
                line: None,
                title: "Missing error handling in auth module".into(),
                description: "Desc 2".into(),
                reviewer: "completeness".into(),
            },
        ];
        dedup_findings(&mut findings);
        assert_eq!(findings.len(), 1);
        // Keeps the first (higher priority)
        assert_eq!(findings[0].reviewer, "correctness");
    }

    #[test]
    fn titles_similar_detects_overlap() {
        assert!(titles_similar("missing error handling", "missing error handling"));
        assert!(titles_similar(
            "missing error handling in auth",
            "missing error handling in auth module"
        ));
        assert!(!titles_similar("sql injection", "missing error handling"));
    }

    #[test]
    fn select_reviewers_by_size() {
        assert_eq!(ReviewerType::select_for_size(10).len(), 2); // Small
        assert_eq!(ReviewerType::select_for_size(100).len(), 3); // Medium
        assert_eq!(ReviewerType::select_for_size(500).len(), 4); // Large
    }
}
