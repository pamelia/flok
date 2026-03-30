//! Layer 1: Shell output compression.
//!
//! A multi-stage pipeline that compresses raw command output before it enters
//! the agent's context window. Stages are composable pure functions.

use std::fmt::Write;

use regex::Regex;
use std::sync::LazyLock;

/// The result of compressing shell output.
#[derive(Debug, Clone)]
pub struct CompressedOutput {
    /// The compressed text.
    pub text: String,
    /// Number of characters in the original output.
    pub original_chars: usize,
    /// Number of characters after compression.
    pub compressed_chars: usize,
}

impl CompressedOutput {
    /// The compression ratio (0.0 = no compression, 1.0 = fully compressed).
    pub fn ratio(&self) -> f64 {
        if self.original_chars == 0 {
            return 0.0;
        }
        1.0 - (self.compressed_chars as f64 / self.original_chars as f64)
    }
}

/// Compress shell output through the full pipeline.
///
/// Stages:
/// 1. Filter (lossless): strip ANSI, git hints, progress, blank lines
/// 2. Command-specific filter (if a known command)
/// 3. Truncate (if output exceeds token budget)
/// 4. Deduplicate consecutive identical lines (with log normalization)
pub fn compress_shell_output(output: &str, command: &str, max_chars: usize) -> CompressedOutput {
    let original_chars = output.len();

    // Stage 1: Generic filter (lossless)
    let filtered = stage_filter(output);

    // Stage 2: Command-specific filter
    let specific = stage_command_specific(&filtered, command);

    // Stage 3: Truncate if too large
    let truncated = stage_truncate(&specific, max_chars);

    // Stage 4: Deduplicate
    let deduped = stage_dedup(&truncated);

    let compressed_chars = deduped.len();

    CompressedOutput { text: deduped, original_chars, compressed_chars }
}

/// Compress shell output with a precise token budget using tiktoken.
///
/// Uses `tiktoken-rs` to count tokens and truncate to fit within the budget.
/// Falls back to char-based truncation if the tokenizer isn't available.
pub fn compress_shell_output_token_budget(
    output: &str,
    command: &str,
    max_tokens: usize,
    model_id: &str,
) -> CompressedOutput {
    let original_chars = output.len();

    // Stages 1-2: filter and command-specific
    let filtered = stage_filter(output);
    let specific = stage_command_specific(&filtered, command);

    // Stage 3: Adaptive truncation using token counting
    let counter = crate::token::TokenCounter::for_model(model_id);
    let truncated = stage_truncate_tokens(&specific, max_tokens, &counter);

    // Stage 4: Deduplicate
    let deduped = stage_dedup(&truncated);

    let compressed_chars = deduped.len();

    CompressedOutput { text: deduped, original_chars, compressed_chars }
}

/// Truncate text to fit within a token budget.
fn stage_truncate_tokens(
    input: &str,
    max_tokens: usize,
    counter: &crate::token::TokenCounter,
) -> String {
    let current_tokens = counter.count(input);
    if current_tokens <= max_tokens {
        return input.to_string();
    }

    // Binary search for the right split point
    // Keep head (40%) and tail (40%) by tokens
    let head_budget = max_tokens * 2 / 5;
    let tail_budget = max_tokens * 2 / 5;

    let lines: Vec<&str> = input.lines().collect();
    let total_lines = lines.len();

    // Find head: accumulate lines from the start until we hit head_budget tokens
    let mut head_end = 0;
    let mut head_tokens = 0;
    for (i, line) in lines.iter().enumerate() {
        let line_tokens = counter.count(line) + 1; // +1 for newline
        if head_tokens + line_tokens > head_budget {
            break;
        }
        head_tokens += line_tokens;
        head_end = i + 1;
    }

    // Find tail: accumulate lines from the end until we hit tail_budget tokens
    let mut tail_start = total_lines;
    let mut tail_tokens = 0;
    for (i, line) in lines.iter().enumerate().rev() {
        let line_tokens = counter.count(line) + 1;
        if tail_tokens + line_tokens > tail_budget {
            break;
        }
        tail_tokens += line_tokens;
        tail_start = i;
    }

    // Ensure no overlap
    if tail_start <= head_end {
        tail_start = head_end;
    }

    let omitted = total_lines - head_end - (total_lines - tail_start);
    let omitted_tokens = current_tokens - head_tokens - tail_tokens;

    let mut result = String::new();
    for line in &lines[..head_end] {
        result.push_str(line);
        result.push('\n');
    }
    let _ = write!(result, "\n... ({omitted} lines, ~{omitted_tokens} tokens omitted) ...\n\n");
    for line in &lines[tail_start..] {
        result.push_str(line);
        result.push('\n');
    }

    result.truncate(result.trim_end().len());
    result
}

// ---------------------------------------------------------------------------
// Stage 1: Generic Filter (lossless)
// ---------------------------------------------------------------------------

/// Strip content with zero informational value.
fn stage_filter(input: &str) -> String {
    static ANSI_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").expect("valid regex"));
    static PROGRESS_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\s*\[?[=>#\-]{3,}").expect("valid regex"));
    static PERCENT_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\s*\d{1,3}%").expect("valid regex"));
    static SEPARATOR_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[-=*~]{3,}\s*$").expect("valid regex"));

    let stripped = ANSI_RE.replace_all(input, "");

    let mut result = String::with_capacity(stripped.len());
    let mut blank_count = 0u32;

    for line in stripped.lines() {
        let trimmed = line.trim();

        // Skip blank lines (collapse runs to one)
        if trimmed.is_empty() {
            blank_count += 1;
            if blank_count <= 1 {
                result.push('\n');
            }
            continue;
        }
        blank_count = 0;

        // Skip git hint lines
        if trimmed.starts_with("(use \"git ") || trimmed.starts_with("(use 'git ") {
            continue;
        }

        // Skip progress bars and percentage indicators
        if PROGRESS_RE.is_match(trimmed) || PERCENT_RE.is_match(trimmed) {
            continue;
        }

        // Skip decorative separators
        if SEPARATOR_RE.is_match(trimmed) {
            continue;
        }

        // Skip carriage return lines (spinner/progress overwrite)
        if line.contains('\r') {
            // Keep only the last segment after the last \r
            if let Some(last) = line.rsplit('\r').next() {
                if !last.trim().is_empty() {
                    result.push_str(last.trim());
                    result.push('\n');
                }
            }
            continue;
        }

        result.push_str(line);
        result.push('\n');
    }

    // Trim trailing whitespace
    result.truncate(result.trim_end().len());
    result
}

// ---------------------------------------------------------------------------
// Stage 2: Command-Specific Filters
// ---------------------------------------------------------------------------

fn stage_command_specific(input: &str, command: &str) -> String {
    let cmd_lower = command.to_lowercase();

    if cmd_lower.starts_with("cargo test")
        || cmd_lower.starts_with("pytest")
        || cmd_lower.starts_with("jest")
        || cmd_lower.contains("npm test")
    {
        return filter_test_output(input);
    }

    if cmd_lower.starts_with("git status") {
        return filter_git_status(input);
    }

    if cmd_lower.starts_with("cargo build")
        || cmd_lower.starts_with("cargo check")
        || cmd_lower.starts_with("npm run build")
    {
        return filter_build_output(input);
    }

    if cmd_lower.starts_with("git diff") {
        return compact_diff(input, 200);
    }

    if cmd_lower.starts_with("git log") {
        return filter_git_log(input);
    }

    if cmd_lower.starts_with("git push") || cmd_lower.starts_with("git pull") {
        return filter_git_push_pull(input);
    }

    // npm/yarn/pnpm install
    if cmd_lower.starts_with("npm install")
        || cmd_lower.starts_with("npm i ")
        || cmd_lower.starts_with("yarn install")
        || cmd_lower.starts_with("yarn add")
        || cmd_lower.starts_with("pnpm install")
        || cmd_lower.starts_with("pnpm add")
    {
        return filter_npm_install(input);
    }

    // pip install
    if cmd_lower.starts_with("pip install") || cmd_lower.starts_with("pip3 install") {
        return filter_pip_install(input);
    }

    // docker build
    if cmd_lower.starts_with("docker build") || cmd_lower.starts_with("docker compose") {
        return filter_docker(input);
    }

    // ls / find / tree output (often huge)
    if cmd_lower.starts_with("ls ") || cmd_lower.starts_with("find ") || cmd_lower == "ls" {
        return filter_listing(input);
    }

    // webpack / vite / esbuild
    if cmd_lower.contains("webpack")
        || cmd_lower.contains("vite")
        || cmd_lower.contains("esbuild")
        || cmd_lower.contains("rollup")
    {
        return filter_bundler_output(input);
    }

    // No specific filter — return as-is
    input.to_string()
}

/// Filter test output: keep summary + failures only.
fn filter_test_output(input: &str) -> String {
    let mut result = String::new();
    let mut in_failure = false;
    let mut found_summary = false;

    for line in input.lines() {
        let trimmed = line.trim();

        // Track if we found a summary line anywhere
        if trimmed.starts_with("test result:")
            || trimmed.starts_with("Tests:")
            || trimmed.starts_with("Test Suites:")
        {
            found_summary = true;
        }

        // Keep failure sections
        if trimmed.starts_with("failures:")
            || trimmed.starts_with("FAILED")
            || trimmed.starts_with("ERRORS")
        {
            in_failure = true;
        }

        if in_failure {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        // Keep summary/result lines
        if trimmed.starts_with("test result:")
            || trimmed.starts_with("Tests:")
            || trimmed.starts_with("Test Suites:")
            || trimmed.starts_with("error")
            || trimmed.starts_with("Error")
        {
            result.push_str(line);
            result.push('\n');
        }
    }

    if !found_summary {
        // If we didn't find a recognizable test summary, return original
        return input.to_string();
    }

    result.truncate(result.trim_end().len());
    result
}

/// Filter git status: strip hint lines.
fn filter_git_status(input: &str) -> String {
    let mut result = String::new();

    for line in input.lines() {
        let trimmed = line.trim();

        // Skip hint lines
        if trimmed.starts_with("(use \"git ") || trimmed.starts_with("(use 'git ") {
            continue;
        }

        // Skip empty "nothing added to commit" style messages
        if trimmed.starts_with("no changes added") || trimmed.starts_with("nothing added") {
            continue;
        }

        result.push_str(line);
        result.push('\n');
    }

    result.truncate(result.trim_end().len());
    result
}

/// Filter build output: keep errors and warnings, strip progress.
fn filter_build_output(input: &str) -> String {
    let mut result = String::new();
    let mut in_error_block = false;

    for line in input.lines() {
        let trimmed = line.trim();

        // Always keep error/warning lines and their context
        if trimmed.starts_with("error")
            || trimmed.starts_with("warning")
            || trimmed.starts_with("Error")
            || trimmed.starts_with("Warning")
            || trimmed.contains("error[")
            || trimmed.contains("warning:")
        {
            in_error_block = true;
            result.push_str(line);
            result.push('\n');
            continue;
        }

        // Keep continuation of error blocks (indented lines or lines starting with -->)
        if in_error_block {
            if trimmed.starts_with("-->")
                || trimmed.starts_with('|')
                || trimmed.starts_with("= help")
                || trimmed.starts_with("= note")
                || line.starts_with("  ")
            {
                result.push_str(line);
                result.push('\n');
                continue;
            }
            in_error_block = false;
        }

        // Skip download/compile progress lines
        if trimmed.starts_with("Compiling ")
            || trimmed.starts_with("Downloading ")
            || trimmed.starts_with("Downloaded ")
            || trimmed.starts_with("Updating ")
        {
            continue;
        }

        // Keep summary lines
        if trimmed.starts_with("Finished ")
            || trimmed.starts_with("error:")
            || trimmed.contains("could not compile")
        {
            result.push_str(line);
            result.push('\n');
        }
    }

    if result.is_empty() {
        // Nothing interesting found — return a one-line summary
        let line_count = input.lines().count();
        return format!("(build output: {line_count} lines, no errors or warnings)");
    }

    result.truncate(result.trim_end().len());
    result
}

/// Compact a git diff: keep hunk headers + changed lines, limit per-hunk output.
/// Inspired by RTK's `compact_diff()` approach.
fn compact_diff(input: &str, max_lines: usize) -> String {
    let mut result = String::new();
    let mut current_file: Option<String> = None;
    let mut hunk_lines = 0u32;
    let mut hunk_added = 0u32;
    let mut hunk_removed = 0u32;
    let mut total_lines = 0usize;
    let max_hunk_lines: u32 = 80;

    for line in input.lines() {
        if total_lines >= max_lines {
            let remaining = input.lines().count() - total_lines;
            let _ = writeln!(result, "\n... ({remaining} more lines in diff)");
            break;
        }

        if line.starts_with("diff --git") {
            // Flush previous file stats
            flush_hunk_stats(&mut result, &mut hunk_added, &mut hunk_removed);

            // Extract filename
            if let Some(name) = line.split(" b/").nth(1) {
                current_file = Some(name.to_string());
                let _ = writeln!(result, "\n{name}");
                total_lines += 2;
            }
            hunk_lines = 0;
            continue;
        }

        // Skip index and mode lines
        if line.starts_with("index ") || line.starts_with("---") || line.starts_with("+++") {
            continue;
        }

        // Hunk header
        if line.starts_with("@@") {
            flush_hunk_stats(&mut result, &mut hunk_added, &mut hunk_removed);
            result.push_str(line);
            result.push('\n');
            total_lines += 1;
            hunk_lines = 0;
            continue;
        }

        // Changed lines
        if current_file.is_some() {
            if line.starts_with('+') || line.starts_with('-') {
                if hunk_lines < max_hunk_lines {
                    result.push_str(line);
                    result.push('\n');
                    total_lines += 1;
                }
                if line.starts_with('+') {
                    hunk_added += 1;
                } else {
                    hunk_removed += 1;
                }
                hunk_lines += 1;
            }
            // Context lines (unchanged) — keep max 2 around changes
            else if line.starts_with(' ') && hunk_lines < max_hunk_lines {
                result.push_str(line);
                result.push('\n');
                total_lines += 1;
                hunk_lines += 1;
            }
        }
    }

    // Flush final stats
    flush_hunk_stats(&mut result, &mut hunk_added, &mut hunk_removed);

    if result.is_empty() {
        return input.to_string();
    }

    result.truncate(result.trim_end().len());
    result
}

fn flush_hunk_stats(result: &mut String, added: &mut u32, removed: &mut u32) {
    if *added > 0 || *removed > 0 {
        let _ = writeln!(result, "  +{added} -{removed}");
    }
    *added = 0;
    *removed = 0;
}

/// Filter git log: strip verbose output, keep one-line summaries.
fn filter_git_log(input: &str) -> String {
    let mut result = String::new();
    let mut commit_count = 0u32;

    for line in input.lines() {
        let trimmed = line.trim();

        // Keep commit hash lines
        if trimmed.starts_with("commit ") || trimmed.starts_with("* ") {
            commit_count += 1;
            if commit_count > 50 {
                let _ = writeln!(result, "... ({} more commits)", input.lines().count() / 6);
                break;
            }
            result.push_str(line);
            result.push('\n');
            continue;
        }

        // Keep author, date, and message lines (first non-empty line after commit)
        if trimmed.starts_with("Author:")
            || trimmed.starts_with("Date:")
            || trimmed.starts_with("Merge:")
        {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        // Keep non-empty message lines (indented)
        if !trimmed.is_empty() && line.starts_with("    ") {
            result.push_str(line);
            result.push('\n');
        }
    }

    result.truncate(result.trim_end().len());
    if result.is_empty() {
        return input.to_string();
    }
    result
}

/// Filter git push/pull: extract summary.
fn filter_git_push_pull(input: &str) -> String {
    let mut result = String::new();

    for line in input.lines() {
        let trimmed = line.trim();

        // Skip progress/transfer lines
        if trimmed.starts_with("Enumerating ")
            || trimmed.starts_with("Counting ")
            || trimmed.starts_with("Compressing ")
            || trimmed.starts_with("Writing ")
            || trimmed.starts_with("Delta ")
            || trimmed.starts_with("remote: Counting")
            || trimmed.starts_with("remote: Compressing")
            || trimmed.starts_with("remote: Total")
            || trimmed.starts_with("Receiving ")
            || trimmed.starts_with("Resolving ")
            || trimmed.starts_with("Unpacking ")
        {
            continue;
        }

        if !trimmed.is_empty() {
            result.push_str(line);
            result.push('\n');
        }
    }

    result.truncate(result.trim_end().len());
    if result.is_empty() {
        return "(ok)".to_string();
    }
    result
}

/// Filter npm/yarn/pnpm install: keep summary, strip download progress.
fn filter_npm_install(input: &str) -> String {
    let mut result = String::new();

    for line in input.lines() {
        let trimmed = line.trim();

        // Skip progress/download lines
        if trimmed.starts_with("npm warn")
            || trimmed.starts_with("npm notice")
            || trimmed.starts_with("npm http")
            || trimmed.starts_with("Downloading ")
            || trimmed.starts_with("Progress:")
            || trimmed.starts_with("⸩")
            || trimmed.starts_with("⠋")
            || trimmed.starts_with("⠙")
            || trimmed.starts_with("⠹")
            || trimmed.starts_with("⠸")
            || trimmed.contains("packages in")
                && !trimmed.starts_with("added")
                && !trimmed.starts_with("removed")
                && !trimmed.starts_with("changed")
        {
            continue;
        }

        // Keep summary lines
        if trimmed.starts_with("added ")
            || trimmed.starts_with("removed ")
            || trimmed.starts_with("changed ")
            || trimmed.starts_with("up to date")
            || trimmed.contains("vulnerabilit")
            || trimmed.starts_with("found ")
            || trimmed.starts_with("Run ")
            || trimmed.starts_with("npm error")
            || trimmed.starts_with("ERR!")
            || trimmed.starts_with("error ")
            || trimmed.starts_with("success ")
            || trimmed.starts_with("Done in ")
            || trimmed.starts_with("✨")
        {
            result.push_str(line);
            result.push('\n');
        }
    }

    if result.trim().is_empty() {
        // No recognizable summary — return truncated original
        let line_count = input.lines().count();
        return format!("(npm install: {line_count} lines, completed)");
    }

    result.truncate(result.trim_end().len());
    result
}

/// Filter pip install: keep summary, strip download progress.
fn filter_pip_install(input: &str) -> String {
    let mut result = String::new();

    for line in input.lines() {
        let trimmed = line.trim();

        // Skip download/progress lines
        if trimmed.starts_with("Downloading ")
            || trimmed.starts_with("  Downloading ")
            || trimmed.contains("━━━━")
            || trimmed.starts_with("Collecting ")
            || trimmed.starts_with("  Using cached ")
            || trimmed.starts_with("  Preparing ")
            || trimmed.starts_with("  Building ")
        {
            continue;
        }

        // Keep important lines
        if trimmed.starts_with("Successfully installed")
            || trimmed.starts_with("Requirement already satisfied")
            || trimmed.starts_with("Installing ")
            || trimmed.starts_with("ERROR:")
            || trimmed.starts_with("WARNING:")
            || trimmed.starts_with("error:")
        {
            result.push_str(line);
            result.push('\n');
        }
    }

    if result.trim().is_empty() {
        let line_count = input.lines().count();
        return format!("(pip install: {line_count} lines, completed)");
    }

    result.truncate(result.trim_end().len());
    result
}

/// Filter docker build/compose output: keep errors, warnings, final summary.
fn filter_docker(input: &str) -> String {
    let mut result = String::new();

    for line in input.lines() {
        let trimmed = line.trim();

        // Skip layer progress
        if trimmed.starts_with("---")
            || trimmed.starts_with("Sending build context")
            || trimmed.starts_with("Pulling ")
            || trimmed.starts_with(" ---> ")
            || trimmed.starts_with("Removing intermediate")
            || (trimmed.starts_with("Step ") && !trimmed.contains("ERROR"))
        {
            continue;
        }

        // Keep errors, warnings, and summary
        if trimmed.starts_with("ERROR")
            || trimmed.starts_with("error")
            || trimmed.starts_with("WARNING")
            || trimmed.starts_with("Successfully built")
            || trimmed.starts_with("Successfully tagged")
            || trimmed.contains("exporting to image")
            || trimmed.starts_with("COPY ")
            || trimmed.starts_with("RUN ")
            || trimmed.starts_with("FROM ")
        {
            result.push_str(line);
            result.push('\n');
        }
    }

    if result.trim().is_empty() {
        let line_count = input.lines().count();
        return format!("(docker: {line_count} lines output)");
    }

    result.truncate(result.trim_end().len());
    result
}

/// Filter directory listings: cap at 100 entries with count.
fn filter_listing(input: &str) -> String {
    let lines: Vec<&str> = input.lines().collect();
    let total = lines.len();

    if total <= 100 {
        return input.to_string();
    }

    let mut result = String::new();
    for line in &lines[..100] {
        result.push_str(line);
        result.push('\n');
    }
    let _ = write!(result, "\n... ({} more entries, {total} total)", total - 100);
    result
}

/// Filter bundler output (webpack, vite, esbuild, rollup): keep warnings, errors, summary.
fn filter_bundler_output(input: &str) -> String {
    let mut result = String::new();
    let mut has_content = false;

    for line in input.lines() {
        let trimmed = line.trim();

        // Keep errors and warnings
        if trimmed.contains("ERROR")
            || trimmed.contains("error")
            || trimmed.contains("WARNING")
            || trimmed.contains("warning")
            || trimmed.contains("WARN")
        {
            result.push_str(line);
            result.push('\n');
            has_content = true;
            continue;
        }

        // Keep summary/result lines
        if trimmed.contains("built in")
            || trimmed.contains("compiled")
            || trimmed.contains("Bundle ")
            || trimmed.starts_with("dist/")
            || trimmed.starts_with("build/")
            || trimmed.starts_with("✓")
            || trimmed.starts_with("✔")
            || trimmed.contains("chunks")
            || trimmed.contains("modules")
            || trimmed.contains("entrypoint")
        {
            result.push_str(line);
            result.push('\n');
            has_content = true;
        }
    }

    if !has_content {
        let line_count = input.lines().count();
        return format!("(bundler: {line_count} lines, no errors)");
    }

    result.truncate(result.trim_end().len());
    result
}

// ---------------------------------------------------------------------------
// Stage 3: Truncate
// ---------------------------------------------------------------------------

fn stage_truncate(input: &str, max_chars: usize) -> String {
    if input.len() <= max_chars {
        return input.to_string();
    }

    // Head/tail mode: keep first half + last half
    let head_size = max_chars * 2 / 5;
    let tail_size = max_chars * 2 / 5;
    let omitted = input.len() - head_size - tail_size;

    let head = &input[..head_size];
    let tail = &input[input.len() - tail_size..];

    let mut result = String::with_capacity(max_chars);
    result.push_str(head);
    let _ = write!(result, "\n\n... ({omitted} characters omitted) ...\n\n");
    result.push_str(tail);
    result
}

// ---------------------------------------------------------------------------
// Stage 4: Deduplicate
// ---------------------------------------------------------------------------

fn stage_dedup(input: &str) -> String {
    static TIMESTAMP_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}[\.\d]*[Z]?").expect("valid regex")
    });
    static UUID_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
            .expect("valid regex")
    });
    static HEX_HASH_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b[0-9a-f]{7,64}\b").expect("valid regex"));
    static NUMERIC_ID_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b\d{6,}\b").expect("valid regex"));

    let lines: Vec<&str> = input.lines().collect();
    if lines.is_empty() {
        return input.to_string();
    }

    let mut result = String::with_capacity(input.len());
    let mut prev_normalized: Option<String> = None;
    let mut prev_line: Option<&str> = None;
    let mut repeat_count: u32 = 0;

    for line in &lines {
        // Normalize for comparison: replace timestamps, UUIDs, hashes, large numbers
        let normalized =
            normalize_for_dedup(line, &TIMESTAMP_RE, &UUID_RE, &HEX_HASH_RE, &NUMERIC_ID_RE);

        if prev_normalized.as_deref() == Some(normalized.as_str()) {
            repeat_count += 1;
        } else {
            // Flush previous
            if let Some(prev) = prev_line {
                if repeat_count > 0 {
                    let _ = writeln!(result, "{prev} (\u{00d7}{count})", count = repeat_count + 1);
                } else {
                    result.push_str(prev);
                    result.push('\n');
                }
            }
            prev_normalized = Some(normalized);
            prev_line = Some(line);
            repeat_count = 0;
        }
    }

    // Flush last
    if let Some(prev) = prev_line {
        if repeat_count > 0 {
            let _ = writeln!(result, "{prev} (\u{00d7}{count})", count = repeat_count + 1);
        } else {
            result.push_str(prev);
            result.push('\n');
        }
    }

    result.truncate(result.trim_end().len());
    result
}

/// Normalize a line for dedup comparison by replacing variable parts with placeholders.
///
/// This catches repeated log lines that only differ in timestamp, request ID, etc.
fn normalize_for_dedup(
    line: &str,
    timestamp_re: &Regex,
    uuid_re: &Regex,
    hex_hash_re: &Regex,
    numeric_id_re: &Regex,
) -> String {
    static TIMING_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\d+\.?\d*\s*(?:ms|s|us|ns|µs)\b").expect("valid regex"));
    static BYTE_SIZE_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\d+\.?\d*\s*(?:B|KB|MB|GB|TB|kB|bytes)\b").expect("valid regex")
    });

    let s = timestamp_re.replace_all(line, "<TS>");
    let s = uuid_re.replace_all(&s, "<UUID>");
    let s = TIMING_RE.replace_all(&s, "<DUR>");
    let s = BYTE_SIZE_RE.replace_all(&s, "<SIZE>");
    let s = hex_hash_re.replace_all(&s, "<HEX>");
    let s = numeric_id_re.replace_all(&s, "<NUM>");
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_strips_ansi_codes() {
        let input = "\x1b[32m✓\x1b[0m test passed\n\x1b[31m✗\x1b[0m test failed";
        let result = stage_filter(input);
        assert!(!result.contains("\x1b["));
        assert!(result.contains("✓ test passed"));
        assert!(result.contains("✗ test failed"));
    }

    #[test]
    fn filter_strips_git_hints() {
        let input = "On branch main\n(use \"git add\" to track)\n\nnew file: foo.rs";
        let result = stage_filter(input);
        assert!(!result.contains("use \"git add\""));
        assert!(result.contains("new file: foo.rs"));
    }

    #[test]
    fn filter_collapses_blank_lines() {
        let input = "line1\n\n\n\n\nline2\n\n\n\nline3";
        let result = stage_filter(input);
        assert_eq!(result, "line1\n\nline2\n\nline3");
    }

    #[test]
    fn filter_strips_separators() {
        let input = "header\n---\ncontent\n===\nfooter";
        let result = stage_filter(input);
        assert!(!result.contains("---"));
        assert!(!result.contains("==="));
        assert!(result.contains("header"));
        assert!(result.contains("content"));
        assert!(result.contains("footer"));
    }

    #[test]
    fn dedup_collapses_repeated_lines() {
        let input = "error: timeout\nerror: timeout\nerror: timeout\nok";
        let result = stage_dedup(input);
        assert!(result.contains("error: timeout (\u{00d7}3)"));
        assert!(result.contains("ok"));
    }

    #[test]
    fn dedup_preserves_unique_lines() {
        let input = "line1\nline2\nline3";
        let result = stage_dedup(input);
        assert_eq!(result, "line1\nline2\nline3");
    }

    #[test]
    fn truncate_preserves_small_output() {
        let input = "short output";
        let result = stage_truncate(input, 1000);
        assert_eq!(result, input);
    }

    #[test]
    fn truncate_splits_large_output() {
        let input = "x".repeat(10_000);
        let result = stage_truncate(&input, 1000);
        assert!(result.len() < input.len());
        assert!(result.contains("characters omitted"));
    }

    #[test]
    fn compress_shell_output_full_pipeline() {
        let input = "\x1b[32mCompiling\x1b[0m foo v0.1.0\n\
                     \x1b[32mCompiling\x1b[0m bar v0.2.0\n\
                     \x1b[32mCompiling\x1b[0m baz v0.3.0\n\
                     \x1b[32m  Finished\x1b[0m dev [unoptimized] target(s) in 1.23s";

        let result = compress_shell_output(input, "cargo build", 16000);
        assert!(result.ratio() > 0.0);
        assert!(!result.text.contains("\x1b["));
    }

    #[test]
    fn test_output_filter_keeps_failures() {
        let input = "\
running 10 tests
test foo ... ok
test bar ... ok
test baz ... FAILED

failures:

---- baz stdout ----
assertion failed: expected 1, got 2

failures:
    baz

test result: FAILED. 9 passed; 1 failed";

        let result = filter_test_output(input);
        assert!(result.contains("FAILED"), "should contain FAILED: {result}");
        assert!(result.contains("assertion failed"), "should contain assertion: {result}");
        assert!(result.contains("9 passed; 1 failed"), "should contain summary: {result}");
        // Should NOT contain individual "ok" test lines
        assert!(!result.contains("test foo ... ok"), "should not contain ok lines: {result}");
    }

    #[test]
    fn git_status_filter_strips_hints() {
        let input = "On branch main\n\
                     Changes not staged for commit:\n\
                     (use \"git add <file>...\" to update)\n\n\
                     \tmodified:   src/main.rs\n\n\
                     no changes added to commit";

        let result = filter_git_status(input);
        assert!(result.contains("modified:   src/main.rs"));
        assert!(!result.contains("use \"git add"));
        assert!(!result.contains("no changes added"));
    }

    #[test]
    fn npm_install_filter_keeps_summary() {
        let input = "\
npm warn deprecated inflight@1.0.6
npm warn deprecated glob@7.2.3
added 542 packages in 12s
82 packages are looking for funding
  run `npm fund` for details
3 moderate severity vulnerabilities";

        let result = filter_npm_install(input);
        assert!(result.contains("added 542 packages"), "summary: {result}");
        assert!(result.contains("vulnerabilit"), "vulns: {result}");
        assert!(!result.contains("warn deprecated"), "no warnings: {result}");
    }

    #[test]
    fn pip_install_filter_keeps_summary() {
        let input = "\
Collecting flask>=2.0
  Downloading Flask-3.0.0-py3-none-any.whl (101 kB)
  ━━━━━━━━━━━━━━━━━ 101.2/101.2 kB 2.1 MB/s
Collecting werkzeug>=3.0
  Using cached werkzeug-3.0.1-py3-none-any.whl
Successfully installed Flask-3.0.0 Jinja2-3.1.2 werkzeug-3.0.1";

        let result = filter_pip_install(input);
        assert!(result.contains("Successfully installed"), "summary: {result}");
        assert!(!result.contains("━━━━"), "no progress: {result}");
        assert!(!result.contains("Downloading"), "no download: {result}");
    }

    #[test]
    fn docker_filter_keeps_errors_and_summary() {
        let input = "\
Sending build context to Docker daemon  2.048kB
Step 1/5 : FROM node:18
 ---> abc123def456
Step 2/5 : COPY package.json .
Step 3/5 : RUN npm install
ERROR: failed to solve: process /bin/sh -c npm install did not complete successfully";

        let result = filter_docker(input);
        assert!(result.contains("ERROR"), "error: {result}");
        assert!(!result.contains("Sending build context"), "no context: {result}");
        assert!(!result.contains("---> abc"), "no layer hash: {result}");
    }

    #[test]
    fn listing_filter_caps_at_100() {
        let input: String =
            (0..500).map(|i| format!("file_{i}.txt")).collect::<Vec<_>>().join("\n");
        let result = filter_listing(&input);
        assert!(result.contains("400 more entries"), "capped: {result}");
        assert!(result.contains("500 total"), "total: {result}");
    }

    #[test]
    fn listing_filter_preserves_small() {
        let input = "a.txt\nb.txt\nc.txt";
        let result = filter_listing(input);
        assert_eq!(result, input);
    }

    #[test]
    fn bundler_filter_keeps_errors_and_summary() {
        let input = "\
  vite v5.0.0 building for production...
  transforming (423) src/components/App.tsx
  ✓ 423 modules transformed.
  dist/index.html     0.46 kB │ gzip: 0.30 kB
  dist/assets/index-abc123.js  145.67 kB │ gzip: 47.23 kB
  ✓ built in 1.23s";

        let result = filter_bundler_output(input);
        assert!(result.contains("built in"), "summary: {result}");
        assert!(result.contains("modules"), "modules: {result}");
        assert!(!result.contains("transforming"), "no progress: {result}");
    }

    #[test]
    fn dedup_with_log_normalization() {
        let input = "\
2024-03-28T10:00:01Z [INFO] Request abc123de processed in 42ms
2024-03-28T10:00:02Z [INFO] Request def456ab processed in 38ms
2024-03-28T10:00:03Z [INFO] Request 99887766 processed in 41ms
Done";

        let result = stage_dedup(input);
        // Should collapse the 3 similar log lines into one with ×3
        assert!(result.contains("\u{00d7}3"), "should dedup: {result}");
        assert!(result.contains("Done"), "should keep Done: {result}");
    }

    #[test]
    fn dedup_normalization_preserves_different_lines() {
        let input = "\
[INFO] Starting server on port 8080
[ERROR] Connection refused to database
[INFO] Retrying connection";

        let result = stage_dedup(input);
        // All 3 lines are structurally different, should be preserved
        assert!(result.contains("Starting server"), "line 1: {result}");
        assert!(result.contains("Connection refused"), "line 2: {result}");
        assert!(result.contains("Retrying"), "line 3: {result}");
    }

    #[test]
    fn token_budget_truncation_preserves_small() {
        let input = "short text that fits in budget";
        let counter = crate::token::TokenCounter::for_model("anthropic/claude-sonnet-4");
        let result = stage_truncate_tokens(input, 100, &counter);
        assert_eq!(result, input);
    }

    #[test]
    fn token_budget_truncation_splits_large() {
        // Create a large input that exceeds the token budget
        let lines: Vec<String> = (0..200).map(|i| format!("Line {i}: some content here")).collect();
        let input = lines.join("\n");
        let counter = crate::token::TokenCounter::for_model("anthropic/claude-sonnet-4");

        let result = stage_truncate_tokens(&input, 50, &counter);
        assert!(result.contains("tokens omitted"), "should have omission marker: {result}");
        assert!(result.len() < input.len(), "should be shorter");
        // Should contain early lines and late lines
        assert!(result.contains("Line 0"), "should have head");
    }

    #[test]
    fn compress_shell_output_token_budget_works() {
        let input = "\x1b[32mCompiling\x1b[0m foo v0.1.0\nFinished dev in 1.0s";
        let result = compress_shell_output_token_budget(
            input,
            "cargo build",
            1000,
            "anthropic/claude-sonnet-4",
        );
        assert!(result.ratio() >= 0.0);
        assert!(!result.text.contains("\x1b["));
    }

    #[test]
    fn build_filter_keeps_errors() {
        let input = "Compiling foo v0.1.0\n\
                     Compiling bar v0.2.0\n\
                     error[E0308]: mismatched types\n\
                      --> src/main.rs:10:5\n\
                       |\n  \
                     10 |     let x: u32 = \"hello\";\n\
                       |                 ^^^^^^^ expected u32\n\
                     Finished dev in 1.0s";

        let result = filter_build_output(input);
        assert!(result.contains("error[E0308]"));
        assert!(result.contains("src/main.rs:10:5"));
        assert!(!result.contains("Compiling foo"));
        assert!(!result.contains("Compiling bar"));
    }
}
