//! Core apply engine — merges a lazy edit snippet into an original file.

use crate::fuzzy;

/// The strategy that was used to apply the edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// Ellipsis markers were detected and resolved against the original.
    EllipsisMerge,
    /// The snippet was matched as a contiguous block via line-level fuzzy matching.
    FuzzyMatch,
    /// The snippet was applied as a complete file replacement.
    FullFile,
}

impl std::fmt::Display for Strategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EllipsisMerge => write!(f, "ellipsis-merge"),
            Self::FuzzyMatch => write!(f, "fuzzy-match"),
            Self::FullFile => write!(f, "full-file"),
        }
    }
}

/// The result of applying an edit.
#[derive(Debug, Clone)]
pub struct ApplyResult {
    /// The new file content after the edit.
    pub content: String,
    /// Which strategy was used.
    pub strategy: Strategy,
}

/// Apply an edit snippet to an original file.
///
/// The snippet may contain ellipsis markers like `// ... existing code ...`
/// to indicate unchanged regions that should be preserved from the original.
///
/// Returns the merged file content and which strategy was used.
///
/// # Errors
///
/// Returns an error if the snippet cannot be matched against the original
/// by any strategy.
pub fn apply_edit(original: &str, snippet: &str) -> Result<ApplyResult, ApplyError> {
    // Strategy 1: Ellipsis merge
    if has_ellipsis_markers(snippet) {
        match try_ellipsis_merge(original, snippet) {
            Ok(content) => {
                return Ok(ApplyResult { content, strategy: Strategy::EllipsisMerge });
            }
            Err(e) => {
                tracing::debug!("ellipsis merge failed: {e}, trying fuzzy match");
            }
        }
    }

    // Strategy 2: Line-level fuzzy match (snippet is a contiguous replacement block)
    match try_fuzzy_replace(original, snippet) {
        Ok(content) => {
            return Ok(ApplyResult { content, strategy: Strategy::FuzzyMatch });
        }
        Err(e) => {
            tracing::debug!("fuzzy match failed: {e}, falling back to full file");
        }
    }

    // Strategy 3: Full file replacement
    // If the snippet looks like a complete file (similar line count or has
    // structural indicators like imports/mod declarations at the top), use it as-is.
    if looks_like_full_file(original, snippet) {
        return Ok(ApplyResult { content: snippet.to_string(), strategy: Strategy::FullFile });
    }

    Err(ApplyError::NoStrategyMatched)
}

/// Error type for apply operations.
#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    /// No context lines from the snippet could be matched in the original.
    #[error("could not match snippet context against the original file")]
    NoContextMatch,
    /// None of the strategies could apply the snippet.
    #[error("no strategy could apply this snippet to the original file")]
    NoStrategyMatched,
}

// ---------------------------------------------------------------------------
// Ellipsis detection
// ---------------------------------------------------------------------------

/// Patterns that indicate an ellipsis/continuation marker in a code snippet.
const ELLIPSIS_PATTERNS: &[&str] = &[
    "... existing code ...",
    "... existing ...",
    "...existing code...",
    "// ...",
    "# ...",
    "/* ... */",
    "-- ...",
    "/// ...",
    "// rest of the code",
    "// ... rest",
    "// ... remaining",
    "// (rest of file)",
    "# rest of the code",
    "// unchanged",
    "# unchanged",
    "// keep existing",
    "// existing code remains",
    "// same as before",
    "// no changes",
];

/// Check if a line is an ellipsis marker.
fn is_ellipsis_line(line: &str) -> bool {
    let trimmed = line.trim().to_lowercase();
    if trimmed.is_empty() {
        return false;
    }

    // Check against known patterns
    for pattern in ELLIPSIS_PATTERNS {
        if trimmed.contains(pattern) {
            return true;
        }
    }

    // Generic pattern: a comment containing "..." with few other chars
    // e.g., "    // ..." or "    # ..."
    let stripped = trimmed
        .trim_start_matches("//")
        .trim_start_matches('#')
        .trim_start_matches("/*")
        .trim_end_matches("*/")
        .trim_start_matches("--")
        .trim();

    if stripped == "..." || stripped == "…" {
        return true;
    }

    false
}

/// Check if a snippet contains any ellipsis markers.
fn has_ellipsis_markers(snippet: &str) -> bool {
    snippet.lines().any(is_ellipsis_line)
}

// ---------------------------------------------------------------------------
// Strategy 1: Ellipsis merge
// ---------------------------------------------------------------------------

/// Try to merge a snippet with ellipsis markers into the original.
///
/// The algorithm:
/// 1. Split the snippet at each ellipsis marker into "segments"
/// 2. Each segment is a block of code lines that should appear in the output
/// 3. For each segment, find where it matches in the original
/// 4. Between segments, preserve the original content
fn try_ellipsis_merge(original: &str, snippet: &str) -> Result<String, ApplyError> {
    let orig_lines: Vec<&str> = original.lines().collect();
    let snippet_lines: Vec<&str> = snippet.lines().collect();

    // Split snippet into segments separated by ellipsis markers.
    // Each segment is a Vec of non-ellipsis lines.
    let segments = split_at_ellipsis(&snippet_lines);

    if segments.is_empty() {
        return Err(ApplyError::NoContextMatch);
    }

    // If there's only one segment (no ellipsis), this isn't an ellipsis merge
    if segments.len() == 1 && !snippet_lines.iter().any(|l| is_ellipsis_line(l)) {
        return Err(ApplyError::NoContextMatch);
    }

    // Match each segment to the original file
    let mut result = Vec::new();
    let mut orig_pos = 0;

    // Check if snippet starts with ellipsis (preserve beginning of file)
    let starts_with_ellipsis = snippet_lines.first().is_some_and(|l| is_ellipsis_line(l));

    // Check if snippet ends with ellipsis (preserve end of file)
    let ends_with_ellipsis = snippet_lines.last().is_some_and(|l| is_ellipsis_line(l));

    for (i, segment) in segments.iter().enumerate() {
        if segment.is_empty() {
            continue;
        }

        // Find where this segment's "anchor" lines match in the original
        // Use the first few non-blank lines as anchor
        let anchors: Vec<&str> =
            segment.iter().copied().filter(|l| !l.trim().is_empty()).take(3).collect();

        if anchors.is_empty() {
            // All-blank segment — just add it
            for &line in segment {
                result.push(line.to_string());
            }
            continue;
        }

        // Search for the anchor in the original, starting from our current position
        let search_region: Vec<&str> = orig_lines[orig_pos..].to_vec();
        let match_result = fuzzy::find_best_match(&search_region, &anchors, 0.7);

        match match_result {
            Some((rel_offset, _score)) => {
                let abs_offset = orig_pos + rel_offset;

                // If this is not the first segment, or if we start with ellipsis,
                // preserve original lines between last position and this match
                if i > 0 || starts_with_ellipsis {
                    for &line in &orig_lines[orig_pos..abs_offset] {
                        result.push(line.to_string());
                    }
                }

                // Add the segment's lines (the new code)
                for &line in segment {
                    result.push(line.to_string());
                }

                // Advance past the matched region
                orig_pos = abs_offset + segment.len();
            }
            None => {
                // Could not find this segment in the original.
                // For the first segment without leading ellipsis, it might be
                // new code at the very beginning — just add it.
                if i == 0 && !starts_with_ellipsis {
                    for &line in segment {
                        result.push(line.to_string());
                    }
                } else {
                    // Can't match — fail
                    return Err(ApplyError::NoContextMatch);
                }
            }
        }
    }

    // If snippet ends with ellipsis, preserve remaining original content
    if ends_with_ellipsis && orig_pos < orig_lines.len() {
        for &line in &orig_lines[orig_pos..] {
            result.push(line.to_string());
        }
    }

    Ok(result.join("\n"))
}

/// Split snippet lines at ellipsis markers into segments.
fn split_at_ellipsis<'a>(lines: &[&'a str]) -> Vec<Vec<&'a str>> {
    let mut segments = Vec::new();
    let mut current = Vec::new();

    for &line in lines {
        if is_ellipsis_line(line) {
            if !current.is_empty() {
                segments.push(current);
                current = Vec::new();
            }
        } else {
            current.push(line);
        }
    }

    if !current.is_empty() {
        segments.push(current);
    }

    segments
}

// ---------------------------------------------------------------------------
// Strategy 2: Fuzzy match replacement
// ---------------------------------------------------------------------------

/// Try to find a region in the original that matches the snippet and replace it.
///
/// This handles cases where the LLM provides a modified version of a code
/// block but with minor whitespace or formatting differences.
fn try_fuzzy_replace(original: &str, snippet: &str) -> Result<String, ApplyError> {
    let orig_lines: Vec<&str> = original.lines().collect();
    let snippet_lines: Vec<&str> = snippet.lines().collect();

    if snippet_lines.is_empty() {
        return Err(ApplyError::NoContextMatch);
    }

    // Need at least a few lines to do meaningful fuzzy matching
    if snippet_lines.len() < 2 {
        return Err(ApplyError::NoContextMatch);
    }

    // Take anchor lines from the snippet (first and last non-blank lines)
    // to narrow down where this block belongs
    let first_nonblank: Vec<&str> =
        snippet_lines.iter().copied().filter(|l| !l.trim().is_empty()).take(3).collect();

    let last_nonblank: Vec<&str> = snippet_lines
        .iter()
        .copied()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    if first_nonblank.is_empty() {
        return Err(ApplyError::NoContextMatch);
    }

    // Find where the first anchor lines match in the original
    let start_match = fuzzy::find_best_match(&orig_lines, &first_nonblank, 0.75);
    let Some((start_pos, _)) = start_match else {
        return Err(ApplyError::NoContextMatch);
    };

    // Find where the last anchor lines match, searching from near the end
    // of where we'd expect the block to end
    let expected_end = (start_pos + snippet_lines.len()).min(orig_lines.len());
    let search_start = if expected_end > last_nonblank.len() + 3 {
        expected_end - last_nonblank.len() - 3
    } else {
        start_pos
    };
    let search_end = (expected_end + 5).min(orig_lines.len());
    let search_region: Vec<&str> = orig_lines[search_start..search_end].to_vec();

    let end_match = fuzzy::find_best_match(&search_region, &last_nonblank, 0.75);
    let end_pos = match end_match {
        Some((rel_offset, _)) => search_start + rel_offset + last_nonblank.len(),
        None => {
            // Fall back: assume the block is the same length as the snippet
            (start_pos + snippet_lines.len()).min(orig_lines.len())
        }
    };

    // Reconstruct: original before + snippet + original after
    let mut result: Vec<String> = Vec::new();
    for &line in &orig_lines[..start_pos] {
        result.push(line.to_string());
    }
    for &line in &snippet_lines {
        result.push(line.to_string());
    }
    for &line in &orig_lines[end_pos..] {
        result.push(line.to_string());
    }

    Ok(result.join("\n"))
}

// ---------------------------------------------------------------------------
// Strategy 3: Full file detection
// ---------------------------------------------------------------------------

/// Heuristic: does the snippet look like a complete file?
fn looks_like_full_file(original: &str, snippet: &str) -> bool {
    let orig_lines = original.lines().count();
    let snippet_lines = snippet.lines().count();

    // If the snippet is at least 50% the size of the original, consider it full-file
    if orig_lines > 0 && snippet_lines as f64 / orig_lines as f64 > 0.5 {
        return true;
    }

    // If the snippet has structural markers at the top (imports, module declarations)
    let first_lines: String = snippet.lines().take(5).collect::<Vec<_>>().join(" ");
    let has_structure = first_lines.contains("use ")
        || first_lines.contains("import ")
        || first_lines.contains("from ")
        || first_lines.contains("#include")
        || first_lines.contains("package ")
        || first_lines.contains("module ")
        || first_lines.contains("#![")
        || first_lines.contains("//!");

    if has_structure && snippet_lines > 10 {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_ellipsis_patterns() {
        assert!(is_ellipsis_line("    // ... existing code ..."));
        assert!(is_ellipsis_line("    # ... existing code ..."));
        assert!(is_ellipsis_line("    // ..."));
        assert!(is_ellipsis_line("    # ..."));
        assert!(is_ellipsis_line("    /* ... */"));
        assert!(is_ellipsis_line("    // rest of the code"));
        assert!(is_ellipsis_line("    // unchanged"));
        assert!(!is_ellipsis_line("    let x = 5;"));
        assert!(!is_ellipsis_line(""));
        assert!(!is_ellipsis_line("    // This is a normal comment"));
    }

    #[test]
    fn snippet_with_ellipsis_detected() {
        let snippet = "fn main() {\n    // ... existing code ...\n    println!(\"new line\");\n}";
        assert!(has_ellipsis_markers(snippet));
    }

    #[test]
    fn snippet_without_ellipsis() {
        let snippet = "fn main() {\n    println!(\"hello\");\n}";
        assert!(!has_ellipsis_markers(snippet));
    }

    #[test]
    fn ellipsis_merge_preserves_middle() {
        let original = "\
fn main() {
    let x = 1;
    let y = 2;
    let z = 3;
    println!(\"{}\", x + y + z);
}";

        let snippet = "\
fn main() {
    let x = 10;
    // ... existing code ...
    println!(\"{}\", x + y + z);
}";

        let result = apply_edit(original, snippet).unwrap();
        assert_eq!(result.strategy, Strategy::EllipsisMerge);
        assert!(result.content.contains("let x = 10;"));
        assert!(result.content.contains("let y = 2;"));
        assert!(result.content.contains("let z = 3;"));
        assert!(result.content.contains("println!"));
    }

    #[test]
    fn ellipsis_merge_preserves_end() {
        let original = "\
use std::io;

fn main() {
    let x = 1;
    println!(\"{x}\");
}

fn helper() {
    // do stuff
}";

        let snippet = "\
use std::io;

fn main() {
    let x = 42;
    println!(\"{x}\");
}

// ... existing code ...";

        let result = apply_edit(original, snippet).unwrap();
        assert_eq!(result.strategy, Strategy::EllipsisMerge);
        assert!(result.content.contains("let x = 42;"));
        assert!(result.content.contains("fn helper()"));
        assert!(result.content.contains("// do stuff"));
    }

    #[test]
    fn ellipsis_merge_preserves_beginning() {
        let original = "\
use std::io;
use std::fs;

fn helper() {
    // helper stuff
}

fn main() {
    let x = 1;
}";

        let snippet = "\
// ... existing code ...

fn main() {
    let x = 99;
}";

        let result = apply_edit(original, snippet).unwrap();
        assert_eq!(result.strategy, Strategy::EllipsisMerge);
        assert!(result.content.contains("use std::io;"));
        assert!(result.content.contains("use std::fs;"));
        assert!(result.content.contains("fn helper()"));
        assert!(result.content.contains("let x = 99;"));
    }

    #[test]
    fn fuzzy_match_replaces_region() {
        let original = "\
fn main() {
    let x = 1;
    let y = 2;
    println!(\"{}\", x + y);
}

fn other() {
    // stuff
}";

        // Snippet replaces the main function with slightly different whitespace
        let snippet = "\
fn main() {
    let x = 100;
    let y = 200;
    let z = 300;
    println!(\"{}\", x + y + z);
}";

        let result = apply_edit(original, snippet).unwrap();
        assert_eq!(result.strategy, Strategy::FuzzyMatch);
        assert!(result.content.contains("let x = 100;"));
        assert!(result.content.contains("let z = 300;"));
        assert!(result.content.contains("fn other()"));
    }

    #[test]
    fn full_file_replacement() {
        let original = "\
fn main() {
    println!(\"old\");
}";

        // Snippet with imports looks like a full file
        let snippet = "\
use std::io;

fn main() {
    println!(\"new\");
}

fn added_function() {
    // new stuff
}

fn another_function() {
    // more new stuff
}";

        let result = apply_edit(original, snippet).unwrap();
        assert_eq!(result.strategy, Strategy::FullFile);
        assert!(result.content.contains("use std::io;"));
        assert!(result.content.contains("fn added_function()"));
    }

    #[test]
    fn split_segments_at_ellipsis() {
        let lines = vec![
            "fn main() {",
            "    let x = 1;",
            "    // ... existing code ...",
            "    println!(\"done\");",
            "}",
        ];
        let segments = split_at_ellipsis(&lines);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0], vec!["fn main() {", "    let x = 1;"]);
        assert_eq!(segments[1], vec!["    println!(\"done\");", "}"]);
    }

    #[test]
    fn no_strategy_matches_garbage() {
        let original = "fn main() {\n    println!(\"hello\");\n}";
        let snippet = "xy"; // Too short for fuzzy, no ellipsis, not full file
        let result = apply_edit(original, snippet);
        assert!(result.is_err());
    }
}
