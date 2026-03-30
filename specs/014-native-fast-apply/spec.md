# Feature Specification: Native Fast Apply, Smart Grep & Context Compaction

**Feature Branch**: `014-native-fast-apply`
**Created**: 2026-03-28
**Status**: Draft

## User Scenarios & Testing

### User Story 1 - Agent Applies Lazy Edit Snippets Reliably (Priority: P0)
**Why this priority**: Every frontier model naturally outputs lazy edit snippets with `// ... existing code ...` markers. Current search-and-replace approaches benchmark at 84-96% accuracy with 2-3.5x retry turns on failures. A tree-sitter semantic merge eliminates this failure mode.
**Acceptance Scenarios**:
1. **Given** the LLM outputs an edit snippet with `// ... existing code ...` markers, **When** the fast apply engine processes it, **Then** the edit is resolved against the file's AST and applied correctly.
2. **Given** a non-contiguous edit (changes in two separate functions), **When** fast apply processes it, **Then** both edits are applied correctly without disturbing the code between them.
3. **Given** the LLM moves a code block (e.g., reorders functions), **When** fast apply processes it, **Then** the move is detected and applied correctly.
4. **Given** fast apply fails to resolve an edit (ambiguous markers), **When** fallback triggers, **Then** it degrades to search-and-replace, then to full-file write, logging the fallback reason.

### User Story 2 - Agent Searches Code by Semantic Structure (Priority: P0)
**Why this priority**: Grep dumps into the agent's context cause "context rot" -- thousands of tokens the model has to reason over. AST-aware search returns precise, symbol-level results.
**Acceptance Scenarios**:
1. **Given** the agent searches for "where is authentication handled", **When** smart grep runs, **Then** it returns symbol-level results (function names, struct names, impl blocks) rather than raw line matches.
2. **Given** the agent runs 8 parallel searches, **When** they execute, **Then** all complete within 200ms total (parallel tokio tasks).
3. **Given** a search for a function name, **When** smart grep runs, **Then** it distinguishes between definitions, call sites, and string occurrences.

### User Story 3 - Context Stays Clean During Long Sessions (Priority: P0)
**Why this priority**: Most context bloat comes from tool responses (file reads, grep output, shell results), not from model generation. Uncompacted sessions degrade model performance as context fills up.
**Acceptance Scenarios**:
1. **Given** a session at 60% context window usage, **When** tier-1 compaction triggers, **Then** tool responses are compressed (deduplicated, summarized) while conversation reasoning is preserved intact.
2. **Given** a session at 80% context window usage, **When** tier-2 compaction triggers, **Then** a structured summary (Goal/Progress/TODOs/Constraints) replaces older messages.
3. **Given** a session at 95% context window usage, **When** emergency compaction triggers, **Then** old tool outputs are truncated programmatically (no LLM call) within 100ms.
4. **Given** a compressed tool response, **When** the agent needs the original content, **Then** it can re-read the file (the compression doesn't destroy the source).

### Edge Cases
- LLM produces malformed lazy edit (missing markers): fall back to search-and-replace
- File has been modified between LLM seeing it and fast apply running: detect via content hash, re-read and retry
- Tree-sitter doesn't support the file's language: fall back to text-based merge (line-level fuzzy matching)
- Smart grep on binary files: skip, return "binary file" indicator
- Compaction of tool response that contains critical error messages: preserve error-tagged content
- Very large file (>100K lines): chunk AST processing to avoid memory spikes
- File with syntax errors (can't parse AST): fall back to text-based merge
- Multiple agents compacting the same session: serialize via session write queue

## Requirements

### Functional Requirements

#### Fast Apply (Tree-Sitter Semantic Merge)

- **FR-001**: Flok MUST support a `fast_apply` tool that accepts lazy edit snippets with `// ... existing code ...` (or language-equivalent) markers and applies them to files via AST-aware merge.
- **FR-002**: The fast apply engine MUST use tree-sitter for parsing:
  - Parse both the original file and the edit snippet into ASTs
  - Identify matching nodes (functions, structs, impl blocks, classes, methods)
  - Apply changes at the AST node level, preserving unmodified code
- **FR-003**: Fast apply MUST support the following edit patterns:
  - **Insertion**: New code added within a function or scope
  - **Replacement**: Existing code block replaced with new content
  - **Deletion**: Code block removed (replaced with nothing)
  - **Move**: Code block moved to a different position
  - **Non-contiguous edits**: Multiple changes in a single snippet separated by `// ... existing code ...`
- **FR-004**: Fast apply MUST have a fallback chain:
  1. AST-aware merge (tree-sitter)
  2. Line-level fuzzy match (find closest matching lines, apply edit)
  3. Search-and-replace (existing `edit` tool behavior)
  4. Full file write (last resort, only if agent provides complete file)
- **FR-005**: Fast apply MUST report which strategy was used and log fallback reasons.
- **FR-006**: Fast apply MUST support common languages via bundled tree-sitter grammars:

  | Language | Grammar |
  |----------|---------|
  | Rust | `tree-sitter-rust` |
  | TypeScript/JavaScript | `tree-sitter-typescript`, `tree-sitter-javascript` |
  | Python | `tree-sitter-python` |
  | Go | `tree-sitter-go` |
  | Java | `tree-sitter-java` |
  | C/C++ | `tree-sitter-c`, `tree-sitter-cpp` |
  | Ruby | `tree-sitter-ruby` |
  | Shell/Bash | `tree-sitter-bash` |
  | JSON/YAML/TOML | `tree-sitter-json`, `tree-sitter-yaml`, `tree-sitter-toml` |
  | Markdown | `tree-sitter-markdown` |
  | HTML/CSS | `tree-sitter-html`, `tree-sitter-css` |
  | SQL | `tree-sitter-sql` |

- **FR-007**: Fast apply MUST complete in < 50ms for files < 10K lines.
- **FR-008**: The existing `edit` tool (search-and-replace) MUST remain available as a direct option. `fast_apply` is an additional tool, not a replacement.

#### Smart Grep (AST-Aware Search)

- **FR-009**: Flok MUST provide an enhanced `smart_grep` tool that combines ripgrep speed with tree-sitter AST awareness.
- **FR-010**: Smart grep MUST support query types:
  - **Text search**: Standard regex search (ripgrep-backed)
  - **Symbol search**: Find definitions of functions, classes, structs, interfaces by name
  - **Reference search**: Find call sites and usages of a symbol
  - **Semantic search**: Match against AST node types (e.g., "all function definitions matching `auth*`")
- **FR-011**: Smart grep MUST run searches in separate tokio tasks, keeping the agent's main context clean.
- **FR-012**: Smart grep MUST support parallel execution: up to 8 concurrent searches per agent turn.
- **FR-013**: Smart grep results MUST be formatted as symbol-level results:
  ```
  src/auth/middleware.rs:
    fn verify_token (line 42) [definition]
    fn refresh_token (line 89) [definition]
  src/routes/users.rs:
    verify_token(req.token()) (line 15) [call]
    verify_token(admin_token) (line 67) [call]
  ```
- **FR-014**: Smart grep MUST complete in < 200ms for projects with < 100K files.

#### Context Compaction (Tool-Response-Aware Compression)

- **FR-015**: Flok MUST implement tiered context compaction:

  | Tier | Trigger | Strategy | LLM Call? | Target |
  |------|---------|----------|-----------|--------|
  | T1 | 60% context | Compress tool responses | No | Remove duplicate content, truncate large outputs, extract key info |
  | T2 | 80% context | Structured summary | Yes (utility model) | Goal/Progress/TODOs/Constraints format |
  | T3 | 95% context | Emergency truncation | No | Drop old tool outputs, keep last N messages |

- **FR-016**: T1 compaction (tool response compression) MUST:
  - Deduplicate repeated file content (if the same file is read multiple times, keep only the latest)
  - Truncate large tool outputs to their first 500 chars + last 200 chars with a summary line
  - Preserve error messages and diagnostic output intact
  - Preserve all model-generated text (reasoning, explanations) intact
  - Run in-process with zero LLM cost

- **FR-017**: T2 compaction (structured summary) MUST:
  - Summarize using the utility-tier model (cheap)
  - Output format: `Goal: ... | Progress: ... | TODOs: ... | Constraints: ...`
  - Preserve the last ~40K tokens of recent context intact
  - Store the summary as a `CompactionPart` in the session

- **FR-018**: T3 compaction (emergency) MUST:
  - Run without any LLM call (pure programmatic truncation)
  - Drop all tool outputs older than the last 5 messages
  - Truncate remaining tool outputs to 200 chars
  - Drop all reasoning parts older than the last 3 messages
  - Complete in < 100ms

- **FR-019**: All compaction tiers MUST preserve:
  - The system prompt (never compacted)
  - The last user message (always kept intact)
  - Tool call state machines (pending/running tools)
  - Active plan context (if a plan is executing)

- **FR-020**: Compaction thresholds MUST be configurable:
  ```toml
  [compaction]
  t1_threshold = 0.60   # 60% context window
  t2_threshold = 0.80   # 80% context window
  t3_threshold = 0.95   # 95% context window
  t2_preserve_tokens = 40000  # Keep last 40K tokens in T2
  ```

### Key Entities

```rust
/// Fast Apply
pub struct FastApplyEngine {
    parsers: HashMap<String, tree_sitter::Parser>,  // Language -> parser
}

pub struct ApplyResult {
    pub strategy: ApplyStrategy,
    pub success: bool,
    pub content: String,         // The resulting file content
    pub changed_ranges: Vec<Range<usize>>,
}

pub enum ApplyStrategy {
    AstMerge,           // Tree-sitter AST-level merge
    LineFuzzyMatch,     // Line-level fuzzy matching
    SearchAndReplace,   // Exact string search-and-replace
    FullFileWrite,      // Complete file replacement
}

/// Smart Grep
pub struct SmartGrepEngine {
    parsers: HashMap<String, tree_sitter::Parser>,
}

pub struct SymbolResult {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
    pub symbol_name: String,
    pub kind: SymbolKind,       // Definition, Call, Reference, Import
    pub context: String,        // Surrounding line for display
}

pub enum SymbolKind {
    Definition,
    Call,
    Reference,
    Import,
}

/// Context Compaction
pub struct CompactionEngine {
    utility_model: String,      // Model for T2 summaries
}

pub enum CompactionTier { T1, T2, T3 }

pub struct CompactionResult {
    pub tier: CompactionTier,
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub tokens_freed: u64,
    pub messages_compacted: usize,
}
```

## Design

### Overview

This spec covers three tightly related subsystems that address the biggest hidden cost multipliers in agent workflows: (1) **Fast Apply** eliminates the 10-15x token overhead of full-file rewrites, (2) **Smart Grep** eliminates context rot from raw grep dumps, and (3) **Context Compaction** prevents session degradation as context fills up. All three run natively in the Rust binary -- no external APIs, no network latency, no cost.

### Detailed Design

#### Fast Apply: AST-Aware Merge

The fast apply engine processes lazy edit snippets like:

```python
def process_user(user):
    # ... existing code ...
    
    # NEW: Add validation
    if not user.is_valid():
        raise ValueError("Invalid user")
    
    # ... existing code ...
```

**Algorithm:**

```rust
impl FastApplyEngine {
    pub fn apply(
        &self,
        original: &str,
        edit_snippet: &str,
        language: &str,
    ) -> Result<ApplyResult> {
        // 1. Try AST merge
        if let Some(parser) = self.parsers.get(language) {
            match self.ast_merge(parser, original, edit_snippet) {
                Ok(result) => return Ok(result),
                Err(e) => tracing::debug!("AST merge failed: {}, trying fallback", e),
            }
        }

        // 2. Try line-level fuzzy match
        match self.line_fuzzy_match(original, edit_snippet) {
            Ok(result) => return Ok(result),
            Err(e) => tracing::debug!("Line fuzzy match failed: {}, trying fallback", e),
        }

        // 3. Fall back to search-and-replace
        // (only works for simple, exact-match edits)
        Err(anyhow!("All fast apply strategies failed"))
    }

    fn ast_merge(
        &self,
        parser: &mut tree_sitter::Parser,
        original: &str,
        edit_snippet: &str,
    ) -> Result<ApplyResult> {
        let original_tree = parser.parse(original, None)
            .ok_or_else(|| anyhow!("Failed to parse original file"))?;

        // Identify "existing code" markers in the edit snippet
        let markers = find_existing_code_markers(edit_snippet);

        // Split edit snippet into segments (new code between markers)
        let segments = split_by_markers(edit_snippet, &markers);

        // For each segment, find the matching AST node in the original tree
        // and apply the replacement
        let mut result = original.to_string();
        let mut offset: isize = 0;

        for segment in &segments {
            if segment.is_new_code {
                // Find the anchor point in the AST
                let anchor = find_ast_anchor(
                    &original_tree,
                    original,
                    &segment.preceding_context,
                    &segment.following_context,
                )?;

                // Apply the edit at the anchor point
                let (start, end) = (
                    (anchor.start as isize + offset) as usize,
                    (anchor.end as isize + offset) as usize,
                );
                let new_len = segment.content.len();
                let old_len = end - start;
                result.replace_range(start..end, &segment.content);
                offset += new_len as isize - old_len as isize;
            }
        }

        Ok(ApplyResult {
            strategy: ApplyStrategy::AstMerge,
            success: true,
            content: result,
            changed_ranges: vec![],
        })
    }
}
```

#### Smart Grep: Symbol-Level Search

```rust
impl SmartGrepEngine {
    pub async fn search(
        &self,
        project_root: &Path,
        query: &str,
        search_type: SearchType,
    ) -> Result<Vec<SymbolResult>> {
        match search_type {
            SearchType::Text => {
                // Delegate to ripgrep for raw speed
                self.ripgrep_search(project_root, query).await
            }
            SearchType::Symbol => {
                // Parse files with tree-sitter, find symbol definitions
                self.symbol_search(project_root, query).await
            }
            SearchType::Semantic => {
                // Combine ripgrep for candidate files + tree-sitter for classification
                self.semantic_search(project_root, query).await
            }
        }
    }

    async fn semantic_search(
        &self,
        root: &Path,
        query: &str,
    ) -> Result<Vec<SymbolResult>> {
        // 1. Fast text search to find candidate files
        let candidates = self.ripgrep_search(root, query).await?;

        // 2. For each candidate file, parse AST and classify matches
        let results: Vec<_> = futures::future::join_all(
            candidates.iter().map(|file| {
                let root = root.to_path_buf();
                async move {
                    let content = tokio::fs::read_to_string(
                        root.join(&file.path)
                    ).await?;

                    let language = detect_language(&file.path);
                    let parser = self.get_parser(&language)?;
                    let tree = parser.parse(&content, None)?;

                    // Walk AST, classify each match as definition/call/reference
                    classify_matches(&tree, &content, query, &file.path)
                }
            })
        ).await;

        Ok(results.into_iter().flatten().flatten().collect())
    }
}
```

#### Context Compaction: Tiered Compression

```rust
impl CompactionEngine {
    pub async fn compact(
        &self,
        state: &AppState,
        session_id: SessionID,
        context_usage: f64,
        config: &CompactionConfig,
    ) -> Result<CompactionResult> {
        let tier = if context_usage >= config.t3_threshold {
            CompactionTier::T3
        } else if context_usage >= config.t2_threshold {
            CompactionTier::T2
        } else if context_usage >= config.t1_threshold {
            CompactionTier::T1
        } else {
            return Err(anyhow!("No compaction needed at {}% usage", context_usage * 100.0));
        };

        match tier {
            CompactionTier::T1 => self.compact_tool_responses(state, session_id).await,
            CompactionTier::T2 => self.compact_structured_summary(state, session_id, config).await,
            CompactionTier::T3 => self.compact_emergency(state, session_id).await,
        }
    }

    /// T1: Compress tool responses only (no LLM call)
    async fn compact_tool_responses(
        &self,
        state: &AppState,
        session_id: SessionID,
    ) -> Result<CompactionResult> {
        let messages = state.db.messages_for_session(session_id).await?;
        let mut tokens_freed = 0u64;

        for message in &messages {
            for part in &message.parts {
                if let Part::ToolCall(tc) = part {
                    if let Some(result) = &tc.result {
                        // Deduplicate: if the same file was read multiple times,
                        // keep only the latest read
                        if tc.tool_name == "read" {
                            if is_duplicate_read(tc, &messages) {
                                let original_tokens = estimate_tokens(result);
                                let compressed = "[Previous read of this file - see latest read below]";
                                // Update the part with compressed content
                                tokens_freed += original_tokens - estimate_tokens(compressed);
                            }
                        }

                        // Truncate large outputs
                        if result.len() > 2000 {
                            let truncated = format!(
                                "{}...\n[truncated {} chars]\n...{}",
                                &result[..500],
                                result.len() - 700,
                                &result[result.len()-200..]
                            );
                            let original_tokens = estimate_tokens(result);
                            tokens_freed += original_tokens - estimate_tokens(&truncated);
                            // Update part
                        }
                    }
                }
            }
        }

        Ok(CompactionResult {
            tier: CompactionTier::T1,
            tokens_before: 0, // Computed from message cache
            tokens_after: 0,
            tokens_freed,
            messages_compacted: messages.len(),
        })
    }

    /// T3: Emergency truncation (no LLM call, < 100ms)
    async fn compact_emergency(
        &self,
        state: &AppState,
        session_id: SessionID,
    ) -> Result<CompactionResult> {
        let messages = state.db.messages_for_session(session_id).await?;
        let total = messages.len();
        let keep_last = 5; // Keep last 5 messages intact

        let mut tokens_freed = 0u64;

        for (i, message) in messages.iter().enumerate() {
            if i >= total - keep_last {
                break; // Keep recent messages
            }

            for part in &message.parts {
                match part {
                    Part::ToolCall(tc) => {
                        if let Some(result) = &tc.result {
                            if result.len() > 200 {
                                let truncated = format!(
                                    "{}... [emergency truncated]",
                                    &result[..200]
                                );
                                tokens_freed += estimate_tokens(result)
                                    - estimate_tokens(&truncated);
                            }
                        }
                    }
                    Part::Reasoning(_) => {
                        // Drop old reasoning parts entirely
                        if i < total - 3 {
                            tokens_freed += estimate_tokens(&part.text_content());
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(CompactionResult {
            tier: CompactionTier::T3,
            tokens_before: 0,
            tokens_after: 0,
            tokens_freed,
            messages_compacted: total - keep_last,
        })
    }
}
```

#### Integration Points

**Fast Apply + Edit Tool**: The `edit` tool gains a `fast_apply` mode:
```rust
// In the edit tool, when the LLM provides a lazy edit snippet:
if args.contains_key("snippet") {
    // Use fast apply engine
    let result = state.fast_apply.apply(&original, &snippet, &language)?;
    // If fast apply fails, fall back to search-and-replace
}
```

**Smart Grep + Grep Tool**: The `grep` tool gains a `--symbols` flag:
```rust
// Enhanced grep tool with symbol awareness
if args.get("symbols").and_then(|v| v.as_bool()) == Some(true) {
    state.smart_grep.search(root, &query, SearchType::Symbol).await
} else {
    // Standard ripgrep text search
}
```

**Compaction + Session Engine**: Compaction is triggered by the prompt loop:
```rust
// In assemble_prompt, after computing token count:
let usage = token_counter.context_usage_ratio(model_context_window);
if usage >= config.compaction.t1_threshold {
    state.compaction.compact(state, session_id, usage, &config.compaction).await?;
}
```

### Alternatives Considered

1. **Use external Fast Apply API (like Morph)**: Rejected. External API adds network latency, cost, and sends your code to third parties. Native Rust is faster, free, and private.
2. **Use regex for edit application**: Rejected. Regex-based approaches benchmark at 84-96% accuracy. AST-aware merge should achieve >99%.
3. **Use external embedding model for smart grep**: Rejected. Tree-sitter AST parsing + ripgrep is faster and doesn't require an embedding model. Semantic search via embeddings can be added later for natural-language queries.
4. **Single compaction threshold (only T2)**: Rejected. Tiered compaction provides better cost/quality tradeoffs. T1 is free (no LLM), T2 is cheap (utility model), T3 is emergency-only.
5. **Compress model output instead of tool responses**: Rejected. The key insight from Morph's analysis: tool responses cause most context bloat. Model output should be preserved intact (it's the reasoning chain).

## Success Criteria

- **SC-001**: Fast apply accuracy > 99% on a benchmark of 1000 lazy edit snippets across 5 languages
- **SC-002**: Fast apply latency < 50ms for files < 10K lines
- **SC-003**: Smart grep symbol search < 200ms for projects with < 100K files
- **SC-004**: T1 compaction reduces tool response tokens by 40-60% with zero quality loss
- **SC-005**: T2 compaction reduces total context by 50-70% while preserving task continuity
- **SC-006**: T3 emergency compaction completes in < 100ms
- **SC-007**: Overall token cost reduction of 3-5x compared to naive approach (no compaction + full file writes)

## Assumptions

- Tree-sitter grammars for the 13 supported languages are stable and well-maintained
- `// ... existing code ...` markers (and language equivalents) are the dominant lazy edit format across frontier models
- Tool responses are the primary source of context bloat (validated by Morph's analysis)
- T1 compaction's lossy truncation of tool outputs is acceptable (agents can re-read files if needed)
- Fast apply's AST merge handles 95%+ of edit patterns; the remaining 5% are caught by the fallback chain

## Open Questions

- Should fast apply support language-specific marker detection (e.g., `# ... existing code ...` for Python, `<!-- existing code -->` for HTML)?
- Should smart grep results be cached across turns? (If the codebase hasn't changed, re-searching is wasteful)
- Should T1 compaction be configurable per tool? (e.g., always compress `bash` output but never compress `read` output)
- How to handle compaction in agent teams? (Each agent has its own session, but the lead may have injected messages from multiple agents)
- Should we add a `compaction_report` visible in the TUI showing what was compacted and why?
