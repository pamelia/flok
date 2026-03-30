//! Layer 2: Conversation history compression.
//!
//! Compresses `tool_result` content blocks in the conversation history before
//! sending to the LLM provider. Includes JSON minification, TOON encoding
//! for JSON arrays, and a deduplication cache.

use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Mutex;

/// Compress a tool result's content string.
///
/// Classification order: JSON array → JSON object → CLI output → passthrough.
/// Error results are NEVER compressed (pass `is_error: true` to skip).
pub fn compress_tool_result(content: &str, is_error: bool) -> String {
    // Safety invariant: never compress errors
    if is_error {
        return content.to_string();
    }

    // Try JSON classification
    if let Some(compressed) = try_compress_json(content) {
        return compressed;
    }

    // Already compressed or not JSON — return as-is
    content.to_string()
}

/// Try to compress content as JSON.
fn try_compress_json(content: &str) -> Option<String> {
    let trimmed = content.trim();

    // Try parsing as JSON
    let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;

    match &value {
        // JSON array of objects → TOON encoding
        serde_json::Value::Array(arr) if arr.len() >= 3 => {
            if let Some(toon) = toon_encode(arr) {
                return Some(toon);
            }
            // Fall through to minify
            Some(serde_json::to_string(&value).unwrap_or_else(|_| content.to_string()))
        }
        // JSON object or other → minify
        _ => Some(serde_json::to_string(&value).unwrap_or_else(|_| content.to_string())),
    }
}

/// TOON encoding: convert a JSON array of uniform objects into a compact
/// columnar format.
///
/// ```text
/// Input:  [{"name":"a","size":1},{"name":"b","size":2}]
/// Output: 2 name,size\na,1\nb,2
/// ```
///
/// Returns `None` if the array is not uniform (objects have different keys).
fn toon_encode(arr: &[serde_json::Value]) -> Option<String> {
    // All elements must be objects
    let objects: Vec<&serde_json::Map<String, serde_json::Value>> =
        arr.iter().filter_map(serde_json::Value::as_object).collect();

    if objects.len() != arr.len() {
        return None; // Not all elements are objects
    }

    // All objects must have the same keys
    let first_keys: Vec<&String> = objects.first()?.keys().collect();
    for obj in &objects[1..] {
        let keys: Vec<&String> = obj.keys().collect();
        if keys != first_keys {
            return None; // Different keys
        }
    }

    if first_keys.is_empty() {
        return None;
    }

    // Build TOON output
    let mut result = String::new();

    // Header: count + column names
    let _ = writeln!(
        result,
        "{} {}",
        objects.len(),
        first_keys.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(",")
    );

    // Rows: values only
    for obj in &objects {
        let values: Vec<String> = first_keys
            .iter()
            .map(|key| value_to_string(obj.get(*key).unwrap_or(&serde_json::Value::Null)))
            .collect();
        result.push_str(&values.join(","));
        result.push('\n');
    }

    result.truncate(result.trim_end().len());
    Some(result)
}

/// Convert a JSON value to a compact string representation for TOON.
fn value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "null".to_string(),
        // Nested objects/arrays: minified JSON
        other => serde_json::to_string(other).unwrap_or_else(|_| "?".to_string()),
    }
}

/// A deduplication cache for tool results.
///
/// Keyed by blake3 hash of the content. Stores the turn number where the
/// content was first seen and the associated file path (if applicable).
pub struct DedupCache {
    inner: Mutex<HashMap<String, DedupEntry>>,
    max_size: usize,
}

struct DedupEntry {
    turn: u32,
    path: Option<String>,
}

impl DedupCache {
    /// Create a new dedup cache with the given maximum size.
    pub fn new(max_size: usize) -> Self {
        Self { inner: Mutex::new(HashMap::new()), max_size }
    }

    /// Check if content was seen before. Returns a placeholder string if so.
    /// Otherwise, records the content and returns `None`.
    pub fn check_and_record(&self, content: &str, turn: u32, path: Option<&str>) -> Option<String> {
        let hash = blake3::hash(content.as_bytes()).to_hex().to_string();

        let mut cache = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

        if let Some(entry) = cache.get(&hash) {
            let path_desc = entry.path.as_deref().map_or(String::new(), |p| format!(" of {p}"));
            return Some(format!(
                "[content identical to previous read{path_desc} at turn {}]",
                entry.turn
            ));
        }

        // Evict oldest if at capacity
        if cache.len() >= self.max_size {
            // Simple eviction: remove any one entry (HashMap doesn't track order)
            if let Some(key) = cache.keys().next().cloned() {
                cache.remove(&key);
            }
        }

        cache.insert(hash, DedupEntry { turn, path: path.map(String::from) });

        None
    }

    /// Clear the cache.
    pub fn clear(&self) {
        let mut cache = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compress_error_is_passthrough() {
        let content = "Error: something broke\n  at line 42\n  stack trace...";
        let result = compress_tool_result(content, true);
        assert_eq!(result, content);
    }

    #[test]
    fn compress_json_object_minifies() {
        let content = r#"{
  "name": "test",
  "value": 42,
  "nested": {
    "a": 1,
    "b": 2
  }
}"#;
        let result = compress_tool_result(content, false);
        assert!(!result.contains('\n'));
        assert!(result.contains("\"name\":\"test\""));
    }

    #[test]
    fn compress_json_array_uses_toon() {
        let content = r#"[
  {"name": "auth.rs", "size": 1204, "modified": "2026-03-28"},
  {"name": "main.rs", "size": 856, "modified": "2026-03-27"},
  {"name": "config.rs", "size": 432, "modified": "2026-03-26"}
]"#;
        let result = compress_tool_result(content, false);
        // Should start with "3 " and contain column names
        assert!(result.starts_with("3 "), "should start with count: {result}");
        // Keys are sorted by serde_json, so order is: modified, name, size
        assert!(result.contains("auth.rs"), "should contain file name: {result}");
        assert!(result.contains("1204"), "should contain size: {result}");
        assert!(result.contains("2026-03-28"), "should contain date: {result}");
    }

    #[test]
    fn toon_encode_non_uniform_falls_back() {
        let content = r#"[
  {"name": "a", "size": 1},
  {"different_key": "b", "size": 2}
]"#;
        let result = compress_tool_result(content, false);
        // Should minify, not TOON
        assert!(!result.starts_with("2 "));
    }

    #[test]
    fn toon_encode_small_array_skipped() {
        let content = r#"[{"a": 1}, {"a": 2}]"#;
        let result = compress_tool_result(content, false);
        // Array of 2 — too small for TOON, should minify
        assert!(!result.starts_with("2 "));
    }

    #[test]
    fn compress_non_json_passthrough() {
        let content = "this is just plain text\nwith multiple lines";
        let result = compress_tool_result(content, false);
        assert_eq!(result, content);
    }

    #[test]
    fn dedup_cache_detects_duplicates() {
        let cache = DedupCache::new(256);
        let content = "fn main() { println!(\"hello\"); }";

        // First time: no match
        assert!(cache.check_and_record(content, 1, Some("main.rs")).is_none());

        // Second time: should return placeholder
        let result = cache.check_and_record(content, 3, Some("main.rs"));
        assert!(result.is_some());
        assert!(result.unwrap().contains("turn 1"));
    }

    #[test]
    fn dedup_cache_different_content_no_match() {
        let cache = DedupCache::new(256);

        cache.check_and_record("content A", 1, None);
        assert!(cache.check_and_record("content B", 2, None).is_none());
    }
}
