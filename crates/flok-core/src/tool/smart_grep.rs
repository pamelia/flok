//! The `smart_grep` tool — searches code with symbol-aware formatting.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use regex::Regex;

use super::{Tool, ToolContext, ToolOutput};

const MAX_RESULTS: usize = 200;
const IGNORED_DIRS: &[&str] = &[".git", ".flok", "target", "node_modules"];

/// Search code with symbol-aware results.
pub struct SmartGrepTool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryType {
    Text,
    Symbol,
    Reference,
    Semantic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DefinitionKind {
    Function,
    Class,
    Struct,
    Trait,
    Interface,
    Impl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchKind {
    Definition(DefinitionKind),
    Call,
    Reference,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchResult {
    path: String,
    line: usize,
    symbol_name: String,
    kind: MatchKind,
    context: String,
}

#[async_trait::async_trait]
impl Tool for SmartGrepTool {
    fn name(&self) -> &'static str {
        "smart_grep"
    }

    fn description(&self) -> &'static str {
        "Search code with cleaner symbol-aware results. Supports `text`, `symbol`, \
         `reference`, and `semantic` query types so the agent gets definitions \
         and usages instead of raw grep dumps."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Text, symbol name, or wildcard pattern to search for"
                },
                "query_type": {
                    "type": "string",
                    "enum": ["text", "symbol", "reference", "semantic"],
                    "description": "Search mode. Defaults to `text`."
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (default: project root)"
                },
                "include": {
                    "type": "string",
                    "description": "Optional glob pattern to include (for example `src/**/*.rs`)"
                },
                "symbol_kind": {
                    "type": "string",
                    "enum": ["function", "class", "struct", "trait", "interface", "impl"],
                    "description": "Optional semantic filter used with `semantic` queries"
                }
            }
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let pattern = args["pattern"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: pattern"))?;
        let query_type = parse_query_type(args["query_type"].as_str())?;
        let symbol_kind = parse_definition_kind(args["symbol_kind"].as_str())?;
        let search_path = args["path"].as_str();
        let include = args["include"].as_str();
        let root = search_path
            .map_or_else(|| ctx.project_root.clone(), |p| resolve_path(&ctx.project_root, p));

        let include_glob = include
            .map(glob::Pattern::new)
            .transpose()
            .map_err(|error| anyhow::anyhow!("invalid include glob: {error}"))?;
        let files = collect_source_files(&root, include_glob.as_ref());
        if files.is_empty() {
            return Ok(ToolOutput::success("No searchable source files found.".to_string()));
        }

        let results = match query_type {
            QueryType::Text => run_text_search(&root, &files, pattern)?,
            QueryType::Symbol => run_symbol_search(&root, &files, pattern, None)?,
            QueryType::Reference => run_reference_search(&root, &files, pattern)?,
            QueryType::Semantic => run_symbol_search(&root, &files, pattern, symbol_kind)?,
        };

        if results.is_empty() {
            return Ok(ToolOutput::success("No matches found.".to_string()));
        }

        Ok(ToolOutput::success(format_results(&results)))
    }
}

fn parse_query_type(value: Option<&str>) -> anyhow::Result<QueryType> {
    match value.unwrap_or("text") {
        "text" => Ok(QueryType::Text),
        "symbol" => Ok(QueryType::Symbol),
        "reference" => Ok(QueryType::Reference),
        "semantic" => Ok(QueryType::Semantic),
        other => anyhow::bail!("unsupported query_type: {other}"),
    }
}

fn parse_definition_kind(value: Option<&str>) -> anyhow::Result<Option<DefinitionKind>> {
    match value {
        None => Ok(None),
        Some("function") => Ok(Some(DefinitionKind::Function)),
        Some("class") => Ok(Some(DefinitionKind::Class)),
        Some("struct") => Ok(Some(DefinitionKind::Struct)),
        Some("trait") => Ok(Some(DefinitionKind::Trait)),
        Some("interface") => Ok(Some(DefinitionKind::Interface)),
        Some("impl") => Ok(Some(DefinitionKind::Impl)),
        Some(other) => anyhow::bail!("unsupported symbol_kind: {other}"),
    }
}

fn resolve_path(project_root: &Path, file_path: &str) -> PathBuf {
    let path = Path::new(file_path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    }
}

fn collect_source_files(root: &Path, include: Option<&glob::Pattern>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                tracing::debug!(path = %dir.display(), %error, "smart_grep skipped unreadable directory");
                continue;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    tracing::debug!(%error, "smart_grep skipped unreadable entry");
                    continue;
                }
            };
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    tracing::debug!(path = %path.display(), %error, "smart_grep skipped unknown file type");
                    continue;
                }
            };

            if file_type.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if IGNORED_DIRS.iter().any(|ignored| *ignored == name) {
                    continue;
                }
                stack.push(path);
                continue;
            }

            if !file_type.is_file() || !is_supported_source_file(&path) {
                continue;
            }

            if let Some(include) = include {
                let relative = path.strip_prefix(root).unwrap_or(&path);
                if !include.matches_path(relative) && !include.matches_path(&path) {
                    continue;
                }
            }

            files.push(path);
        }
    }

    files.sort();
    files
}

fn is_supported_source_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(std::ffi::OsStr::to_str),
        Some(
            "rs" | "py"
                | "js"
                | "jsx"
                | "ts"
                | "tsx"
                | "go"
                | "java"
                | "c"
                | "cc"
                | "cpp"
                | "h"
                | "hpp"
                | "rb"
                | "sh"
        )
    )
}

fn run_text_search(
    root: &Path,
    files: &[PathBuf],
    pattern: &str,
) -> anyhow::Result<Vec<SearchResult>> {
    let regex =
        Regex::new(pattern).map_err(|error| anyhow::anyhow!("invalid regex pattern: {error}"))?;
    let mut results = Vec::new();

    for file in files {
        let Ok(content) = std::fs::read_to_string(file) else {
            continue;
        };
        for (index, line) in content.lines().enumerate() {
            if !regex.is_match(line) {
                continue;
            }
            results.push(SearchResult {
                path: relative_path(root, file),
                line: index + 1,
                symbol_name: pattern.to_string(),
                kind: MatchKind::Reference,
                context: line.trim().to_string(),
            });
            if results.len() >= MAX_RESULTS {
                return Ok(results);
            }
        }
    }

    Ok(results)
}

fn run_symbol_search(
    root: &Path,
    files: &[PathBuf],
    pattern: &str,
    kind_filter: Option<DefinitionKind>,
) -> anyhow::Result<Vec<SearchResult>> {
    let matcher = wildcard_regex(pattern)?;
    let mut results = Vec::new();

    for file in files {
        let Ok(content) = std::fs::read_to_string(file) else {
            continue;
        };
        if file_extension(file) == Some("rs") {
            match run_rust_symbol_search_file(root, file, &content, &matcher, kind_filter) {
                Ok(file_results) => {
                    for result in file_results {
                        results.push(result);
                        if results.len() >= MAX_RESULTS {
                            return Ok(results);
                        }
                    }
                    continue;
                }
                Err(error) => {
                    tracing::debug!(
                        path = %file.display(),
                        %error,
                        "smart_grep failed to parse rust file, using heuristic definition scan"
                    );
                }
            }
        }
        if matches!(file_extension(file), Some("js" | "jsx")) {
            match run_javascript_symbol_search_file(root, file, &content, &matcher, kind_filter) {
                Ok(file_results) => {
                    for result in file_results {
                        results.push(result);
                        if results.len() >= MAX_RESULTS {
                            return Ok(results);
                        }
                    }
                    continue;
                }
                Err(error) => {
                    tracing::debug!(
                        path = %file.display(),
                        %error,
                        "smart_grep failed to parse javascript file, using heuristic definition scan"
                    );
                }
            }
        }
        for (index, line) in content.lines().enumerate() {
            let Some((kind, symbol_name)) = classify_definition(file, line) else {
                continue;
            };
            if kind_filter.is_some_and(|expected| expected != kind)
                || !matcher.is_match(&symbol_name)
            {
                continue;
            }

            results.push(SearchResult {
                path: relative_path(root, file),
                line: index + 1,
                symbol_name,
                kind: MatchKind::Definition(kind),
                context: line.trim().to_string(),
            });
            if results.len() >= MAX_RESULTS {
                return Ok(results);
            }
        }
    }

    Ok(results)
}

fn run_reference_search(
    root: &Path,
    files: &[PathBuf],
    pattern: &str,
) -> anyhow::Result<Vec<SearchResult>> {
    let symbol_matcher = wildcard_regex(pattern)?;
    let exact_symbol = if has_wildcards(pattern) { None } else { Some(pattern.to_string()) };
    let mut results = Vec::new();

    for file in files {
        let Ok(content) = std::fs::read_to_string(file) else {
            continue;
        };
        if file_extension(file) == Some("rs") {
            match run_rust_reference_search_file(root, file, &content, &symbol_matcher) {
                Ok(file_results) => {
                    for result in file_results {
                        results.push(result);
                        if results.len() >= MAX_RESULTS {
                            return Ok(results);
                        }
                    }
                    continue;
                }
                Err(error) => {
                    tracing::debug!(
                        path = %file.display(),
                        %error,
                        "smart_grep failed to parse rust file, using heuristic reference scan"
                    );
                }
            }
        }
        if matches!(file_extension(file), Some("js" | "jsx")) {
            match run_javascript_reference_search_file(root, file, &content, &symbol_matcher) {
                Ok(file_results) => {
                    for result in file_results {
                        results.push(result);
                        if results.len() >= MAX_RESULTS {
                            return Ok(results);
                        }
                    }
                    continue;
                }
                Err(error) => {
                    tracing::debug!(
                        path = %file.display(),
                        %error,
                        "smart_grep failed to parse javascript file, using heuristic reference scan"
                    );
                }
            }
        }
        for (index, line) in content.lines().enumerate() {
            if let Some((_, symbol_name)) = classify_definition(file, line) {
                if symbol_matcher.is_match(&symbol_name) {
                    continue;
                }
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let Some(symbol_name) =
                extract_reference_symbol(trimmed, exact_symbol.as_deref(), &symbol_matcher)
            else {
                continue;
            };

            let kind = if exact_symbol
                .as_deref()
                .is_some_and(|symbol| trimmed.contains(&format!("{symbol}(")))
            {
                MatchKind::Call
            } else {
                MatchKind::Reference
            };

            results.push(SearchResult {
                path: relative_path(root, file),
                line: index + 1,
                symbol_name,
                kind,
                context: trimmed.to_string(),
            });
            if results.len() >= MAX_RESULTS {
                return Ok(results);
            }
        }
    }

    Ok(results)
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

fn file_extension(path: &Path) -> Option<&str> {
    path.extension().and_then(std::ffi::OsStr::to_str)
}

fn wildcard_regex(pattern: &str) -> anyhow::Result<Regex> {
    let mut regex = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            other => regex.push_str(&regex::escape(&other.to_string())),
        }
    }
    regex.push('$');
    Regex::new(&regex).map_err(|error| anyhow::anyhow!("invalid wildcard pattern: {error}"))
}

fn has_wildcards(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?')
}

fn extract_reference_symbol(
    line: &str,
    exact_symbol: Option<&str>,
    symbol_matcher: &Regex,
) -> Option<String> {
    if let Some(symbol) = exact_symbol {
        if line.contains(symbol) {
            return Some(symbol.to_string());
        }
        return None;
    }

    let word_regex = Regex::new(r"[A-Za-z_][A-Za-z0-9_]*").ok()?;
    let found = word_regex
        .find_iter(line)
        .map(|candidate| candidate.as_str())
        .find(|candidate| symbol_matcher.is_match(candidate))
        .map(ToString::to_string);
    found
}

fn run_rust_symbol_search_file(
    root: &Path,
    file: &Path,
    content: &str,
    matcher: &Regex,
    kind_filter: Option<DefinitionKind>,
) -> anyhow::Result<Vec<SearchResult>> {
    let tree = parse_rust_tree(content)?;
    let lines: Vec<&str> = content.lines().collect();
    let ctx = SearchFileContext { source: content.as_bytes(), lines: &lines, root, file };
    let mut results = Vec::new();
    collect_rust_symbol_results(tree.root_node(), &ctx, matcher, kind_filter, &mut results);
    Ok(results)
}

fn collect_rust_symbol_results(
    node: tree_sitter::Node<'_>,
    ctx: &SearchFileContext<'_>,
    matcher: &Regex,
    kind_filter: Option<DefinitionKind>,
    results: &mut Vec<SearchResult>,
) {
    if let Some(definition) = rust_definition_match(node, ctx.source, ctx.lines) {
        if kind_filter.map_or(true, |expected| expected == definition.kind)
            && matcher.is_match(&definition.symbol_name)
        {
            results.push(SearchResult {
                path: relative_path(ctx.root, ctx.file),
                line: definition.line,
                symbol_name: definition.symbol_name,
                kind: MatchKind::Definition(definition.kind),
                context: definition.context,
            });
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_rust_symbol_results(child, ctx, matcher, kind_filter, results);
    }
}

fn run_rust_reference_search_file(
    root: &Path,
    file: &Path,
    content: &str,
    matcher: &Regex,
) -> anyhow::Result<Vec<SearchResult>> {
    let tree = parse_rust_tree(content)?;
    let lines: Vec<&str> = content.lines().collect();
    let ctx = SearchFileContext { source: content.as_bytes(), lines: &lines, root, file };
    let mut definition_ranges = HashSet::new();
    collect_rust_definition_ranges(tree.root_node(), &ctx, &mut definition_ranges);

    let mut call_ranges = HashSet::new();
    let mut results = Vec::new();
    collect_rust_call_results(tree.root_node(), &ctx, matcher, &mut call_ranges, &mut results);
    collect_rust_reference_results(
        tree.root_node(),
        &ctx,
        matcher,
        &definition_ranges,
        &call_ranges,
        &mut results,
    );
    Ok(results)
}

fn collect_rust_definition_ranges(
    node: tree_sitter::Node<'_>,
    ctx: &SearchFileContext<'_>,
    ranges: &mut HashSet<(usize, usize)>,
) {
    if let Some(definition) = rust_definition_match(node, ctx.source, ctx.lines) {
        ranges.insert((definition.start_byte, definition.end_byte));
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_rust_definition_ranges(child, ctx, ranges);
    }
}

fn collect_rust_call_results(
    node: tree_sitter::Node<'_>,
    ctx: &SearchFileContext<'_>,
    matcher: &Regex,
    call_ranges: &mut HashSet<(usize, usize)>,
    results: &mut Vec<SearchResult>,
) {
    if node.kind() == "call_expression" {
        if let Some(function_node) = node.child_by_field_name("function") {
            if let Some(symbol_node) = rust_call_symbol_node(function_node) {
                if let Ok(symbol_name) = symbol_node.utf8_text(ctx.source) {
                    if matcher.is_match(symbol_name) {
                        let range = (symbol_node.start_byte(), symbol_node.end_byte());
                        if call_ranges.insert(range) {
                            let line = symbol_node.start_position().row + 1;
                            results.push(SearchResult {
                                path: relative_path(ctx.root, ctx.file),
                                line,
                                symbol_name: symbol_name.to_string(),
                                kind: MatchKind::Call,
                                context: line_context(ctx.lines, line),
                            });
                        }
                    }
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_rust_call_results(child, ctx, matcher, call_ranges, results);
    }
}

fn collect_rust_reference_results(
    node: tree_sitter::Node<'_>,
    ctx: &SearchFileContext<'_>,
    matcher: &Regex,
    definition_ranges: &HashSet<(usize, usize)>,
    call_ranges: &HashSet<(usize, usize)>,
    results: &mut Vec<SearchResult>,
) {
    if matches!(node.kind(), "identifier" | "field_identifier") {
        if let Ok(symbol_name) = node.utf8_text(ctx.source) {
            if matcher.is_match(symbol_name) {
                let range = (node.start_byte(), node.end_byte());
                if !definition_ranges.contains(&range) && !call_ranges.contains(&range) {
                    let line = node.start_position().row + 1;
                    results.push(SearchResult {
                        path: relative_path(ctx.root, ctx.file),
                        line,
                        symbol_name: symbol_name.to_string(),
                        kind: MatchKind::Reference,
                        context: line_context(ctx.lines, line),
                    });
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_rust_reference_results(
            child,
            ctx,
            matcher,
            definition_ranges,
            call_ranges,
            results,
        );
    }
}

fn rust_call_symbol_node(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    if matches!(node.kind(), "identifier" | "field_identifier") {
        return Some(node);
    }

    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();
    for child in children.into_iter().rev() {
        if let Some(found) = rust_call_symbol_node(child) {
            return Some(found);
        }
    }
    None
}

fn parse_rust_tree(content: &str) -> anyhow::Result<tree_sitter::Tree> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .map_err(|error| anyhow::anyhow!("failed to load rust parser: {error}"))?;
    parser.parse(content, None).ok_or_else(|| anyhow::anyhow!("failed to parse rust source"))
}

fn rust_definition_match(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    lines: &[&str],
) -> Option<RustDefinitionMatch> {
    match node.kind() {
        "function_item" => {
            rust_named_definition(node, "name", DefinitionKind::Function, source, lines)
        }
        "struct_item" => rust_named_definition(node, "name", DefinitionKind::Struct, source, lines),
        "trait_item" => rust_named_definition(node, "name", DefinitionKind::Trait, source, lines),
        "impl_item" => rust_named_definition(node, "type", DefinitionKind::Impl, source, lines),
        _ => None,
    }
}

fn rust_named_definition(
    node: tree_sitter::Node<'_>,
    field_name: &str,
    kind: DefinitionKind,
    source: &[u8],
    lines: &[&str],
) -> Option<RustDefinitionMatch> {
    let name_node = node.child_by_field_name(field_name)?;
    let symbol_name = name_node.utf8_text(source).ok()?.trim().to_string();
    if symbol_name.is_empty() {
        return None;
    }
    let line = name_node.start_position().row + 1;
    Some(RustDefinitionMatch {
        kind,
        symbol_name,
        line,
        context: line_context(lines, line),
        start_byte: name_node.start_byte(),
        end_byte: name_node.end_byte(),
    })
}

fn line_context(lines: &[&str], line: usize) -> String {
    lines.get(line.saturating_sub(1)).copied().unwrap_or_default().trim().to_string()
}

fn run_javascript_symbol_search_file(
    root: &Path,
    file: &Path,
    content: &str,
    matcher: &Regex,
    kind_filter: Option<DefinitionKind>,
) -> anyhow::Result<Vec<SearchResult>> {
    let tree = parse_javascript_tree(content)?;
    let lines: Vec<&str> = content.lines().collect();
    let ctx = SearchFileContext { source: content.as_bytes(), lines: &lines, root, file };
    let mut results = Vec::new();
    collect_javascript_symbol_results(tree.root_node(), &ctx, matcher, kind_filter, &mut results);
    Ok(results)
}

fn collect_javascript_symbol_results(
    node: tree_sitter::Node<'_>,
    ctx: &SearchFileContext<'_>,
    matcher: &Regex,
    kind_filter: Option<DefinitionKind>,
    results: &mut Vec<SearchResult>,
) {
    if let Some(definition) = javascript_definition_match(node, ctx.source, ctx.lines) {
        if kind_filter.map_or(true, |expected| expected == definition.kind)
            && matcher.is_match(&definition.symbol_name)
        {
            results.push(SearchResult {
                path: relative_path(ctx.root, ctx.file),
                line: definition.line,
                symbol_name: definition.symbol_name,
                kind: MatchKind::Definition(definition.kind),
                context: definition.context,
            });
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_javascript_symbol_results(child, ctx, matcher, kind_filter, results);
    }
}

fn run_javascript_reference_search_file(
    root: &Path,
    file: &Path,
    content: &str,
    matcher: &Regex,
) -> anyhow::Result<Vec<SearchResult>> {
    let tree = parse_javascript_tree(content)?;
    let lines: Vec<&str> = content.lines().collect();
    let ctx = SearchFileContext { source: content.as_bytes(), lines: &lines, root, file };
    let mut definition_ranges = HashSet::new();
    collect_javascript_definition_ranges(tree.root_node(), &ctx, &mut definition_ranges);

    let mut call_ranges = HashSet::new();
    let mut results = Vec::new();
    collect_javascript_call_results(
        tree.root_node(),
        &ctx,
        matcher,
        &mut call_ranges,
        &mut results,
    );
    collect_javascript_reference_results(
        tree.root_node(),
        &ctx,
        matcher,
        &definition_ranges,
        &call_ranges,
        &mut results,
    );
    Ok(results)
}

fn collect_javascript_definition_ranges(
    node: tree_sitter::Node<'_>,
    ctx: &SearchFileContext<'_>,
    ranges: &mut HashSet<(usize, usize)>,
) {
    if let Some(definition) = javascript_definition_match(node, ctx.source, ctx.lines) {
        ranges.insert((definition.start_byte, definition.end_byte));
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_javascript_definition_ranges(child, ctx, ranges);
    }
}

fn collect_javascript_call_results(
    node: tree_sitter::Node<'_>,
    ctx: &SearchFileContext<'_>,
    matcher: &Regex,
    call_ranges: &mut HashSet<(usize, usize)>,
    results: &mut Vec<SearchResult>,
) {
    if node.kind() == "call_expression" {
        if let Some(function_node) = node.child_by_field_name("function") {
            if let Some(symbol_node) = javascript_call_symbol_node(function_node) {
                if let Ok(symbol_name) = symbol_node.utf8_text(ctx.source) {
                    if matcher.is_match(symbol_name) {
                        let range = (symbol_node.start_byte(), symbol_node.end_byte());
                        if call_ranges.insert(range) {
                            let line = symbol_node.start_position().row + 1;
                            results.push(SearchResult {
                                path: relative_path(ctx.root, ctx.file),
                                line,
                                symbol_name: symbol_name.to_string(),
                                kind: MatchKind::Call,
                                context: line_context(ctx.lines, line),
                            });
                        }
                    }
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_javascript_call_results(child, ctx, matcher, call_ranges, results);
    }
}

fn collect_javascript_reference_results(
    node: tree_sitter::Node<'_>,
    ctx: &SearchFileContext<'_>,
    matcher: &Regex,
    definition_ranges: &HashSet<(usize, usize)>,
    call_ranges: &HashSet<(usize, usize)>,
    results: &mut Vec<SearchResult>,
) {
    if matches!(node.kind(), "identifier" | "property_identifier") {
        if let Ok(symbol_name) = node.utf8_text(ctx.source) {
            if matcher.is_match(symbol_name) {
                let range = (node.start_byte(), node.end_byte());
                if !definition_ranges.contains(&range) && !call_ranges.contains(&range) {
                    let line = node.start_position().row + 1;
                    results.push(SearchResult {
                        path: relative_path(ctx.root, ctx.file),
                        line,
                        symbol_name: symbol_name.to_string(),
                        kind: MatchKind::Reference,
                        context: line_context(ctx.lines, line),
                    });
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_javascript_reference_results(
            child,
            ctx,
            matcher,
            definition_ranges,
            call_ranges,
            results,
        );
    }
}

fn javascript_call_symbol_node(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    if matches!(node.kind(), "identifier" | "property_identifier") {
        return Some(node);
    }

    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();
    for child in children.into_iter().rev() {
        if let Some(found) = javascript_call_symbol_node(child) {
            return Some(found);
        }
    }
    None
}

#[derive(Debug, Clone)]
struct RustDefinitionMatch {
    kind: DefinitionKind,
    symbol_name: String,
    line: usize,
    context: String,
    start_byte: usize,
    end_byte: usize,
}

fn parse_javascript_tree(content: &str) -> anyhow::Result<tree_sitter::Tree> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_javascript::LANGUAGE.into())
        .map_err(|error| anyhow::anyhow!("failed to load javascript parser: {error}"))?;
    parser.parse(content, None).ok_or_else(|| anyhow::anyhow!("failed to parse javascript source"))
}

fn javascript_definition_match(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    lines: &[&str],
) -> Option<RustDefinitionMatch> {
    match node.kind() {
        "function_declaration" | "method_definition" => {
            rust_named_definition(node, "name", DefinitionKind::Function, source, lines)
        }
        "class_declaration" => {
            rust_named_definition(node, "name", DefinitionKind::Class, source, lines)
        }
        "variable_declarator" => javascript_variable_definition_match(node, source, lines),
        _ => None,
    }
}

fn javascript_variable_definition_match(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    lines: &[&str],
) -> Option<RustDefinitionMatch> {
    let value = node.child_by_field_name("value")?;
    if !matches!(value.kind(), "arrow_function" | "function" | "function_expression") {
        return None;
    }
    rust_named_definition(node, "name", DefinitionKind::Function, source, lines)
}

struct SearchFileContext<'a> {
    source: &'a [u8],
    lines: &'a [&'a str],
    root: &'a Path,
    file: &'a Path,
}

fn classify_definition(path: &Path, line: &str) -> Option<(DefinitionKind, String)> {
    let extension = file_extension(path)?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    match extension {
        "rs" => classify_rust_definition(trimmed),
        "py" => classify_python_definition(trimmed),
        "js" | "jsx" | "ts" | "tsx" => classify_js_definition(trimmed),
        "go" => classify_go_definition(trimmed),
        "java" => classify_java_definition(trimmed),
        "c" | "cc" | "cpp" | "h" | "hpp" => classify_cpp_definition(trimmed),
        "rb" => classify_ruby_definition(trimmed),
        "sh" => classify_shell_definition(trimmed),
        _ => None,
    }
}

fn classify_rust_definition(line: &str) -> Option<(DefinitionKind, String)> {
    capture_definition(
        line,
        &[
            (DefinitionKind::Function, r"^(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\b"),
            (DefinitionKind::Struct, r"^(?:pub\s+)?struct\s+([A-Za-z_][A-Za-z0-9_]*)\b"),
            (DefinitionKind::Trait, r"^(?:pub\s+)?trait\s+([A-Za-z_][A-Za-z0-9_]*)\b"),
            (
                DefinitionKind::Impl,
                r"^impl(?:<[^>]+>)?(?:\s+[A-Za-z_][A-Za-z0-9_:<>]*)?\s+for\s+([A-Za-z_][A-Za-z0-9_:<>]*)\b",
            ),
            (DefinitionKind::Impl, r"^impl(?:<[^>]+>)?\s+([A-Za-z_][A-Za-z0-9_:<>]*)\b"),
        ],
    )
}

fn classify_python_definition(line: &str) -> Option<(DefinitionKind, String)> {
    capture_definition(
        line,
        &[
            (DefinitionKind::Function, r"^(?:async\s+)?def\s+([A-Za-z_][A-Za-z0-9_]*)\b"),
            (DefinitionKind::Class, r"^class\s+([A-Za-z_][A-Za-z0-9_]*)\b"),
        ],
    )
}

fn classify_js_definition(line: &str) -> Option<(DefinitionKind, String)> {
    capture_definition(
        line,
        &[
            (
                DefinitionKind::Function,
                r"^(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_$][A-Za-z0-9_$]*)\b",
            ),
            (DefinitionKind::Class, r"^(?:export\s+)?class\s+([A-Za-z_$][A-Za-z0-9_$]*)\b"),
            (
                DefinitionKind::Function,
                r"^(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*=\s*(?:async\s*)?\(",
            ),
        ],
    )
}

fn classify_go_definition(line: &str) -> Option<(DefinitionKind, String)> {
    capture_definition(
        line,
        &[
            (DefinitionKind::Function, r"^func\s+([A-Za-z_][A-Za-z0-9_]*)\s*\("),
            (DefinitionKind::Function, r"^func\s+\([^)]*\)\s*([A-Za-z_][A-Za-z0-9_]*)\s*\("),
            (DefinitionKind::Struct, r"^type\s+([A-Za-z_][A-Za-z0-9_]*)\s+struct\b"),
            (DefinitionKind::Interface, r"^type\s+([A-Za-z_][A-Za-z0-9_]*)\s+interface\b"),
        ],
    )
}

fn classify_java_definition(line: &str) -> Option<(DefinitionKind, String)> {
    capture_definition(
        line,
        &[
            (DefinitionKind::Class, r"^(?:public\s+)?class\s+([A-Za-z_][A-Za-z0-9_]*)\b"),
            (DefinitionKind::Interface, r"^(?:public\s+)?interface\s+([A-Za-z_][A-Za-z0-9_]*)\b"),
            (
                DefinitionKind::Function,
                r"^(?:public|private|protected|static|final|native|synchronized|abstract|\s)+[A-Za-z_<>\[\]]+\s+([A-Za-z_][A-Za-z0-9_]*)\s*\([^;]*\)\s*\{?$",
            ),
        ],
    )
}

fn classify_cpp_definition(line: &str) -> Option<(DefinitionKind, String)> {
    capture_definition(
        line,
        &[
            (DefinitionKind::Class, r"^class\s+([A-Za-z_][A-Za-z0-9_]*)\b"),
            (DefinitionKind::Struct, r"^struct\s+([A-Za-z_][A-Za-z0-9_]*)\b"),
            (
                DefinitionKind::Function,
                r"^(?:inline\s+|static\s+|virtual\s+|constexpr\s+|[\w:<>\*&]+\s+)+([A-Za-z_][A-Za-z0-9_]*)\s*\([^;]*\)\s*\{?$",
            ),
        ],
    )
}

fn classify_ruby_definition(line: &str) -> Option<(DefinitionKind, String)> {
    capture_definition(
        line,
        &[
            (DefinitionKind::Function, r"^def\s+([A-Za-z_][A-Za-z0-9_!?=]*)\b"),
            (DefinitionKind::Class, r"^class\s+([A-Za-z_][A-Za-z0-9_:]*)\b"),
        ],
    )
}

fn classify_shell_definition(line: &str) -> Option<(DefinitionKind, String)> {
    capture_definition(
        line,
        &[(DefinitionKind::Function, r"^(?:function\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*\(\)\s*\{")],
    )
}

fn capture_definition(
    line: &str,
    patterns: &[(DefinitionKind, &str)],
) -> Option<(DefinitionKind, String)> {
    for (kind, pattern) in patterns {
        let regex = Regex::new(pattern).ok()?;
        if let Some(captures) = regex.captures(line) {
            if let Some(symbol_name) = captures.get(1) {
                return Some((*kind, symbol_name.as_str().to_string()));
            }
        }
    }
    None
}

fn format_results(results: &[SearchResult]) -> String {
    let mut grouped: BTreeMap<&str, Vec<&SearchResult>> = BTreeMap::new();
    for result in results {
        grouped.entry(&result.path).or_default().push(result);
    }

    let mut output = String::new();
    let mut emitted = 0usize;
    for (path, entries) in grouped {
        output.push_str(path);
        output.push_str(":\n");
        for entry in entries {
            emitted += 1;
            output.push_str("  ");
            output.push_str(&entry.symbol_name);
            output.push_str(" (line ");
            output.push_str(&entry.line.to_string());
            output.push_str(") [");
            output.push_str(match entry.kind {
                MatchKind::Definition(kind) => match kind {
                    DefinitionKind::Function => "definition:function",
                    DefinitionKind::Class => "definition:class",
                    DefinitionKind::Struct => "definition:struct",
                    DefinitionKind::Trait => "definition:trait",
                    DefinitionKind::Interface => "definition:interface",
                    DefinitionKind::Impl => "definition:impl",
                },
                MatchKind::Call => "call",
                MatchKind::Reference => "reference",
            });
            output.push_str("]\n");
            output.push_str("    ");
            output.push_str(entry.context.trim());
            output.push('\n');
        }
    }

    if results.len() >= MAX_RESULTS {
        use std::fmt::Write as _;
        let _ = writeln!(output, "\n... (showing first {emitted} matches)");
    }

    output.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn smart_grep_text_search_finds_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn verify_token() {}\n").unwrap();
        let ctx = ToolContext::test(dir.path().to_path_buf());

        let result = SmartGrepTool
            .execute(serde_json::json!({"pattern": "verify_token", "query_type": "text"}), &ctx)
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("main.rs"));
        assert!(result.content.contains("verify_token"));
    }

    #[tokio::test]
    async fn smart_grep_symbol_search_returns_definition() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.rs"),
            "pub fn verify_token() {}\nfn refresh_token() {}\n",
        )
        .unwrap();
        let ctx = ToolContext::test(dir.path().to_path_buf());

        let result = SmartGrepTool
            .execute(serde_json::json!({"pattern": "verify_*", "query_type": "symbol"}), &ctx)
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("definition:function"));
        assert!(result.content.contains("verify_token"));
        assert!(!result.content.contains("refresh_token"));
    }

    #[tokio::test]
    async fn smart_grep_reference_search_distinguishes_calls() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.rs"),
            "fn verify_token() {}\nfn use_it() { verify_token(); }\n",
        )
        .unwrap();
        let ctx = ToolContext::test(dir.path().to_path_buf());

        let result = SmartGrepTool
            .execute(
                serde_json::json!({"pattern": "verify_token", "query_type": "reference"}),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("[call]"));
        assert!(!result.content.contains("definition:function"));
    }

    #[tokio::test]
    async fn smart_grep_semantic_search_filters_symbol_kind() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.py"),
            "class AuthManager:\n    pass\n\ndef auth_user():\n    return True\n",
        )
        .unwrap();
        let ctx = ToolContext::test(dir.path().to_path_buf());

        let result = SmartGrepTool
            .execute(
                serde_json::json!({
                    "pattern": "auth*",
                    "query_type": "semantic",
                    "symbol_kind": "function"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("auth_user"));
        assert!(!result.content.contains("AuthManager"));
    }

    #[tokio::test]
    async fn smart_grep_rust_semantic_search_uses_ast_for_impl_generics() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.rs"),
            "struct AuthManager<'a, T> {\n    marker: &'a T,\n}\n\nimpl<'a, T> AuthManager<'a, T>\nwhere\n    T: Clone,\n{\n    fn verify_token(&self) {}\n}\n",
        )
        .unwrap();
        let ctx = ToolContext::test(dir.path().to_path_buf());

        let result = SmartGrepTool
            .execute(
                serde_json::json!({
                    "pattern": "AuthManager*",
                    "query_type": "semantic",
                    "symbol_kind": "impl"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("definition:impl"));
        assert!(result.content.contains("AuthManager<'a, T>"));
    }

    #[tokio::test]
    async fn smart_grep_rust_reference_search_marks_turbofish_method_calls() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.rs"),
            "struct User;\nstruct AuthService;\n\nimpl AuthService {\n    fn verify_token<T>(&self) {}\n}\n\nfn use_it(service: &AuthService) {\n    service.verify_token::<User>();\n}\n",
        )
        .unwrap();
        let ctx = ToolContext::test(dir.path().to_path_buf());

        let result = SmartGrepTool
            .execute(
                serde_json::json!({"pattern": "verify_token", "query_type": "reference"}),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("[call]"));
        assert!(result.content.contains("service.verify_token::<User>();"));
    }

    #[tokio::test]
    async fn smart_grep_javascript_symbol_search_finds_class_method_definition() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.js"),
            "class AuthService {\n  verifyToken(user) {\n    return user;\n  }\n}\n",
        )
        .unwrap();
        let ctx = ToolContext::test(dir.path().to_path_buf());

        let result = SmartGrepTool
            .execute(
                serde_json::json!({
                    "pattern": "verifyToken",
                    "query_type": "semantic",
                    "symbol_kind": "function"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("definition:function"));
        assert!(result.content.contains("verifyToken"));
    }

    #[tokio::test]
    async fn smart_grep_javascript_reference_search_marks_member_calls() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.js"),
            "class AuthService {\n  verifyToken(user) {\n    return user;\n  }\n}\n\nfunction useIt(service, user) {\n  return service.verifyToken(user);\n}\n",
        )
        .unwrap();
        let ctx = ToolContext::test(dir.path().to_path_buf());

        let result = SmartGrepTool
            .execute(serde_json::json!({"pattern": "verifyToken", "query_type": "reference"}), &ctx)
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("[call]"));
        assert!(result.content.contains("service.verifyToken(user);"));
    }
}
