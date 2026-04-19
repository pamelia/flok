#[derive(Debug, Default, Clone)]
pub(crate) struct InputHistory {
    entries: Vec<String>,  // most recent at end
    cursor: Option<usize>, // index into entries; None = not browsing
    draft: Option<String>, // saved draft when browsing started
}

impl InputHistory {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Push a submitted entry to history. Clears cursor/draft (end-of-history).
    pub(crate) fn push(&mut self, entry: String) {
        if entry.is_empty() {
            return;
        }
        // deduplicate: if last entry equals this, skip
        if self.entries.last().is_some_and(|e| e == &entry) {
            self.cursor = None;
            self.draft = None;
            return;
        }
        self.entries.push(entry);
        self.cursor = None;
        self.draft = None;
    }

    /// Move cursor up (to older entry). If at bottom (None), save `current_text` as draft.
    /// Returns the text to display in composer, or None if no change (already at oldest).
    pub(crate) fn recall_prev(&mut self, current_text: &str) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        let new_idx = match self.cursor {
            None => {
                self.draft = Some(current_text.to_string());
                self.entries.len() - 1
            }
            Some(0) => return None, // at oldest, nothing to do
            Some(i) => i - 1,
        };
        self.cursor = Some(new_idx);
        Some(self.entries[new_idx].clone())
    }

    /// Move cursor down (to newer). If reaches past end, restore draft.
    /// Returns text to display, or None if no change.
    pub(crate) fn recall_next(&mut self, _current_text: &str) -> Option<String> {
        match self.cursor {
            None => None,
            Some(i) if i + 1 < self.entries.len() => {
                self.cursor = Some(i + 1);
                Some(self.entries[i + 1].clone())
            }
            Some(_) => {
                self.cursor = None;
                // restore draft (or empty string if none was saved)
                Some(self.draft.take().unwrap_or_default())
            }
        }
    }

    pub(crate) fn is_browsing(&self) -> bool {
        self.cursor.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_dedupes_consecutive_duplicates() {
        let mut h = InputHistory::new();
        h.push("hello".to_string());
        h.push("hello".to_string());
        h.push("world".to_string());
        h.push("world".to_string());
        // Only two distinct entries stored.
        assert_eq!(h.recall_prev("").as_deref(), Some("world"));
        assert_eq!(h.recall_prev("").as_deref(), Some("hello"));
        assert_eq!(h.recall_prev("").as_deref(), None);
    }

    #[test]
    fn push_ignores_empty() {
        let mut h = InputHistory::new();
        h.push(String::new());
        assert_eq!(h.recall_prev(""), None);
        assert!(!h.is_browsing());
    }

    #[test]
    fn recall_prev_on_empty_returns_none() {
        let mut h = InputHistory::new();
        assert_eq!(h.recall_prev("draft"), None);
        assert!(!h.is_browsing());
    }

    #[test]
    fn recall_prev_from_bottom_saves_draft_and_returns_last() {
        let mut h = InputHistory::new();
        h.push("first".to_string());
        h.push("second".to_string());
        let out = h.recall_prev("partial-draft");
        assert_eq!(out.as_deref(), Some("second"));
        assert!(h.is_browsing());
        // Draft should be restored when we come back down past newest.
        let restored = h.recall_next("second");
        assert_eq!(restored.as_deref(), Some("partial-draft"));
        assert!(!h.is_browsing());
    }

    #[test]
    fn recall_prev_at_oldest_returns_none() {
        let mut h = InputHistory::new();
        h.push("a".to_string());
        h.push("b".to_string());
        assert_eq!(h.recall_prev("").as_deref(), Some("b"));
        assert_eq!(h.recall_prev("b").as_deref(), Some("a"));
        // At oldest now; further recall_prev must return None and not change state.
        assert_eq!(h.recall_prev("a"), None);
        assert!(h.is_browsing());
    }

    #[test]
    fn recall_next_returns_newer_entries() {
        let mut h = InputHistory::new();
        h.push("a".to_string());
        h.push("b".to_string());
        h.push("c".to_string());
        // Walk all the way up.
        assert_eq!(h.recall_prev("").as_deref(), Some("c"));
        assert_eq!(h.recall_prev("c").as_deref(), Some("b"));
        assert_eq!(h.recall_prev("b").as_deref(), Some("a"));
        // Walk back down through newer entries.
        assert_eq!(h.recall_next("a").as_deref(), Some("b"));
        assert_eq!(h.recall_next("b").as_deref(), Some("c"));
    }

    #[test]
    fn recall_next_past_newest_restores_draft() {
        let mut h = InputHistory::new();
        h.push("one".to_string());
        h.push("two".to_string());
        assert_eq!(h.recall_prev("my-draft").as_deref(), Some("two"));
        // Stepping past newest returns the saved draft and ends browsing.
        let restored = h.recall_next("two");
        assert_eq!(restored.as_deref(), Some("my-draft"));
        assert!(!h.is_browsing());
        // Further recall_next with no browsing returns None.
        assert_eq!(h.recall_next("my-draft"), None);
    }

    #[test]
    fn recall_cycle_up_then_down_restores_draft() {
        let mut h = InputHistory::new();
        h.push("alpha".to_string());
        h.push("beta".to_string());
        h.push("gamma".to_string());
        // Save draft and cycle up.
        assert_eq!(h.recall_prev("draft!").as_deref(), Some("gamma"));
        assert_eq!(h.recall_prev("gamma").as_deref(), Some("beta"));
        assert_eq!(h.recall_prev("beta").as_deref(), Some("alpha"));
        // Cycle back down.
        assert_eq!(h.recall_next("alpha").as_deref(), Some("beta"));
        assert_eq!(h.recall_next("beta").as_deref(), Some("gamma"));
        // One more down: draft must be restored.
        assert_eq!(h.recall_next("gamma").as_deref(), Some("draft!"));
        assert!(!h.is_browsing());
    }

    #[test]
    fn push_after_browsing_resets_state() {
        let mut h = InputHistory::new();
        h.push("a".to_string());
        h.push("b".to_string());
        assert_eq!(h.recall_prev("draft").as_deref(), Some("b"));
        assert!(h.is_browsing());
        h.push("c".to_string());
        assert!(!h.is_browsing());
        // Recall order reflects new entry.
        assert_eq!(h.recall_prev("").as_deref(), Some("c"));
        assert_eq!(h.recall_prev("c").as_deref(), Some("b"));
        assert_eq!(h.recall_prev("b").as_deref(), Some("a"));
        assert_eq!(h.recall_prev("a"), None);
    }

    #[test]
    fn empty_draft_restored_as_empty_string() {
        let mut h = InputHistory::new();
        h.push("only".to_string());
        // Empty current text when browsing starts -> draft is empty string.
        assert_eq!(h.recall_prev("").as_deref(), Some("only"));
        let restored = h.recall_next("only");
        assert_eq!(restored.as_deref(), Some(""));
        assert!(!h.is_browsing());
    }
}
