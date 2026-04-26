//! Automatic post-edit verification.
//!
//! Detects a lightweight repo-appropriate verification command and runs it
//! after successful write-like tool operations.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const VERIFICATION_TIMEOUT: Duration = Duration::from_secs(180);
const OUTPUT_LIMIT: usize = 8_000;

/// A concrete verification command to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationCommand {
    program: String,
    args: Vec<String>,
}

impl VerificationCommand {
    fn new(program: impl Into<String>, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self { program: program.into(), args: args.into_iter().map(Into::into).collect() }
    }

    /// Human-readable command line.
    #[must_use]
    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// A preferred verification target carried across an automatic self-fix retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationPreference {
    command: VerificationCommand,
    scope_files: Vec<String>,
}

/// Policy used when verification gates a runtime action.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationPolicy {
    pub level: VerificationLevel,
    pub require_for_completion: bool,
    pub max_repair_attempts: u32,
}

impl Default for VerificationPolicy {
    fn default() -> Self {
        Self {
            level: VerificationLevel::Targeted,
            require_for_completion: true,
            max_repair_attempts: 1,
        }
    }
}

/// Verification strictness level.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationLevel {
    Sanity,
    Targeted,
    RepoDefault,
    Full,
}

/// Why verification stopped for a step or action.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStopReason {
    Passed,
    Failed,
    SkippedNoCommand,
    SkippedNoChanges,
    RepairBudgetExhausted,
}

/// Whether a failure is primarily style/lint or correctness breaking.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationFailureImpact {
    Style,
    Correctness,
    Unknown,
}

/// Durable verification history attached to plan run steps.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationRecord {
    pub level: VerificationLevel,
    pub command: Option<String>,
    pub scope_files: Vec<String>,
    pub success: Option<bool>,
    pub exit_code: Option<i32>,
    pub duration_ms: u128,
    pub stop_reason: VerificationStopReason,
    pub failure_kind: Option<VerificationFailureKind>,
    pub failure_impact: Option<VerificationFailureImpact>,
    pub summary: String,
    pub recorded_at: DateTime<Utc>,
}

impl VerificationRecord {
    /// Create a durable record from a completed command report.
    #[must_use]
    pub fn from_report(
        level: VerificationLevel,
        scope_files: Vec<String>,
        duration: Duration,
        report: &VerificationReport,
    ) -> Self {
        let failure_summary = report.failure_summary();
        let failure_kind = failure_summary.as_ref().map(|summary| summary.kind.clone());
        let failure_impact = failure_kind.as_ref().map(VerificationFailureImpact::from_kind);
        Self {
            level,
            command: Some(report.command.clone()),
            scope_files,
            success: Some(report.success),
            exit_code: report.exit_code,
            duration_ms: duration.as_millis(),
            stop_reason: if report.success {
                VerificationStopReason::Passed
            } else {
                VerificationStopReason::Failed
            },
            failure_kind,
            failure_impact,
            summary: report.summary(),
            recorded_at: Utc::now(),
        }
    }

    /// Create a durable skipped-verification record.
    #[must_use]
    pub fn skipped(
        level: VerificationLevel,
        scope_files: Vec<String>,
        stop_reason: VerificationStopReason,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            level,
            command: None,
            scope_files,
            success: None,
            exit_code: None,
            duration_ms: 0,
            stop_reason,
            failure_kind: None,
            failure_impact: None,
            summary: summary.into(),
            recorded_at: Utc::now(),
        }
    }
}

/// Whether retry edits can be mapped to the failing verification scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryChangeRelevance {
    Relevant,
    Irrelevant,
    Unknown,
}

impl VerificationPreference {
    fn new(command: VerificationCommand, scope_files: Vec<String>) -> Self {
        Self { command, scope_files }
    }

    /// Whether the latest retry edits are relevant to the failing verification scope.
    #[must_use]
    pub fn retry_change_relevance(
        &self,
        project_root: &Path,
        changed_files: &[String],
    ) -> RetryChangeRelevance {
        let changed_paths = normalize_changed_files(project_root, changed_files);
        if changed_paths.is_empty() {
            return RetryChangeRelevance::Unknown;
        }

        let scope_paths = normalize_changed_files(project_root, &self.scope_files);
        if !scope_paths.is_empty()
            && changed_paths.iter().any(|changed| {
                scope_paths.iter().any(|scope| {
                    changed == scope || changed.starts_with(scope) || scope.starts_with(changed)
                })
            })
        {
            return RetryChangeRelevance::Relevant;
        }

        if command_scope_matches_changed_paths(&self.command, &changed_paths) {
            RetryChangeRelevance::Relevant
        } else {
            RetryChangeRelevance::Irrelevant
        }
    }

    /// Human-readable summary of the failing verification scope.
    #[must_use]
    pub fn scope_summary(&self) -> String {
        match self.scope_files.as_slice() {
            [] => "unknown".to_string(),
            [single] => single.clone(),
            [first, second] => format!("{first}, {second}"),
            [first, second, rest @ ..] => {
                format!("{first}, {second}, and {} more", rest.len())
            }
        }
    }
}

/// Coarse failure class for verification retries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationFailureKind {
    CommandStart,
    Timeout,
    Build,
    Test,
    Lint,
    Unknown,
}

impl VerificationFailureImpact {
    fn from_kind(kind: &VerificationFailureKind) -> Self {
        match kind {
            VerificationFailureKind::Lint => Self::Style,
            VerificationFailureKind::Build | VerificationFailureKind::Test => Self::Correctness,
            VerificationFailureKind::CommandStart
            | VerificationFailureKind::Timeout
            | VerificationFailureKind::Unknown => Self::Unknown,
        }
    }
}

impl VerificationFailureKind {
    #[must_use]
    pub fn description(&self) -> &'static str {
        match self {
            Self::CommandStart => "verification command startup failure",
            Self::Timeout => "verification timeout",
            Self::Build => "build or typecheck failure",
            Self::Test => "test failure",
            Self::Lint => "lint failure",
            Self::Unknown => "unknown verification failure",
        }
    }
}

/// A normalized summary of a failed verification run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationFailureSummary {
    pub kind: VerificationFailureKind,
    pub headline: Option<String>,
    fingerprint: String,
}

impl VerificationFailureSummary {
    pub(crate) fn new(kind: VerificationFailureKind, headline: Option<String>) -> Self {
        let fingerprint =
            headline.as_deref().map(normalize_failure_fingerprint).unwrap_or_default();
        Self { kind, headline, fingerprint }
    }

    /// Whether two failures belong to the same retry family.
    #[must_use]
    pub fn same_family_as(&self, other: &Self) -> bool {
        self.kind == other.kind && self.fingerprint == other.fingerprint
    }
}

/// Result of an automatic verification run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationReport {
    pub(crate) executed_command: VerificationCommand,
    pub command: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub output: String,
}

impl VerificationReport {
    /// Summarize the verification outcome for UI and error reporting.
    #[must_use]
    pub fn summary(&self) -> String {
        use std::fmt::Write as _;

        let mut text = String::new();
        let status = if self.success { "passed" } else { "failed" };
        let _ = writeln!(text, "Automatic verification {status}.");
        let _ = writeln!(text, "Command: {}", self.command);
        if let Some(code) = self.exit_code {
            let _ = writeln!(text, "Exit code: {code}");
        }
        if !self.output.trim().is_empty() {
            let _ = writeln!(text, "\n{}", self.output.trim_end());
        }
        text.trim_end().to_string()
    }

    /// Build a retry preference from the failed verification run.
    #[must_use]
    pub fn retry_preference(&self, scope_files: &[String]) -> VerificationPreference {
        VerificationPreference::new(self.executed_command.clone(), scope_files.to_vec())
    }

    /// Classify a failed verification run for retry guidance.
    #[must_use]
    pub fn failure_summary(&self) -> Option<VerificationFailureSummary> {
        if self.success {
            return None;
        }

        let normalized_command = self.command.to_ascii_lowercase();
        let normalized_output = self.output.to_ascii_lowercase();
        let headline = select_failure_headline(&self.output);

        let kind = if normalized_output.starts_with("failed to start verification command:") {
            VerificationFailureKind::CommandStart
        } else if normalized_output.starts_with("verification timed out after") {
            VerificationFailureKind::Timeout
        } else if normalized_command.contains("clippy")
            || normalized_output.contains("clippy::")
            || normalized_output.contains("lint ")
        {
            VerificationFailureKind::Lint
        } else if normalized_command.contains(" test")
            || normalized_output.contains("test failed")
            || normalized_output.contains("failures:")
            || normalized_output.contains("failing")
            || normalized_output.contains("assertion")
            || normalized_output.contains("panic")
        {
            VerificationFailureKind::Test
        } else if normalized_command.contains(" check")
            || normalized_output.contains("could not compile")
            || normalized_output.contains("error[")
            || normalized_output.contains("expected expression")
            || normalized_output.contains("mismatched types")
        {
            VerificationFailureKind::Build
        } else {
            VerificationFailureKind::Unknown
        };

        Some(VerificationFailureSummary::new(kind, headline))
    }
}

/// Detect a repo-appropriate verification command.
#[must_use]
pub fn detect_command(
    project_root: &Path,
    changed_files: &[String],
) -> Option<VerificationCommand> {
    detect_command_with_preference(project_root, changed_files, None)
}

/// Detect a repo-appropriate verification command, optionally preferring a
/// previously failing scoped command during an automatic self-fix retry.
#[must_use]
pub fn detect_command_with_preference(
    project_root: &Path,
    changed_files: &[String],
    preference: Option<&VerificationPreference>,
) -> Option<VerificationCommand> {
    if let Some(command) = preferred_command(project_root, changed_files, preference) {
        return Some(command);
    }

    if let Some(command) = detect_rust_command(project_root, changed_files) {
        return Some(command);
    }

    if let Some(command) = detect_node_command(project_root, changed_files) {
        return Some(command);
    }

    if let Some(command) = detect_python_command(project_root, changed_files) {
        return Some(command);
    }

    if let Some(command) = detect_go_command(project_root, changed_files) {
        return Some(command);
    }

    None
}

fn preferred_command(
    project_root: &Path,
    changed_files: &[String],
    preference: Option<&VerificationPreference>,
) -> Option<VerificationCommand> {
    let preference = preference?;
    let effective_scope =
        if changed_files.is_empty() { preference.scope_files.as_slice() } else { changed_files };

    if effective_scope.is_empty()
        || command_matches_changed_files(project_root, effective_scope, &preference.command)
    {
        return Some(preference.command.clone());
    }

    None
}

/// Run a verification command inside the project root.
pub async fn run_command(
    project_root: &Path,
    command: &VerificationCommand,
) -> anyhow::Result<VerificationReport> {
    let mut process = tokio::process::Command::new(&command.program);
    process
        .args(&command.args)
        .current_dir(project_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let output = match tokio::time::timeout(VERIFICATION_TIMEOUT, process.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            return Ok(VerificationReport {
                executed_command: command.clone(),
                command: command.display(),
                success: false,
                exit_code: None,
                output: format!("failed to start verification command: {error}"),
            });
        }
        Err(_) => {
            return Ok(VerificationReport {
                executed_command: command.clone(),
                command: command.display(),
                success: false,
                exit_code: None,
                output: format!("verification timed out after {}s", VERIFICATION_TIMEOUT.as_secs()),
            });
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = combine_output(&stdout, &stderr);

    Ok(VerificationReport {
        executed_command: command.clone(),
        command: command.display(),
        success: output.status.success(),
        exit_code: output.status.code(),
        output: truncate_output(&combined),
    })
}

fn detect_node_command(
    project_root: &Path,
    changed_files: &[String],
) -> Option<VerificationCommand> {
    if !should_verify_node(project_root, changed_files) {
        return None;
    }

    let package_json = project_root.join("package.json");
    let content = std::fs::read_to_string(package_json).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
    let test_script = parsed.get("scripts")?.get("test")?.as_str()?;
    let package_manager = detect_node_package_manager(project_root);

    let changed_paths = normalize_changed_files(project_root, changed_files);
    let targeted_tests: Vec<String> = changed_paths
        .iter()
        .filter(|path| is_node_test_path(path))
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect();

    let command = if !targeted_tests.is_empty() && node_test_runner_supports_targeting(test_script)
    {
        node_targeted_test_command(package_manager, targeted_tests)
    } else {
        node_full_test_command(package_manager)
    };

    Some(command)
}

fn combine_output(stdout: &str, stderr: &str) -> String {
    match (stdout.trim(), stderr.trim()) {
        ("", "") => String::new(),
        ("", stderr) => format!("STDERR:\n{stderr}"),
        (stdout, "") => stdout.to_string(),
        (stdout, stderr) => format!("{stdout}\n\nSTDERR:\n{stderr}"),
    }
}

fn select_failure_headline(output: &str) -> Option<String> {
    let mut fallback = None;
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "STDERR:" {
            continue;
        }

        let normalized = normalize_failure_line(trimmed);
        if fallback.is_none() {
            fallback = Some(normalized.clone());
        }

        let lower = normalized.to_ascii_lowercase();
        if ["error", "failed", "failure", "fail", "panic", "assert", "mismatch"]
            .iter()
            .any(|pattern| lower.contains(pattern))
        {
            return Some(normalized);
        }
    }

    fallback
}

fn normalize_failure_line(line: &str) -> String {
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= 160 {
        collapsed
    } else {
        let mut truncated: String = collapsed.chars().take(157).collect();
        truncated.push_str("...");
        truncated
    }
}

fn normalize_failure_fingerprint(line: &str) -> String {
    line.to_ascii_lowercase()
        .chars()
        .map(|character| if character.is_ascii_digit() { '#' } else { character })
        .collect::<String>()
}

fn truncate_output(output: &str) -> String {
    let total_chars = output.chars().count();
    if total_chars <= OUTPUT_LIMIT {
        return output.to_string();
    }

    let head_len = OUTPUT_LIMIT / 2;
    let tail_len = OUTPUT_LIMIT / 2;
    let head: String = output.chars().take(head_len).collect();
    let tail: String = output.chars().skip(total_chars - tail_len).collect();
    format!("{head}\n\n...[verification output truncated]...\n\n{tail}")
}

fn should_verify_rust(project_root: &Path, changed_files: &[String]) -> bool {
    project_root.join("Cargo.toml").exists()
        && matches_language(changed_files, &["rs"], &["Cargo.toml"])
}

fn detect_rust_command(
    project_root: &Path,
    changed_files: &[String],
) -> Option<VerificationCommand> {
    if !should_verify_rust(project_root, changed_files) {
        return None;
    }

    let root_manifest = project_root.join("Cargo.toml");
    if changed_files.is_empty()
        || is_virtual_workspace_manifest(&root_manifest)
            && touches_root_manifest(project_root, changed_files)
    {
        return Some(VerificationCommand::new("cargo", ["check", "--workspace"]));
    }

    let changed_paths = normalize_changed_files(project_root, changed_files);
    let manifests = changed_paths
        .iter()
        .filter(|path| is_rust_related_path(path))
        .map(|path| rust_manifest_for_path(project_root, path))
        .collect::<Option<BTreeSet<_>>>()?;

    if manifests.len() != 1 {
        return Some(VerificationCommand::new("cargo", ["check", "--workspace"]));
    }

    let manifest = manifests.into_iter().next()?;
    if manifest == root_manifest && is_virtual_workspace_manifest(&manifest) {
        return Some(VerificationCommand::new("cargo", ["check", "--workspace"]));
    }

    let relative_manifest = relative_display_path(project_root, &manifest);
    Some(VerificationCommand::new(
        "cargo",
        ["check".to_string(), "--manifest-path".to_string(), relative_manifest],
    ))
}

fn should_verify_node(project_root: &Path, changed_files: &[String]) -> bool {
    project_root.join("package.json").exists()
        && matches_language(
            changed_files,
            &["js", "jsx", "ts", "tsx", "mjs", "cjs"],
            &["package.json"],
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodePackageManager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

fn detect_node_package_manager(project_root: &Path) -> NodePackageManager {
    if project_root.join("pnpm-lock.yaml").exists() {
        NodePackageManager::Pnpm
    } else if project_root.join("yarn.lock").exists() {
        NodePackageManager::Yarn
    } else if project_root.join("bun.lock").exists() || project_root.join("bun.lockb").exists() {
        NodePackageManager::Bun
    } else {
        NodePackageManager::Npm
    }
}

fn node_test_runner_supports_targeting(test_script: &str) -> bool {
    let script = test_script.to_ascii_lowercase();
    ["vitest", "jest", "mocha", "ava"].iter().any(|runner| script.contains(runner))
}

fn node_full_test_command(package_manager: NodePackageManager) -> VerificationCommand {
    match package_manager {
        NodePackageManager::Npm => VerificationCommand::new("npm", ["test"]),
        NodePackageManager::Pnpm => VerificationCommand::new("pnpm", ["test"]),
        NodePackageManager::Yarn => VerificationCommand::new("yarn", ["test"]),
        NodePackageManager::Bun => VerificationCommand::new("bun", ["test"]),
    }
}

fn node_targeted_test_command(
    package_manager: NodePackageManager,
    targeted_tests: Vec<String>,
) -> VerificationCommand {
    match package_manager {
        NodePackageManager::Npm => VerificationCommand::new(
            "npm",
            std::iter::once("test".to_string())
                .chain(std::iter::once("--".to_string()))
                .chain(targeted_tests)
                .collect::<Vec<_>>(),
        ),
        NodePackageManager::Pnpm => VerificationCommand::new(
            "pnpm",
            std::iter::once("test".to_string())
                .chain(std::iter::once("--".to_string()))
                .chain(targeted_tests)
                .collect::<Vec<_>>(),
        ),
        NodePackageManager::Yarn => VerificationCommand::new(
            "yarn",
            std::iter::once("test".to_string()).chain(targeted_tests).collect::<Vec<_>>(),
        ),
        NodePackageManager::Bun => VerificationCommand::new(
            "bun",
            std::iter::once("run".to_string())
                .chain(std::iter::once("test".to_string()))
                .chain(std::iter::once("--".to_string()))
                .chain(targeted_tests)
                .collect::<Vec<_>>(),
        ),
    }
}

fn should_verify_python(project_root: &Path, changed_files: &[String]) -> bool {
    (project_root.join("pyproject.toml").exists()
        || project_root.join("pytest.ini").exists()
        || project_root.join("setup.py").exists())
        && matches_language(changed_files, &["py"], &["pyproject.toml", "pytest.ini", "setup.py"])
}

fn detect_python_command(
    project_root: &Path,
    changed_files: &[String],
) -> Option<VerificationCommand> {
    if !should_verify_python(project_root, changed_files) {
        return None;
    }

    let changed_paths = normalize_changed_files(project_root, changed_files);
    let targeted_tests: Vec<String> = changed_paths
        .iter()
        .filter(|path| is_python_test_path(path))
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect();

    if targeted_tests.is_empty() {
        return Some(VerificationCommand::new("python3", ["-m", "pytest"]));
    }

    let args = std::iter::once("-m".to_string())
        .chain(std::iter::once("pytest".to_string()))
        .chain(targeted_tests)
        .collect::<Vec<_>>();
    Some(VerificationCommand::new("python3", args))
}

fn should_verify_go(project_root: &Path, changed_files: &[String]) -> bool {
    project_root.join("go.mod").exists() && matches_language(changed_files, &["go"], &["go.mod"])
}

fn detect_go_command(project_root: &Path, changed_files: &[String]) -> Option<VerificationCommand> {
    if !should_verify_go(project_root, changed_files) {
        return None;
    }

    if changed_files.is_empty() || touches_file(project_root, changed_files, "go.mod") {
        return Some(VerificationCommand::new("go", ["test", "./..."]));
    }

    let packages: BTreeSet<String> = normalize_changed_files(project_root, changed_files)
        .into_iter()
        .filter(|path| is_go_related_path(path))
        .map(|path| go_package_pattern_for_path(&path))
        .collect();

    if packages.is_empty() {
        return Some(VerificationCommand::new("go", ["test", "./..."]));
    }

    let args = std::iter::once("test".to_string()).chain(packages).collect::<Vec<_>>();
    Some(VerificationCommand::new("go", args))
}

fn command_matches_changed_files(
    project_root: &Path,
    changed_files: &[String],
    command: &VerificationCommand,
) -> bool {
    match command.program.as_str() {
        "cargo" => should_verify_rust(project_root, changed_files),
        "go" => should_verify_go(project_root, changed_files),
        "npm" | "pnpm" | "yarn" | "bun" => should_verify_node(project_root, changed_files),
        program if program.starts_with("python") => {
            should_verify_python(project_root, changed_files)
        }
        _ => false,
    }
}

fn command_scope_matches_changed_paths(
    command: &VerificationCommand,
    changed_paths: &[PathBuf],
) -> bool {
    match command.program.as_str() {
        "cargo" => cargo_command_matches_changed_paths(command, changed_paths),
        "go" => go_command_matches_changed_paths(command, changed_paths),
        "npm" | "pnpm" | "yarn" | "bun" => {
            node_command_matches_changed_paths(command, changed_paths)
        }
        program if program.starts_with("python") => {
            python_command_matches_changed_paths(command, changed_paths)
        }
        _ => false,
    }
}

fn cargo_command_matches_changed_paths(
    command: &VerificationCommand,
    changed_paths: &[PathBuf],
) -> bool {
    if changed_paths.is_empty() {
        return false;
    }

    if command.args.iter().any(|arg| arg == "--workspace") {
        return true;
    }

    let Some(manifest_index) = command.args.iter().position(|arg| arg == "--manifest-path") else {
        return true;
    };
    let Some(manifest) = command.args.get(manifest_index + 1) else {
        return true;
    };
    let manifest_dir = Path::new(manifest).parent().unwrap_or_else(|| Path::new(""));
    changed_paths.iter().any(|path| path.starts_with(manifest_dir))
}

fn go_command_matches_changed_paths(
    command: &VerificationCommand,
    changed_paths: &[PathBuf],
) -> bool {
    if changed_paths.is_empty() {
        return false;
    }

    let patterns = command.args.iter().skip(1);
    for pattern in patterns {
        if pattern == "./..." {
            return true;
        }

        if pattern == "." {
            if changed_paths.iter().any(|path| match path.parent() {
                Some(parent) => parent.as_os_str().is_empty(),
                None => true,
            }) {
                return true;
            }
            continue;
        }

        if let Some(relative) = pattern.strip_prefix("./") {
            let directory = Path::new(relative);
            if changed_paths.iter().any(|path| path.starts_with(directory)) {
                return true;
            }
        }
    }

    false
}

fn python_command_matches_changed_paths(
    command: &VerificationCommand,
    changed_paths: &[PathBuf],
) -> bool {
    if changed_paths.is_empty() {
        return false;
    }

    let Some(pytest_index) = command.args.iter().position(|arg| arg == "pytest") else {
        return true;
    };
    let targeted = command.args.iter().skip(pytest_index + 1).collect::<Vec<_>>();
    if targeted.is_empty() {
        return true;
    }

    changed_paths.iter().any(|path| targeted.iter().any(|target| path == Path::new(target)))
}

fn node_command_matches_changed_paths(
    command: &VerificationCommand,
    changed_paths: &[PathBuf],
) -> bool {
    if changed_paths.is_empty() {
        return false;
    }

    let targeted = match command.program.as_str() {
        "npm" | "pnpm" => command
            .args
            .iter()
            .position(|arg| arg == "--")
            .map(|index| command.args.iter().skip(index + 1).collect::<Vec<_>>())
            .unwrap_or_default(),
        "yarn" => command.args.iter().skip(1).collect::<Vec<_>>(),
        "bun" => command
            .args
            .iter()
            .position(|arg| arg == "--")
            .map(|index| command.args.iter().skip(index + 1).collect::<Vec<_>>())
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    if targeted.is_empty() {
        return true;
    }

    changed_paths.iter().any(|path| targeted.iter().any(|target| path == Path::new(target)))
}

fn matches_language(changed_files: &[String], extensions: &[&str], marker_names: &[&str]) -> bool {
    if changed_files.is_empty() {
        return true;
    }

    changed_files.iter().any(|file| {
        let path = Path::new(file);
        path.extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|ext| extensions.contains(&ext))
            || path
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|name| marker_names.contains(&name))
    })
}

fn normalize_changed_files(project_root: &Path, changed_files: &[String]) -> Vec<PathBuf> {
    changed_files
        .iter()
        .map(|file| {
            let path = Path::new(file);
            if path.is_absolute() {
                path.strip_prefix(project_root)
                    .map_or_else(|_| path.to_path_buf(), Path::to_path_buf)
            } else {
                path.to_path_buf()
            }
        })
        .collect()
}

fn touches_root_manifest(project_root: &Path, changed_files: &[String]) -> bool {
    touches_file(project_root, changed_files, "Cargo.toml")
}

fn touches_file(project_root: &Path, changed_files: &[String], file_name: &str) -> bool {
    normalize_changed_files(project_root, changed_files)
        .iter()
        .any(|path| path == Path::new(file_name))
}

fn is_virtual_workspace_manifest(manifest_path: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(manifest_path) else {
        return false;
    };
    let parsed: toml::Value = match toml::from_str(&content) {
        Ok(parsed) => parsed,
        Err(_) => return false,
    };

    parsed.get("workspace").is_some() && parsed.get("package").is_none()
}

fn rust_manifest_for_path(project_root: &Path, changed_path: &Path) -> Option<PathBuf> {
    let anchor = if changed_path.file_name().is_some_and(|name| name == "Cargo.toml") {
        changed_path.parent().unwrap_or_else(|| Path::new(""))
    } else {
        changed_path.parent().unwrap_or_else(|| Path::new(""))
    };

    let mut current = Some(anchor);
    while let Some(relative_dir) = current {
        let manifest = project_root.join(relative_dir).join("Cargo.toml");
        if manifest.exists() {
            return Some(manifest);
        }
        current = relative_dir.parent();
    }

    None
}

fn is_rust_related_path(path: &Path) -> bool {
    path.extension().and_then(std::ffi::OsStr::to_str) == Some("rs")
        || path.file_name().and_then(std::ffi::OsStr::to_str) == Some("Cargo.toml")
}

fn is_python_test_path(path: &Path) -> bool {
    if path.extension().and_then(std::ffi::OsStr::to_str) != Some("py") {
        return false;
    }

    path.components().any(|component| component.as_os_str() == "tests")
        || path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|name| name.starts_with("test_") || name.ends_with("_test.py"))
}

fn is_node_test_path(path: &Path) -> bool {
    let is_test_extension = matches!(
        path.extension().and_then(std::ffi::OsStr::to_str),
        Some("js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs")
    );
    if !is_test_extension {
        return false;
    }

    path.components()
        .any(|component| matches!(component.as_os_str().to_str(), Some("tests" | "__tests__")))
        || path.file_name().and_then(std::ffi::OsStr::to_str).is_some_and(|name| {
            name.contains(".test.")
                || name.contains(".spec.")
                || name.starts_with("test.")
                || name.starts_with("test_")
        })
}

fn is_go_related_path(path: &Path) -> bool {
    path.extension().and_then(std::ffi::OsStr::to_str) == Some("go")
        || path.file_name().and_then(std::ffi::OsStr::to_str) == Some("go.mod")
}

fn go_package_pattern_for_path(path: &Path) -> String {
    if path.file_name().and_then(std::ffi::OsStr::to_str) == Some("go.mod") {
        return "./...".to_string();
    }

    let directory = path.parent().unwrap_or_else(|| Path::new(""));
    if directory.as_os_str().is_empty() {
        return ".".to_string();
    }

    format!("./{}", directory.to_string_lossy().replace('\\', "/"))
}

fn relative_display_path(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root).map_or_else(
        |_| path.to_string_lossy().into_owned(),
        |relative| relative.to_string_lossy().replace('\\', "/"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_rust_command_for_rust_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\nmembers = [\"crates/app\"]\n")
            .expect("cargo toml");
        std::fs::create_dir_all(dir.path().join("crates/app/src")).expect("crate dir");
        std::fs::write(
            dir.path().join("crates/app/Cargo.toml"),
            "[package]\nname='app'\nversion='0.1.0'\nedition='2021'\n",
        )
        .expect("crate cargo toml");

        let command = detect_command(
            dir.path(),
            &[dir.path().join("crates/app/src/lib.rs").display().to_string()],
        )
        .expect("rust command");
        assert_eq!(command.display(), "cargo check --manifest-path crates/app/Cargo.toml");
    }

    #[test]
    fn detect_rust_command_falls_back_to_workspace_for_multiple_packages() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/app\", \"crates/core\"]\n",
        )
        .expect("workspace cargo toml");
        std::fs::create_dir_all(dir.path().join("crates/app/src")).expect("app dir");
        std::fs::create_dir_all(dir.path().join("crates/core/src")).expect("core dir");
        std::fs::write(
            dir.path().join("crates/app/Cargo.toml"),
            "[package]\nname='app'\nversion='0.1.0'\nedition='2021'\n",
        )
        .expect("app manifest");
        std::fs::write(
            dir.path().join("crates/core/Cargo.toml"),
            "[package]\nname='core'\nversion='0.1.0'\nedition='2021'\n",
        )
        .expect("core manifest");

        let command = detect_command(
            dir.path(),
            &[
                dir.path().join("crates/app/src/lib.rs").display().to_string(),
                dir.path().join("crates/core/src/lib.rs").display().to_string(),
            ],
        )
        .expect("workspace rust command");
        assert_eq!(command.display(), "cargo check --workspace");
    }

    #[test]
    fn detect_rust_command_falls_back_to_workspace_for_root_manifest_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\nmembers = [\"crates/app\"]\n")
            .expect("workspace cargo toml");

        let command =
            detect_command(dir.path(), &[dir.path().join("Cargo.toml").display().to_string()])
                .expect("workspace rust command");
        assert_eq!(command.display(), "cargo check --workspace");
    }

    #[test]
    fn detect_command_with_preference_reuses_previous_rust_scope() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\nmembers = [\"crates/app\"]\n")
            .expect("workspace cargo toml");
        std::fs::create_dir_all(dir.path().join("crates/app/src")).expect("crate dir");
        std::fs::write(
            dir.path().join("crates/app/Cargo.toml"),
            "[package]\nname='app'\nversion='0.1.0'\nedition='2021'\n",
        )
        .expect("crate cargo toml");

        let preference = VerificationPreference::new(
            VerificationCommand::new(
                "cargo",
                ["check", "--manifest-path", "crates/app/Cargo.toml"],
            ),
            vec![dir.path().join("crates/app/src/lib.rs").display().to_string()],
        );

        let command = detect_command_with_preference(
            dir.path(),
            &[dir.path().join("Cargo.toml").display().to_string()],
            Some(&preference),
        )
        .expect("preferred rust command");
        assert_eq!(command.display(), "cargo check --manifest-path crates/app/Cargo.toml");
    }

    #[test]
    fn detect_node_command_prefers_package_manager_lockfile() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"name":"fixture","scripts":{"test":"vitest"}}"#,
        )
        .expect("package json");
        std::fs::write(dir.path().join("pnpm-lock.yaml"), "lockfileVersion: '9.0'")
            .expect("lockfile");

        let command =
            detect_command(dir.path(), &[dir.path().join("src/index.ts").display().to_string()])
                .expect("node command");
        assert_eq!(command.display(), "pnpm test");
    }

    #[test]
    fn detect_node_command_targets_supported_runner_test_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"name":"fixture","scripts":{"test":"vitest run"}}"#,
        )
        .expect("package json");

        let command =
            detect_command(dir.path(), &[dir.path().join("src/app.test.ts").display().to_string()])
                .expect("node command");
        assert_eq!(command.display(), "npm test -- src/app.test.ts");
    }

    #[test]
    fn detect_node_command_falls_back_for_unknown_runner() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"name":"fixture","scripts":{"test":"custom-runner execute-tests"}}"#,
        )
        .expect("package json");

        let command =
            detect_command(dir.path(), &[dir.path().join("src/app.test.ts").display().to_string()])
                .expect("node command");
        assert_eq!(command.display(), "npm test");
    }

    #[test]
    fn detect_node_command_falls_back_for_non_test_file_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"name":"fixture","scripts":{"test":"jest --runInBand"}}"#,
        )
        .expect("package json");

        let command =
            detect_command(dir.path(), &[dir.path().join("src/app.ts").display().to_string()])
                .expect("node command");
        assert_eq!(command.display(), "npm test");
    }

    #[test]
    fn detect_command_with_preference_falls_back_when_scope_switches_languages() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n",
        )
        .expect("cargo toml");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"name":"fixture","scripts":{"test":"vitest run"}}"#,
        )
        .expect("package json");

        let preference = VerificationPreference::new(
            VerificationCommand::new("cargo", ["check", "--manifest-path", "Cargo.toml"]),
            vec![dir.path().join("src/lib.rs").display().to_string()],
        );

        let command = detect_command_with_preference(
            dir.path(),
            &[dir.path().join("src/app.test.ts").display().to_string()],
            Some(&preference),
        )
        .expect("node fallback command");
        assert_eq!(command.display(), "npm test -- src/app.test.ts");
    }

    #[test]
    fn detect_returns_none_for_docs_only_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n",
        )
        .expect("cargo toml");

        let command =
            detect_command(dir.path(), &[dir.path().join("README.md").display().to_string()]);
        assert!(command.is_none());
    }

    #[test]
    fn detect_python_command_targets_changed_test_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname='fixture'\nversion='0.1.0'\n",
        )
        .expect("pyproject");
        std::fs::create_dir_all(dir.path().join("tests")).expect("tests dir");

        let command = detect_command(
            dir.path(),
            &[dir.path().join("tests/test_api.py").display().to_string()],
        )
        .expect("python command");
        assert_eq!(command.display(), "python3 -m pytest tests/test_api.py");
    }

    #[test]
    fn detect_go_command_targets_changed_package() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("go.mod"), "module example.com/fixture\n\ngo 1.22\n")
            .expect("go mod");

        let command = detect_command(
            dir.path(),
            &[dir.path().join("internal/api/handler.go").display().to_string()],
        )
        .expect("go command");
        assert_eq!(command.display(), "go test ./internal/api");
    }

    #[test]
    fn detect_go_command_targets_multiple_packages() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("go.mod"), "module example.com/fixture\n\ngo 1.22\n")
            .expect("go mod");

        let command = detect_command(
            dir.path(),
            &[
                dir.path().join("internal/api/handler.go").display().to_string(),
                dir.path().join("pkg/client/client.go").display().to_string(),
            ],
        )
        .expect("go command");
        assert_eq!(command.display(), "go test ./internal/api ./pkg/client");
    }

    #[test]
    fn failure_summary_detects_build_failure() {
        let report = VerificationReport {
            executed_command: VerificationCommand::new(
                "cargo",
                ["check", "--manifest-path", "crates/app/Cargo.toml"],
            ),
            command: "cargo check --manifest-path crates/app/Cargo.toml".to_string(),
            success: false,
            exit_code: Some(101),
            output: "error[E0308]: mismatched types\n  --> src/lib.rs:12:5".to_string(),
        };

        let summary = report.failure_summary().expect("failure summary");
        assert_eq!(summary.kind, VerificationFailureKind::Build);
        assert_eq!(summary.headline.as_deref(), Some("error[E0308]: mismatched types"));
    }

    #[test]
    fn failure_summary_detects_test_failure() {
        let report = VerificationReport {
            executed_command: VerificationCommand::new("go", ["test", "./pkg/client"]),
            command: "go test ./pkg/client".to_string(),
            success: false,
            exit_code: Some(1),
            output: "--- FAIL: TestClient (0.00s)\nassertion failed: expected ok".to_string(),
        };

        let summary = report.failure_summary().expect("failure summary");
        assert_eq!(summary.kind, VerificationFailureKind::Test);
        assert_eq!(summary.headline.as_deref(), Some("--- FAIL: TestClient (0.00s)"));
    }

    #[test]
    fn failure_summary_same_family_ignores_line_numbers() {
        let first = VerificationFailureSummary::new(
            VerificationFailureKind::Build,
            Some("error[E0308]: mismatched types at src/lib.rs:12".to_string()),
        );
        let second = VerificationFailureSummary::new(
            VerificationFailureKind::Build,
            Some("error[E0308]: mismatched types at src/lib.rs:48".to_string()),
        );

        assert!(first.same_family_as(&second));
    }

    #[test]
    fn retry_change_relevance_treats_same_crate_manifest_as_relevant() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\nmembers = [\"crates/app\"]\n")
            .expect("workspace cargo toml");
        std::fs::create_dir_all(dir.path().join("crates/app/src")).expect("crate dir");
        std::fs::write(
            dir.path().join("crates/app/Cargo.toml"),
            "[package]\nname='app'\nversion='0.1.0'\nedition='2021'\n",
        )
        .expect("crate cargo toml");

        let command = detect_command(
            dir.path(),
            &[dir.path().join("crates/app/src/lib.rs").display().to_string()],
        )
        .expect("rust command");
        let preference = VerificationPreference::new(
            command,
            vec![dir.path().join("crates/app/src/lib.rs").display().to_string()],
        );

        assert_eq!(
            preference.retry_change_relevance(
                dir.path(),
                &[dir.path().join("crates/app/Cargo.toml").display().to_string()],
            ),
            RetryChangeRelevance::Relevant
        );
    }

    #[test]
    fn retry_change_relevance_is_unknown_without_changed_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let preference = VerificationPreference::new(
            VerificationCommand::new("cargo", ["check", "--workspace"]),
            vec!["src/lib.rs".to_string()],
        );

        assert_eq!(
            preference.retry_change_relevance(dir.path(), &[]),
            RetryChangeRelevance::Unknown
        );
    }
}
