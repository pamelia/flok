//! Line-level fuzzy matching utilities.

/// Compute a similarity score between two strings (0.0 to 1.0).
///
/// Uses a simple character-level comparison after trimming whitespace.
/// Returns 1.0 for exact match, 0.0 for completely different strings.
pub(crate) fn line_similarity(a: &str, b: &str) -> f64 {
    let a = a.trim();
    let b = b.trim();

    if a == b {
        return 1.0;
    }
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    // Use longest common subsequence ratio
    let lcs_len = lcs_length(a.as_bytes(), b.as_bytes());
    let max_len = a.len().max(b.len());
    lcs_len as f64 / max_len as f64
}

/// Compute the length of the longest common subsequence.
fn lcs_length(a: &[u8], b: &[u8]) -> usize {
    // Optimize for short strings with a single-row DP
    let mut prev = vec![0usize; b.len() + 1];
    let mut curr = vec![0usize; b.len() + 1];

    for &ac in a {
        for (j, &bc) in b.iter().enumerate() {
            curr[j + 1] = if ac == bc { prev[j] + 1 } else { prev[j + 1].max(curr[j]) };
        }
        std::mem::swap(&mut prev, &mut curr);
        curr.fill(0);
    }

    *prev.iter().max().unwrap_or(&0)
}

/// Find the best matching position for a sequence of `needle` lines
/// within `haystack` lines. Returns `(start_index, score)`.
///
/// The score is the average line similarity across the matched region.
/// Only considers positions where at least one line matches well.
pub(crate) fn find_best_match(
    haystack: &[&str],
    needle: &[&str],
    min_score: f64,
) -> Option<(usize, f64)> {
    if needle.is_empty() || haystack.is_empty() || needle.len() > haystack.len() {
        return None;
    }

    let mut best_pos = 0;
    let mut best_score = 0.0;

    for start in 0..=(haystack.len() - needle.len()) {
        let score = region_similarity(haystack, start, needle);
        if score > best_score {
            best_score = score;
            best_pos = start;
        }
    }

    if best_score >= min_score {
        Some((best_pos, best_score))
    } else {
        None
    }
}

/// Compute similarity between a region of haystack starting at `offset`
/// and the needle lines.
fn region_similarity(haystack: &[&str], offset: usize, needle: &[&str]) -> f64 {
    let mut total = 0.0;
    for (i, &n) in needle.iter().enumerate() {
        if offset + i < haystack.len() {
            total += line_similarity(haystack[offset + i], n);
        }
    }
    total / needle.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_returns_one() {
        assert!((line_similarity("hello world", "hello world") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn whitespace_trimmed_match() {
        assert!((line_similarity("  hello  ", "hello") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn completely_different() {
        let score = line_similarity("aaaa", "zzzz");
        assert!(score < 0.1);
    }

    #[test]
    fn partial_similarity() {
        let score = line_similarity("fn hello_world()", "fn hello_world(x: i32)");
        assert!(score > 0.7);
    }

    #[test]
    fn find_match_exact_region() {
        let haystack: Vec<&str> = vec!["line1", "line2", "line3", "line4", "line5"];
        let needle: Vec<&str> = vec!["line2", "line3"];
        let result = find_best_match(&haystack, &needle, 0.8);
        assert!(result.is_some());
        let (pos, score) = result.unwrap();
        assert_eq!(pos, 1);
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn find_match_no_match() {
        let haystack: Vec<&str> = vec!["aaa", "bbb", "ccc"];
        let needle: Vec<&str> = vec!["xxx", "yyy"];
        let result = find_best_match(&haystack, &needle, 0.8);
        assert!(result.is_none());
    }
}
