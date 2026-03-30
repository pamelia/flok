//! Markdown rendering component for iocraft.
//!
//! Renders markdown as styled iocraft elements with:
//! - Cyan bold headings
//! - Fenced code blocks with language labels and dark background
//! - Green inline code
//! - Bold in white
//! - Italic
//! - Bullet and numbered lists with cyan markers
//! - Tables with box-drawing borders

use iocraft::prelude::*;

use crate::theme::Theme;

#[derive(Default, Props)]
pub struct MarkdownProps {
    pub content: String,
    pub theme: Option<Theme>,
}

#[component]
pub fn Markdown(props: &MarkdownProps) -> impl Into<AnyElement<'static>> {
    let theme = props.theme.unwrap_or_default();
    let blocks = parse_markdown_blocks(&props.content);

    element! {
        View(flex_direction: FlexDirection::Column, width: 100pct) {
            #(blocks.into_iter().enumerate().map(|(i, block)| {
                render_block(i, block, theme)
            }))
        }
    }
}

#[derive(Debug)]
#[allow(clippy::enum_variant_names)]
enum Block {
    Heading(u8, String),
    CodeBlock(String, String),
    BulletItem(String),
    NumberedItem(usize, String),
    TableRow(Vec<String>, bool), // cells, is_header
    Paragraph(String),
    Empty,
}

fn parse_markdown_blocks(input: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut in_code = false;
    let mut code_lang = String::new();
    let mut code_buf = String::new();
    let mut prev_was_table_sep = false;

    for line in input.lines() {
        let trimmed = line.trim();

        // Fenced code blocks
        if trimmed.starts_with("```") {
            if in_code {
                blocks.push(Block::CodeBlock(
                    std::mem::take(&mut code_lang),
                    std::mem::take(&mut code_buf),
                ));
                in_code = false;
            } else {
                in_code = true;
                code_lang = trimmed.strip_prefix("```").unwrap_or("").to_string();
            }
            continue;
        }

        if in_code {
            if !code_buf.is_empty() {
                code_buf.push('\n');
            }
            code_buf.push_str(line);
            continue;
        }

        if trimmed.is_empty() {
            prev_was_table_sep = false;
            blocks.push(Block::Empty);
            continue;
        }

        // Table separator row (|---|---|)
        if trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.contains("---") {
            prev_was_table_sep = true;
            continue;
        }

        // Table rows
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            let cells: Vec<String> =
                trimmed[1..trimmed.len() - 1].split('|').map(|c| c.trim().to_string()).collect();
            // If prev row was a separator, the one before that was a header
            // Mark current as non-header; the one before the separator was header
            let is_header = !prev_was_table_sep
                && blocks.iter().rev().all(|b| !matches!(b, Block::TableRow(_, false)));
            blocks.push(Block::TableRow(cells, is_header));
            prev_was_table_sep = false;
            continue;
        }
        prev_was_table_sep = false;

        // Headings
        if let Some(rest) = trimmed.strip_prefix("### ") {
            blocks.push(Block::Heading(3, rest.to_string()));
        } else if let Some(rest) = trimmed.strip_prefix("## ") {
            blocks.push(Block::Heading(2, rest.to_string()));
        } else if let Some(rest) = trimmed.strip_prefix("# ") {
            blocks.push(Block::Heading(1, rest.to_string()));
        }
        // Bullet lists
        else if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* "))
        {
            blocks.push(Block::BulletItem(rest.to_string()));
        }
        // Numbered lists
        else if let Some(pos) = trimmed.find(". ") {
            let prefix = &trimmed[..pos];
            if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()) {
                if let Ok(num) = prefix.parse::<usize>() {
                    blocks.push(Block::NumberedItem(num, trimmed[pos + 2..].to_string()));
                } else {
                    blocks.push(Block::Paragraph(trimmed.to_string()));
                }
            } else {
                blocks.push(Block::Paragraph(trimmed.to_string()));
            }
        } else {
            blocks.push(Block::Paragraph(trimmed.to_string()));
        }
    }

    if in_code {
        blocks.push(Block::CodeBlock(code_lang, code_buf));
    }

    blocks
}

fn render_block(idx: usize, block: Block, theme: Theme) -> AnyElement<'static> {
    let key = ElementKey::new(idx);
    match block {
        Block::Heading(1, text) => element! {
            View(key, padding_top: 1u32) {
                Text(
                    content: text,
                    color: theme.heading,
                    weight: Weight::Bold,
                    decoration: TextDecoration::Underline,
                )
            }
        }
        .into_any(),
        Block::Heading(2, text) => element! {
            View(key, padding_top: 1u32) {
                Text(content: text, color: theme.heading, weight: Weight::Bold)
            }
        }
        .into_any(),
        Block::Heading(_, text) => element! {
            View(key) {
                Text(content: text, color: theme.heading, weight: Weight::Bold)
            }
        }
        .into_any(),
        Block::CodeBlock(lang, code) => {
            let label = if lang.is_empty() { String::new() } else { format!(" {lang} ") };
            element! {
                View(key, flex_direction: FlexDirection::Column, padding_top: 1u32) {
                    #(if label.is_empty() { None } else {
                        Some(element! { Text(content: label, color: theme.text_muted) })
                    })
                    View(
                        background_color: theme.code_bg,
                        padding_left: 2u32,
                        padding_right: 1u32,
                        padding_top: 1u32,
                        padding_bottom: 1u32,
                        border_style: BorderStyle::Single,
                        border_color: theme.border,
                    ) {
                        Text(content: code, color: theme.text)
                    }
                }
            }
            .into_any()
        }
        Block::BulletItem(text) => {
            let spans = parse_inline(&text, theme);
            element! {
                View(key, flex_direction: FlexDirection::Row) {
                    Text(content: "  \u{2022} ", color: theme.primary)
                    MixedText(contents: spans)
                }
            }
            .into_any()
        }
        Block::NumberedItem(num, text) => {
            let spans = parse_inline(&text, theme);
            element! {
                View(key, flex_direction: FlexDirection::Row) {
                    Text(content: format!("  {num}. "), color: theme.primary)
                    MixedText(contents: spans)
                }
            }
            .into_any()
        }
        Block::TableRow(cells, is_header) => {
            let fg = if is_header { theme.primary } else { theme.text };
            let weight = if is_header { Weight::Bold } else { Weight::Normal };
            let border_char = "\u{2502}";
            element! {
                View(key, flex_direction: FlexDirection::Row) {
                    Text(content: border_char, color: theme.table_border)
                    #(cells.into_iter().enumerate().map(|(ci, cell)| {
                        let spans = if is_header {
                            vec![MixedTextContent::new(cell).color(fg).weight(weight)]
                        } else {
                            parse_inline(&cell, theme)
                        };
                        element! {
                            View(key: ci, flex_direction: FlexDirection::Row, min_width: 10u32) {
                                Text(content: " ", color: theme.bg)
                                MixedText(contents: spans)
                                Text(content: " ", color: theme.bg)
                                Text(content: border_char, color: theme.table_border)
                            }
                        }
                    }))
                }
            }
            .into_any()
        }
        Block::Paragraph(text) => {
            let spans = parse_inline(&text, theme);
            element! {
                View(key) {
                    MixedText(contents: spans)
                }
            }
            .into_any()
        }
        Block::Empty => element! {
            View(key, min_height: 1u32) {}
        }
        .into_any(),
    }
}

/// Parse inline markdown: **bold**, *italic*, `code`, plain text.
fn parse_inline(text: &str, theme: Theme) -> Vec<MixedTextContent> {
    let mut spans: Vec<MixedTextContent> = Vec::new();
    let mut chars = text.char_indices().peekable();
    let mut current = String::new();

    while let Some((i, ch)) = chars.next() {
        match ch {
            '`' => {
                if !current.is_empty() {
                    spans.push(
                        MixedTextContent::new(std::mem::take(&mut current)).color(theme.text),
                    );
                }
                let mut code = String::new();
                for (_, c) in chars.by_ref() {
                    if c == '`' {
                        break;
                    }
                    code.push(c);
                }
                spans.push(MixedTextContent::new(format!(" {code} ")).color(theme.code_fg));
            }
            '*' if text[i..].starts_with("**") => {
                if !current.is_empty() {
                    spans.push(
                        MixedTextContent::new(std::mem::take(&mut current)).color(theme.text),
                    );
                }
                chars.next(); // skip second *
                let mut bold = String::new();
                while let Some((_, c)) = chars.next() {
                    if c == '*' && chars.peek().is_some_and(|(_, nc)| *nc == '*') {
                        chars.next();
                        break;
                    }
                    bold.push(c);
                }
                spans.push(MixedTextContent::new(bold).color(theme.bold_fg).weight(Weight::Bold));
            }
            '*' | '_' => {
                if !current.is_empty() {
                    spans.push(
                        MixedTextContent::new(std::mem::take(&mut current)).color(theme.text),
                    );
                }
                let delim = ch;
                let mut italic_text = String::new();
                for (_, c) in chars.by_ref() {
                    if c == delim {
                        break;
                    }
                    italic_text.push(c);
                }
                spans.push(MixedTextContent::new(italic_text).color(theme.text).italic());
            }
            _ => {
                current.push(ch);
            }
        }
    }

    if !current.is_empty() {
        spans.push(MixedTextContent::new(current).color(theme.text));
    }

    if spans.is_empty() {
        spans.push(MixedTextContent::new(text.to_string()).color(theme.text));
    }

    spans
}
