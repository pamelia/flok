//! # Self-Review Loop
//!
//! Iterative PR improvement loop that alternates between automated code
//! review and applying fixes. Runs until the review is clean, max turns
//! are reached, or oscillation is detected.
//!
//! ## Flow
//!
//! 1. Get the PR diff (`gh pr diff` or `git diff`)
//! 2. Run `ReviewEngine::review()` on the diff
//! 3. If clean → stop
//! 4. Triage findings: decide which to address vs skip
//! 5. Apply fixes (via the session engine's LLM)
//! 6. Run tests (auto-detect test runner)
//! 7. Commit and push
//! 8. Go to step 1

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::provider::Provider;
use crate::review::engine::ReviewEngine;
use crate::review::types::{Finding, FindingKind, FindingPriority, ReviewResult, Verdict};

/// Maximum number of review-fix iterations.
const MAX_TURNS: usize = 5;

/// Why the self-review loop stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// Review passed — no actionable critical/high findings.
    CleanReview,
    /// Maximum number of turns reached.
    MaxTurns,
    /// Files are being changed back and forth between turns.
    Oscillation,
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CleanReview => write!(f, "clean_review"),
            Self::MaxTurns => write!(f, "max_turns"),
            Self::Oscillation => write!(f, "oscillation"),
        }
    }
}

/// Record of what happened in a single turn.
#[derive(Debug, Clone)]
pub struct TurnRecord {
    /// Turn number (1-indexed).
    pub turn: usize,
    /// The review result for this turn.
    pub review: ReviewResult,
    /// Findings that were addressed.
    pub addressed: Vec<Finding>,
    /// Findings that were skipped (with reasons).
    pub skipped: Vec<(Finding, String)>,
    /// Files changed during this turn's fixes.
    pub files_changed: HashSet<String>,
    /// Commit SHA (if a commit was made).
    pub commit_sha: Option<String>,
}

/// The complete result of a self-review loop run.
#[derive(Debug, Clone)]
pub struct SelfReviewResult {
    /// Why the loop stopped.
    pub stop_reason: StopReason,
    /// Records for each turn.
    pub turns: Vec<TurnRecord>,
    /// The final review's verdict.
    pub final_verdict: Verdict,
}

impl SelfReviewResult {
    /// Total number of findings addressed across all turns.
    pub fn total_addressed(&self) -> usize {
        self.turns.iter().map(|t| t.addressed.len()).sum()
    }

    /// Total number of findings skipped across all turns.
    pub fn total_skipped(&self) -> usize {
        self.turns.iter().map(|t| t.skipped.len()).sum()
    }

    /// Format as a human-readable summary.
    pub fn format_summary(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();

        let _ = writeln!(s, "## Self-Review Loop Complete\n");
        let _ = writeln!(s, "**Stop reason:** {}", self.stop_reason);
        let _ = writeln!(s, "**Turns:** {}", self.turns.len());
        let _ = writeln!(s, "**Final verdict:** {}", self.final_verdict);
        let _ = writeln!(s, "**Total addressed:** {}", self.total_addressed());
        let _ = writeln!(s, "**Total skipped:** {}\n", self.total_skipped());

        for turn in &self.turns {
            let _ = writeln!(s, "### Turn {}", turn.turn);
            let _ = writeln!(s, "- Verdict: {}", turn.review.verdict);
            let _ = writeln!(
                s,
                "- Findings: {} critical, {} high, {} medium, {} low",
                turn.review.count_by_priority(FindingPriority::Critical),
                turn.review.count_by_priority(FindingPriority::High),
                turn.review.count_by_priority(FindingPriority::Medium),
                turn.review.count_by_priority(FindingPriority::Low),
            );
            let _ = writeln!(s, "- Addressed: {}", turn.addressed.len());
            let _ = writeln!(s, "- Skipped: {}", turn.skipped.len());
            if let Some(sha) = &turn.commit_sha {
                let _ = writeln!(s, "- Commit: {}", &sha[..8.min(sha.len())]);
            }
            let _ = writeln!(s);
        }

        s
    }
}

/// The self-review loop engine.
pub struct SelfReviewLoop {
    review_engine: ReviewEngine,
    project_root: std::path::PathBuf,
}

impl SelfReviewLoop {
    /// Create a new self-review loop.
    pub fn new(provider: Arc<dyn Provider>, project_root: std::path::PathBuf) -> Self {
        Self { review_engine: ReviewEngine::new(provider), project_root }
    }

    /// Run the self-review loop on a PR or branch diff.
    ///
    /// # Arguments
    ///
    /// * `base_branch` — The base branch to diff against (e.g., "main").
    /// * `context` — Optional PR description or additional context.
    ///
    /// Returns a `SelfReviewResult` with all turn records and the stop reason.
    pub async fn run(
        &self,
        base_branch: &str,
        context: Option<&str>,
    ) -> anyhow::Result<SelfReviewResult> {
        let mut turns: Vec<TurnRecord> = Vec::new();
        let mut files_per_turn: HashMap<usize, HashSet<String>> = HashMap::new();
        let mut stop_reason = StopReason::MaxTurns;

        for turn_num in 1..=MAX_TURNS {
            tracing::info!(turn = turn_num, "self-review loop: starting turn");

            // Step 1: Get the current diff
            let diff = get_diff(&self.project_root, base_branch).await?;

            if diff.trim().is_empty() {
                tracing::info!("no changes to review");
                stop_reason = StopReason::CleanReview;
                break;
            }

            // Step 2: Run the review
            let review = self.review_engine.review(&diff, context).await?;

            // Step 3: Check stop conditions
            if review.is_clean() {
                tracing::info!(turn = turn_num, "review is clean — stopping");
                turns.push(TurnRecord {
                    turn: turn_num,
                    review,
                    addressed: Vec::new(),
                    skipped: Vec::new(),
                    files_changed: HashSet::new(),
                    commit_sha: None,
                });
                stop_reason = StopReason::CleanReview;
                break;
            }

            // Step 3b: Check oscillation (after turn 2)
            if turn_num > 2 {
                if let Some(two_turns_ago) = files_per_turn.get(&(turn_num - 2)) {
                    let current_files: HashSet<String> =
                        review.findings.iter().filter_map(|f| f.file.clone()).collect();

                    if !current_files.is_empty() && !two_turns_ago.is_empty() {
                        let overlap = current_files.intersection(two_turns_ago).count();
                        let overlap_pct = overlap as f64 / current_files.len().max(1) as f64;

                        if overlap_pct > 0.5 {
                            tracing::warn!(
                                turn = turn_num,
                                overlap_pct = format!("{:.0}%", overlap_pct * 100.0),
                                "oscillation detected — stopping"
                            );
                            turns.push(TurnRecord {
                                turn: turn_num,
                                review,
                                addressed: Vec::new(),
                                skipped: Vec::new(),
                                files_changed: HashSet::new(),
                                commit_sha: None,
                            });
                            stop_reason = StopReason::Oscillation;
                            break;
                        }
                    }
                }
            }

            // Step 4: Triage findings
            let (to_address, to_skip) = triage_findings(&review.findings);

            // Track files for oscillation detection
            let turn_files: HashSet<String> =
                to_address.iter().filter_map(|f| f.file.clone()).collect();
            files_per_turn.insert(turn_num, turn_files.clone());

            // Steps 5-7 would involve: applying fixes via LLM, running tests,
            // and committing. For now, we record the triage results.
            // The actual fix application will be handled by the session engine
            // that invokes this loop.

            turns.push(TurnRecord {
                turn: turn_num,
                review,
                addressed: to_address,
                skipped: to_skip,
                files_changed: turn_files,
                commit_sha: None,
            });
        }

        let final_verdict = turns.last().map_or(Verdict::Approve, |t| t.review.verdict);

        Ok(SelfReviewResult { stop_reason, turns, final_verdict })
    }
}

/// Triage findings into "address" and "skip" buckets.
///
/// Address: real bugs, correctness issues, high-priority suggestions.
/// Skip: nitpicks, subjective style, thoughts, questions, risks.
fn triage_findings(findings: &[Finding]) -> (Vec<Finding>, Vec<(Finding, String)>) {
    let mut to_address = Vec::new();
    let mut to_skip = Vec::new();

    for finding in findings {
        if should_address(finding) {
            to_address.push(finding.clone());
        } else {
            let reason = skip_reason(finding);
            to_skip.push((finding.clone(), reason));
        }
    }

    (to_address, to_skip)
}

/// Determine if a finding should be addressed.
fn should_address(finding: &Finding) -> bool {
    // Always address critical/high bugs
    if finding.priority <= FindingPriority::High && finding.kind == FindingKind::Bug {
        return true;
    }

    // Address critical/high suggestions
    if finding.priority <= FindingPriority::High && finding.kind == FindingKind::Suggestion {
        return true;
    }

    // Address medium bugs
    if finding.priority == FindingPriority::Medium && finding.kind == FindingKind::Bug {
        return true;
    }

    false
}

/// Explain why a finding is being skipped.
fn skip_reason(finding: &Finding) -> String {
    match finding.kind {
        FindingKind::Nitpick => "nitpick — non-blocking".into(),
        FindingKind::Thought => "thought/observation — not actionable".into(),
        FindingKind::Question => "question — requires human input".into(),
        FindingKind::Risk => "risk acknowledgement — not a code change".into(),
        FindingKind::Suggestion if finding.priority == FindingPriority::Low => {
            "low-priority suggestion — optional improvement".into()
        }
        FindingKind::Suggestion => "medium suggestion — deferred".into(),
        FindingKind::Bug if finding.priority == FindingPriority::Low => {
            "low-priority issue — minor".into()
        }
        FindingKind::Bug => "low-priority bug — deferred".into(),
    }
}

/// Get the diff between the current branch and the base branch.
async fn get_diff(project_root: &Path, base_branch: &str) -> anyhow::Result<String> {
    let output = tokio::process::Command::new("git")
        .args(["diff", base_branch])
        .current_dir(project_root)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("git diff failed: {stderr}"));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triage_addresses_critical_bugs() {
        let findings = vec![
            Finding {
                priority: FindingPriority::Critical,
                kind: FindingKind::Bug,
                file: Some("src/auth.rs".into()),
                line: Some("42".into()),
                title: "SQL injection".into(),
                description: "User input not sanitized".into(),
                reviewer: "correctness".into(),
            },
            Finding {
                priority: FindingPriority::Low,
                kind: FindingKind::Nitpick,
                file: None,
                line: None,
                title: "Variable naming".into(),
                description: "x is not descriptive".into(),
                reviewer: "style".into(),
            },
        ];

        let (address, skip) = triage_findings(&findings);
        assert_eq!(address.len(), 1);
        assert_eq!(address[0].title, "SQL injection");
        assert_eq!(skip.len(), 1);
        assert_eq!(skip[0].0.title, "Variable naming");
    }

    #[test]
    fn triage_skips_thoughts_and_questions() {
        let findings = vec![
            Finding {
                priority: FindingPriority::Medium,
                kind: FindingKind::Thought,
                file: None,
                line: None,
                title: "Architecture observation".into(),
                description: "This module might benefit from splitting".into(),
                reviewer: "architecture".into(),
            },
            Finding {
                priority: FindingPriority::Medium,
                kind: FindingKind::Question,
                file: None,
                line: None,
                title: "Clarification needed".into(),
                description: "Why was this approach chosen?".into(),
                reviewer: "completeness".into(),
            },
        ];

        let (address, skip) = triage_findings(&findings);
        assert_eq!(address.len(), 0);
        assert_eq!(skip.len(), 2);
    }

    #[test]
    fn stop_reason_display() {
        assert_eq!(StopReason::CleanReview.to_string(), "clean_review");
        assert_eq!(StopReason::MaxTurns.to_string(), "max_turns");
        assert_eq!(StopReason::Oscillation.to_string(), "oscillation");
    }

    #[test]
    fn self_review_result_summary() {
        let result = SelfReviewResult {
            stop_reason: StopReason::CleanReview,
            turns: vec![TurnRecord {
                turn: 1,
                review: ReviewResult {
                    verdict: Verdict::RequestChanges,
                    findings: vec![Finding {
                        priority: FindingPriority::High,
                        kind: FindingKind::Bug,
                        file: Some("src/lib.rs".into()),
                        line: None,
                        title: "Missing null check".into(),
                        description: "Could panic".into(),
                        reviewer: "correctness".into(),
                    }],
                    summary: "Found issue".into(),
                },
                addressed: vec![Finding {
                    priority: FindingPriority::High,
                    kind: FindingKind::Bug,
                    file: Some("src/lib.rs".into()),
                    line: None,
                    title: "Missing null check".into(),
                    description: "Could panic".into(),
                    reviewer: "correctness".into(),
                }],
                skipped: Vec::new(),
                files_changed: ["src/lib.rs".to_string()].into_iter().collect(),
                commit_sha: Some("abc12345".into()),
            }],
            final_verdict: Verdict::Approve,
        };

        assert_eq!(result.total_addressed(), 1);
        assert_eq!(result.total_skipped(), 0);

        let summary = result.format_summary();
        assert!(summary.contains("clean_review"));
        assert!(summary.contains("APPROVE"));
        assert!(summary.contains("Turn 1"));
    }
}
