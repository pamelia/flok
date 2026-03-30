//! Structured types for code review results.

/// Priority tier for a review finding.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum FindingPriority {
    /// Must fix — correctness bug, security issue, data loss risk.
    Critical,
    /// Should fix — logic error, missing error handling, performance issue.
    High,
    /// Consider fixing — code clarity, naming, minor improvements.
    Medium,
    /// Optional — style nits, subjective suggestions, thoughts.
    Low,
}

impl std::fmt::Display for FindingPriority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Critical => write!(f, "critical"),
            Self::High => write!(f, "high"),
            Self::Medium => write!(f, "medium"),
            Self::Low => write!(f, "low"),
        }
    }
}

/// The kind of finding — determines whether it's actionable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// A concrete bug or correctness issue that should be fixed.
    Bug,
    /// A suggestion for improvement (may or may not be actionable).
    Suggestion,
    /// A stylistic nitpick (non-blocking).
    Nitpick,
    /// A thought or observation (not directly actionable).
    Thought,
    /// A question for clarification.
    Question,
    /// A risk or concern to be aware of.
    Risk,
}

impl FindingKind {
    /// Whether this finding kind is actionable (should be addressed).
    pub fn is_actionable(self) -> bool {
        matches!(self, Self::Bug | Self::Suggestion)
    }
}

impl std::fmt::Display for FindingKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bug => write!(f, "bug"),
            Self::Suggestion => write!(f, "suggestion"),
            Self::Nitpick => write!(f, "nitpick"),
            Self::Thought => write!(f, "thought"),
            Self::Question => write!(f, "question"),
            Self::Risk => write!(f, "risk"),
        }
    }
}

/// A single finding from a code review.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Finding {
    /// Priority tier.
    pub priority: FindingPriority,
    /// Kind of finding.
    pub kind: FindingKind,
    /// Which file this finding relates to (if any).
    pub file: Option<String>,
    /// Line number or range (if applicable).
    pub line: Option<String>,
    /// Short title summarizing the finding.
    pub title: String,
    /// Detailed description of the issue and suggested fix.
    pub description: String,
    /// Which reviewer reported this finding.
    pub reviewer: String,
}

/// Binary verdict for a code review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    /// Code is acceptable — no critical/high blocking issues.
    Approve,
    /// Code needs changes — has critical or high-priority issues.
    RequestChanges,
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Approve => write!(f, "APPROVE"),
            Self::RequestChanges => write!(f, "REQUEST_CHANGES"),
        }
    }
}

/// The complete result of a code review.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReviewResult {
    /// Binary verdict.
    pub verdict: Verdict,
    /// All findings, sorted by priority (critical first).
    pub findings: Vec<Finding>,
    /// Summary text for the review.
    pub summary: String,
}

impl ReviewResult {
    /// Count findings by priority.
    pub fn count_by_priority(&self, priority: FindingPriority) -> usize {
        self.findings.iter().filter(|f| f.priority == priority).count()
    }

    /// Whether all remaining findings are non-actionable.
    pub fn all_non_actionable(&self) -> bool {
        self.findings.iter().all(|f| !f.kind.is_actionable())
    }

    /// Whether this review is "clean" — approve with no critical/high actionable findings.
    pub fn is_clean(&self) -> bool {
        self.verdict == Verdict::Approve
            && self.count_by_priority(FindingPriority::Critical) == 0
            && self.count_by_priority(FindingPriority::High) == 0
    }

    /// Format as a human-readable report.
    pub fn format_report(&self) -> String {
        use std::fmt::Write;
        let mut report = String::new();

        let _ = writeln!(report, "## Code Review: {}\n", self.verdict);
        let _ = writeln!(report, "{}\n", self.summary);

        let _ = writeln!(
            report,
            "**Findings:** {} critical, {} high, {} medium, {} low\n",
            self.count_by_priority(FindingPriority::Critical),
            self.count_by_priority(FindingPriority::High),
            self.count_by_priority(FindingPriority::Medium),
            self.count_by_priority(FindingPriority::Low),
        );

        for (i, finding) in self.findings.iter().enumerate() {
            let location = match (&finding.file, &finding.line) {
                (Some(f), Some(l)) => format!(" ({f}:{l})"),
                (Some(f), None) => format!(" ({f})"),
                _ => String::new(),
            };
            let _ = writeln!(
                report,
                "{}. **[{}/{}]** {}{}\n   {}",
                i + 1,
                finding.priority,
                finding.kind,
                finding.title,
                location,
                finding.description,
            );
        }

        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_review() {
        let result = ReviewResult {
            verdict: Verdict::Approve,
            findings: vec![Finding {
                priority: FindingPriority::Low,
                kind: FindingKind::Nitpick,
                file: Some("src/main.rs".into()),
                line: None,
                title: "Consider renaming variable".into(),
                description: "x is not descriptive".into(),
                reviewer: "style".into(),
            }],
            summary: "Code looks good".into(),
        };
        assert!(result.is_clean());
    }

    #[test]
    fn dirty_review() {
        let result = ReviewResult {
            verdict: Verdict::RequestChanges,
            findings: vec![Finding {
                priority: FindingPriority::Critical,
                kind: FindingKind::Bug,
                file: Some("src/auth.rs".into()),
                line: Some("42".into()),
                title: "SQL injection".into(),
                description: "User input is not sanitized".into(),
                reviewer: "correctness".into(),
            }],
            summary: "Critical security issue found".into(),
        };
        assert!(!result.is_clean());
        assert_eq!(result.count_by_priority(FindingPriority::Critical), 1);
    }

    #[test]
    fn format_report_includes_all_findings() {
        let result = ReviewResult {
            verdict: Verdict::RequestChanges,
            findings: vec![
                Finding {
                    priority: FindingPriority::High,
                    kind: FindingKind::Bug,
                    file: Some("src/lib.rs".into()),
                    line: Some("10".into()),
                    title: "Off by one error".into(),
                    description: "Loop iterates one too many times".into(),
                    reviewer: "correctness".into(),
                },
                Finding {
                    priority: FindingPriority::Low,
                    kind: FindingKind::Nitpick,
                    file: None,
                    line: None,
                    title: "Unused import".into(),
                    description: "Remove unused import".into(),
                    reviewer: "style".into(),
                },
            ],
            summary: "Found issues".into(),
        };
        let report = result.format_report();
        assert!(report.contains("REQUEST_CHANGES"));
        assert!(report.contains("Off by one error"));
        assert!(report.contains("Unused import"));
        assert!(report.contains("(src/lib.rs:10)"));
    }
}
