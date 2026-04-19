//! Token-budget-aware compression for tool output.
//!
//! 4-stage pipeline: Filter → Group → Truncate → Deduplicate, plus a final
//! hard budget check and a passthrough fast path.

use std::sync::LazyLock;

use regex::Regex;

use crate::config::OutputCompressionConfig;

static ANSI_ESCAPE_RE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").ok());
static PROGRESS_RE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^[\s\[\]=>0-9.%]+$").ok());
static NORMALIZE_TRAILING_RE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?:[\s:/._-]*[#\d][#\d.%:/_-]*)+\s*$").ok());

/// Compression pipeline bound to runtime config.
pub(crate) struct CompressionPipeline<'cfg> {
    cfg: &'cfg OutputCompressionConfig,
}

/// Summary of one compression run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompressionResult {
    pub output: String,
    pub original_lines: usize,
    pub final_lines: usize,
    pub original_chars: usize,
    pub final_chars: usize,
    pub stages_applied: Vec<&'static str>,
}

impl<'cfg> CompressionPipeline<'cfg> {
    pub(crate) fn new(cfg: &'cfg OutputCompressionConfig) -> Self {
        Self { cfg }
    }

    pub(crate) fn compress(&self, input: &str) -> CompressionResult {
        let original_lines = line_count(input);
        let original_chars = char_count(input);

        if original_lines <= self.cfg.passthrough_threshold_lines {
            return CompressionResult {
                output: input.to_string(),
                original_lines,
                final_lines: original_lines,
                original_chars,
                final_chars: original_chars,
                stages_applied: Vec::new(),
            };
        }

        let mut stages_applied = Vec::new();
        let mut lines = input.split('\n').map(str::to_string).collect::<Vec<_>>();

        let filtered = self.filter(lines.clone());
        if filtered != lines {
            self.log_stage("filter", &lines, &filtered);
            stages_applied.push("filter");
            lines = filtered;
        }

        let grouped = self.group(lines.clone());
        if grouped != lines {
            self.log_stage("group", &lines, &grouped);
            stages_applied.push("group");
            lines = grouped;
        }

        let truncated = self.truncate(lines.clone());
        if truncated != lines {
            self.log_stage("truncate", &lines, &truncated);
            stages_applied.push("truncate");
            lines = truncated;
        }

        let deduplicated = self.deduplicate(lines.clone());
        if deduplicated != lines {
            self.log_stage("deduplicate", &lines, &deduplicated);
            stages_applied.push("deduplicate");
            lines = deduplicated;
        }

        let mut output = lines.join("\n");
        if char_count(&output) > self.cfg.max_chars {
            let truncated_output = hard_truncate_chars(&output, self.cfg.max_chars);
            if truncated_output != output {
                tracing::debug!(
                    stage = "budget",
                    before_chars = char_count(&output),
                    after_chars = char_count(&truncated_output),
                    "tool output compression stage applied"
                );
                stages_applied.push("budget");
                output = truncated_output;
            }
        }

        let final_lines = line_count(&output);
        let final_chars = char_count(&output);

        CompressionResult {
            output,
            original_lines,
            final_lines,
            original_chars,
            final_chars,
            stages_applied,
        }
    }

    #[expect(
        clippy::unused_self,
        reason = "Uniform stage method shape for testing and pipeline composition"
    )]
    pub(crate) fn filter(&self, lines: Vec<String>) -> Vec<String> {
        let mut filtered = Vec::with_capacity(lines.len());
        let mut previous_blank = false;

        for raw_line in lines {
            let stripped_ansi = strip_ansi_sequences(&raw_line);
            let last_frame =
                stripped_ansi.rsplit('\r').next().map_or_else(String::new, ToString::to_string);
            let trimmed = last_frame.trim_end().to_string();

            if trimmed.trim().is_empty() {
                if !previous_blank {
                    filtered.push(String::new());
                    previous_blank = true;
                }
                continue;
            }
            previous_blank = false;

            if is_progress_line(&trimmed) {
                continue;
            }

            filtered.push(trimmed);
        }

        filtered
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "Pipeline stages intentionally own line buffers"
    )]
    pub(crate) fn group(&self, lines: Vec<String>) -> Vec<String> {
        let mut grouped = Vec::new();
        let mut index = 0;

        while index < lines.len() {
            let exact_run = run_length(&lines, index, |candidate| candidate == &lines[index]);
            if exact_run >= self.cfg.group_exact_min {
                grouped.push(lines[index].clone());
                grouped.push(format!("... (× {exact_run} times)"));
                index += exact_run;
                continue;
            }

            let normalized = normalize_for_grouping(&lines[index]);
            let similar_run = if normalized.is_empty() {
                1
            } else {
                run_length(&lines, index, |candidate| {
                    normalize_for_grouping(candidate) == normalized
                })
            };

            if similar_run >= self.cfg.group_similar_min {
                grouped.push(lines[index].clone());
                grouped.push(format!("... (× {similar_run} similar lines)"));
                index += similar_run;
                continue;
            }

            grouped.push(lines[index].clone());
            index += 1;
        }

        grouped
    }

    pub(crate) fn truncate(&self, lines: Vec<String>) -> Vec<String> {
        if lines.len() <= self.cfg.max_lines {
            return lines;
        }

        let head_len = self.cfg.head_lines.min(lines.len());
        let tail_len = self.cfg.tail_lines.min(lines.len().saturating_sub(head_len));

        if head_len + tail_len >= lines.len() {
            return lines;
        }

        let elided = lines.len() - head_len - tail_len;
        let mut truncated = Vec::with_capacity(head_len + tail_len + 1);
        truncated.extend(lines[..head_len].iter().cloned());
        truncated.push(format!("... [{elided} lines elided by compression] ..."));
        truncated.extend(lines[lines.len() - tail_len..].iter().cloned());
        truncated
    }

    #[expect(
        clippy::unused_self,
        reason = "Uniform stage method shape for testing and pipeline composition"
    )]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Pipeline stages intentionally own line buffers"
    )]
    pub(crate) fn deduplicate(&self, lines: Vec<String>) -> Vec<String> {
        let mut deduplicated = Vec::new();
        let mut index = 0;

        while index < lines.len() {
            if let Some(block_len) = longest_previous_block_match(&lines, index) {
                deduplicated.push(format!("... [duplicate {block_len}-line block elided] ..."));
                index += block_len;
                continue;
            }

            deduplicated.push(lines[index].clone());
            index += 1;
        }

        deduplicated
    }

    #[expect(
        clippy::unused_self,
        reason = "Uniform stage method shape for testing and pipeline composition"
    )]
    fn log_stage(&self, stage: &'static str, before: &[String], after: &[String]) {
        tracing::debug!(
            stage,
            before_lines = before.len(),
            after_lines = after.len(),
            before_chars = joined_char_count(before),
            after_chars = joined_char_count(after),
            "tool output compression stage applied"
        );
    }
}

fn strip_ansi_sequences(line: &str) -> String {
    ANSI_ESCAPE_RE
        .as_ref()
        .map_or_else(|| line.to_string(), |regex| regex.replace_all(line, "").into_owned())
}

fn is_progress_line(line: &str) -> bool {
    PROGRESS_RE.as_ref().is_some_and(|regex| {
        regex.is_match(line)
            && (line.contains('%')
                || line.contains('=')
                || line.contains('>')
                || line.contains('[')
                || line.contains(']'))
    })
}

fn normalize_for_grouping(line: &str) -> String {
    NORMALIZE_TRAILING_RE.as_ref().map_or_else(
        || line.trim_end().to_string(),
        |regex| regex.replace(line.trim_end(), "").trim_end().to_string(),
    )
}

fn run_length(lines: &[String], start: usize, predicate: impl Fn(&String) -> bool) -> usize {
    let mut len = 0;
    while start + len < lines.len() && predicate(&lines[start + len]) {
        len += 1;
    }
    len
}

fn longest_previous_block_match(lines: &[String], start: usize) -> Option<usize> {
    let mut best: Option<usize> = None;

    for previous_start in 0..start {
        let mut length = 0;
        while start + length < lines.len()
            && previous_start + length < start
            && lines[previous_start + length] == lines[start + length]
        {
            length += 1;
        }

        if length >= 3 {
            best = Some(best.map_or(length, |current| current.max(length)));
        }
    }

    best
}

fn hard_truncate_chars(input: &str, max_chars: usize) -> String {
    let total_chars = char_count(input);
    if total_chars <= max_chars {
        return input.to_string();
    }

    let marker = format!("\n... [output hard-truncated to {max_chars} chars] ...\n");
    let marker_chars = char_count(&marker);
    if marker_chars >= max_chars {
        return input.chars().take(max_chars).collect();
    }

    let keep_each_side = (max_chars - marker_chars) / 2;
    let head: String = input.chars().take(keep_each_side).collect();
    let tail: String =
        input.chars().rev().take(keep_each_side).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{head}{marker}{tail}")
}

fn char_count(input: &str) -> usize {
    input.chars().count()
}

fn line_count(input: &str) -> usize {
    if input.is_empty() {
        0
    } else {
        input.lines().count()
    }
}

fn joined_char_count(lines: &[String]) -> usize {
    lines.iter().map(|line| char_count(line)).sum::<usize>() + lines.len().saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> OutputCompressionConfig {
        OutputCompressionConfig {
            passthrough_threshold_lines: 5,
            max_lines: 6,
            head_lines: 2,
            tail_lines: 2,
            max_chars: 80,
            group_exact_min: 3,
            group_similar_min: 5,
            ..OutputCompressionConfig::default()
        }
    }

    #[test]
    fn passthrough_short_output_skips_pipeline() {
        let cfg = OutputCompressionConfig { passthrough_threshold_lines: 40, ..test_config() };
        let pipeline = CompressionPipeline::new(&cfg);
        let input = (1..=20).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");

        let result = pipeline.compress(&input);

        assert_eq!(result.output, input);
        assert!(result.stages_applied.is_empty());
    }

    #[test]
    fn filter_strips_ansi_sequences() {
        let cfg = test_config();
        let pipeline = CompressionPipeline::new(&cfg);
        let filtered = pipeline.filter(vec!["\u{1b}[31mred\u{1b}[0m".to_string()]);
        assert_eq!(filtered, vec!["red".to_string()]);
    }

    #[test]
    fn filter_strips_carriage_return_spam() {
        let cfg = test_config();
        let pipeline = CompressionPipeline::new(&cfg);
        let filtered =
            pipeline.filter(vec!["progress 10%\rprogress 50%\rprogress 100%".to_string()]);
        assert_eq!(filtered, vec!["progress 100%".to_string()]);
    }

    #[test]
    fn filter_collapses_blank_line_runs() {
        let cfg = test_config();
        let pipeline = CompressionPipeline::new(&cfg);
        let filtered = pipeline.filter(vec![
            "alpha".to_string(),
            String::new(),
            String::new(),
            "beta".to_string(),
            String::new(),
            String::new(),
        ]);

        assert_eq!(
            filtered,
            vec!["alpha".to_string(), String::new(), "beta".to_string(), String::new(),]
        );
    }

    #[test]
    fn group_exact_repeats() {
        let cfg = test_config();
        let pipeline = CompressionPipeline::new(&cfg);
        let grouped = pipeline.group(vec!["same".to_string(); 5]);
        assert_eq!(grouped, vec!["same".to_string(), "... (× 5 times)".to_string()]);
    }

    #[test]
    fn group_similar_by_normalized_form() {
        let cfg = test_config();
        let pipeline = CompressionPipeline::new(&cfg);
        let grouped = pipeline
            .group((1..=10).map(|n| format!("[build] compiling foo/{n}.2.3")).collect::<Vec<_>>());

        assert_eq!(
            grouped,
            vec!["[build] compiling foo/1.2.3".to_string(), "... (× 10 similar lines)".to_string(),]
        );
    }

    #[test]
    fn truncate_preserves_head_and_tail() {
        let cfg = test_config();
        let pipeline = CompressionPipeline::new(&cfg);
        let lines = (0..10).map(|n| format!("line {n}")).collect::<Vec<_>>();

        let truncated = pipeline.truncate(lines);

        assert_eq!(
            truncated,
            vec![
                "line 0".to_string(),
                "line 1".to_string(),
                "... [6 lines elided by compression] ...".to_string(),
                "line 8".to_string(),
                "line 9".to_string(),
            ]
        );
    }

    #[test]
    fn deduplicate_removes_repeated_blocks() {
        let cfg = test_config();
        let pipeline = CompressionPipeline::new(&cfg);
        let lines = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
            "e".to_string(),
            "x".to_string(),
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
            "e".to_string(),
        ];

        let deduplicated = pipeline.deduplicate(lines);

        assert_eq!(
            deduplicated,
            vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string(),
                "e".to_string(),
                "x".to_string(),
                "... [duplicate 5-line block elided] ...".to_string(),
            ]
        );
    }

    #[test]
    fn final_budget_cap_hard_truncates_chars() {
        let cfg = OutputCompressionConfig {
            passthrough_threshold_lines: 0,
            max_lines: 1_000,
            max_chars: 60,
            group_exact_min: 100,
            group_similar_min: 100,
            ..test_config()
        };
        let pipeline = CompressionPipeline::new(&cfg);
        let input = (0..20).map(|n| format!("line-{n:02}")).collect::<Vec<_>>().join("\n");

        let result = pipeline.compress(&input);

        assert!(result.output.contains("hard-truncated"));
        assert!(result.final_chars <= cfg.max_chars);
        assert!(result.stages_applied.contains(&"budget"));
    }

    #[test]
    fn stages_applied_reflects_triggered_stages() {
        let cfg = OutputCompressionConfig {
            passthrough_threshold_lines: 0,
            max_lines: 4,
            head_lines: 1,
            tail_lines: 1,
            max_chars: 1_000,
            ..test_config()
        };
        let pipeline = CompressionPipeline::new(&cfg);
        let input =
            ["\u{1b}[32mkeep\u{1b}[0m", "same", "same", "same", "tail-0", "tail-1", "tail-2"]
                .join("\n");

        let result = pipeline.compress(&input);

        assert_eq!(result.stages_applied, vec!["filter", "group", "truncate"]);
    }
}
