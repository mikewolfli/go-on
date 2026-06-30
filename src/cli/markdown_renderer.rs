//! Terminal Markdown Renderer
//!
//! Renders markdown text (code blocks, tables, lists, inline formatting)
//! to ANSI-colored terminal output.

// ── Language → terminal colors ───────────────────────────────────────────
// A small built-in map for common languages to give code blocks color hints.
fn lang_color(lang: &str) -> &'static str {
    match lang {
        "rust" | "rs" => "33",                             // yellow
        "python" | "py" => "34",                           // blue
        "javascript" | "js" | "typescript" | "ts" => "32", // green
        "go" | "golang" => "36",                           // cyan
        "json" | "yaml" | "yml" | "toml" => "35",          // magenta
        "html" | "xml" | "svg" => "31",                    // red
        "bash" | "sh" | "zsh" | "shell" => "33",           // yellow
        "diff" | "patch" => "32",                          // green
        "sql" => "34",                                     // blue
        _ => "90",                                         // bright black (gray)
    }
}

/// Render markdown text to ANSI-colored output.
///
/// Returns a `String` with embedded ANSI escape codes suitable for
/// writing to a terminal that supports basic ANSI (all modern terminals).
pub fn render_markdown(text: &str) -> String {
    let mut out = String::new();
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_content = String::new();
    let mut in_table = false;
    let mut table_col_widths: Vec<usize> = Vec::new();
    let mut table_rows: Vec<Vec<String>> = Vec::new();

    for line in text.lines() {
        // ── Code block fences ──
        if line.trim_start().starts_with("```") {
            if in_code_block {
                // End code block — render accumulated content
                out.push_str(&render_code_block(&code_content, &code_lang));
                code_content.clear();
                code_lang.clear();
                in_code_block = false;
            } else {
                // Start code block
                in_code_block = true;
                code_lang = line
                    .trim_start()
                    .trim_start_matches("```")
                    .trim()
                    .to_string();
            }
            continue;
        }

        if in_code_block {
            code_content.push_str(line);
            code_content.push('\n');
            continue;
        }

        // ── Tables ──
        if line.trim_start().starts_with('|') {
            let cells: Vec<&str> = line
                .split('|')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();

            // Detect separator row (e.g. |---|---|)
            if cells
                .iter()
                .all(|c| c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' '))
            {
                continue; // skip separator row
            }

            if !in_table {
                in_table = true;
                table_rows.clear();
                table_col_widths = cells.iter().map(|c| c.chars().count()).collect();
            } else {
                // Update max column widths
                for (i, cell) in cells.iter().enumerate() {
                    if i < table_col_widths.len() {
                        table_col_widths[i] = table_col_widths[i].max(cell.chars().count());
                    }
                }
            }
            table_rows.push(cells.iter().map(|s| s.to_string()).collect());
            continue;
        } else if in_table {
            // End of table — render it
            out.push_str(&render_table(&table_rows, &table_col_widths));
            in_table = false;
            table_rows.clear();
            table_col_widths.clear();
        }

        // ── Horizontal rules ──
        let trimmed = line.trim();
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            out.push_str(&format!(
                "{}{}{}\n",
                ansi("90"),
                "─".repeat(terminal_width().min(60)),
                ansi("0")
            ));
            continue;
        }

        // ── Headings ──
        if let Some(level) = heading_level(trimmed) {
            let content = trimmed.trim_start_matches('#').trim();
            let color = match level {
                1 => "1;36", // bold cyan
                2 => "1;34", // bold blue
                3 => "1;33", // bold yellow
                _ => "1;90", // bold gray
            };
            let prefix = "#".repeat(level);
            out.push_str(&format!(
                "\n{}{}{}{} {}\n",
                ansi(color),
                prefix,
                ansi("0"),
                ansi("1"),
                render_inline(content)
            ));
            continue;
        }

        // ── Blockquotes ──
        if let Some(content) = trimmed.strip_prefix('>') {
            out.push_str(&format!(
                " {}│ {}\n",
                ansi("90"),
                render_inline(content.trim())
            ));
            continue;
        }

        // ── Lists ──
        if let Some(content) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            out.push_str(&format!(
                "  {}{} {}{}\n",
                ansi("33"),
                "•",
                ansi("0"),
                render_inline(content)
            ));
            continue;
        }
        if let Some(content) = trimmed.strip_prefix('-').or_else(|| {
            trimmed
                .strip_prefix('*')
                .or_else(|| trimmed.strip_prefix('+'))
        }) {
            if trimmed.len() > 2 && content.trim().len() < trimmed.len() - 1 {
                // Was a list item already handled above
            }
        }
        // Ordered list
        if let Some((num_str, content)) = trimmed.split_once(". ") {
            if num_str.chars().all(|c| c.is_ascii_digit()) {
                out.push_str(&format!(
                    "  {}.{} {}\n",
                    ansi("36"),
                    ansi("0"),
                    render_inline(content)
                ));
                continue;
            }
        }

        // ── Regular paragraph with inline formatting ──
        let rendered = render_inline(trimmed);
        if !rendered.is_empty() {
            out.push_str(&rendered);
            out.push('\n');
        } else if trimmed.is_empty() {
            out.push('\n');
        }
    }

    // Flush remaining code block / table
    if in_code_block {
        out.push_str(&render_code_block(&code_content, &code_lang));
    }
    if in_table {
        out.push_str(&render_table(&table_rows, &table_col_widths));
    }

    out
}

/// Render inline formatting: bold, italic, inline code, links.
fn render_inline(text: &str) -> String {
    let mut out = String::new();
    let s = text;
    let mut pos = 0;

    while pos < s.len() {
        let remaining = &s[pos..];

        // Inline code: `...`
        if let Some(rest) = remaining.strip_prefix('`') {
            pos += 1;
            let end = rest
                .find('`')
                .map(|i| i + 1)
                .unwrap_or_else(|| rest.len());
            let code = &rest[..end];
            out.push_str(&format!("{}{}{}", ansi("90"), code, ansi("0")));
            pos += end;
            continue;
        }

        // Bold: **...**
        if let Some(rest) = remaining.strip_prefix("**") {
            pos += 2;
            if let Some(end) = rest.find("**") {
                let inner = &rest[..end];
                out.push_str(&format!("{}{}{}", ansi("1"), inner, ansi("0")));
                pos += end + 2;
                continue;
            }
        }

        // Italic: *...* (only if not followed by another *)
        if let Some(rest) = remaining.strip_prefix('*') {
            if !rest.starts_with('*') {
                pos += 1;
                if let Some(end) = rest.find('*') {
                    let inner = &rest[..end];
                    out.push_str(&format!("{}{}{}", ansi("3"), inner, ansi("0")));
                    pos += end + 1;
                    continue;
                }
            }
        }

        // Bold: __...__
        if let Some(rest) = remaining.strip_prefix("__") {
            pos += 2;
            if let Some(end) = rest.find("__") {
                let inner = &rest[..end];
                out.push_str(&format!("{}{}{}", ansi("1"), inner, ansi("0")));
                pos += end + 2;
                continue;
            }
        }

        // Italic: _..._
        if let Some(rest) = remaining.strip_prefix('_') {
            if !rest.starts_with('_') {
                pos += 1;
                if let Some(end) = rest.find('_') {
                    let inner = &rest[..end];
                    out.push_str(&format!("{}{}{}", ansi("3"), inner, ansi("0")));
                    pos += end + 1;
                    continue;
                }
            }
        }

        // Link: [text](url)
        if let Some(after_open) = remaining.strip_prefix('[') {
            pos += 1;
            if let Some(text_end) = after_open.find(']') {
                if after_open[text_end + 1..].starts_with('(') {
                    let text = &after_open[..text_end];
                    let after_bracket = &after_open[text_end + 1..];
                    if let Some(url_end) = after_bracket.find(')') {
                        // let url = &after_bracket[1..url_end];
                        out.push_str(&format!("{}{}{}", ansi("4;34"), text, ansi("0")));
                        pos += 1 + text_end + 1 + url_end + 1;
                        continue;
                    }
                }
            }
        }

        // Plain character
        out.push(s[pos..].chars().next().unwrap());
        pos += s[pos..].chars().next().unwrap().len_utf8();
    }

    out
}

/// Render a code block with syntax coloring.
fn render_code_block(code: &str, lang: &str) -> String {
    let color = lang_color(lang);
    let lang_label = if lang.is_empty() { "" } else { lang };
    let mut out = String::new();

    // Language label line
    if !lang_label.is_empty() {
        out.push_str(&format!(
            " {}┌─ {} {}┐\n",
            ansi("90"),
            ansi(color),
            lang_label
        ));
    } else {
        out.push_str(&format!(" {}┌─ code ─┐\n", ansi("90")));
    }

    // Code lines with gutter
    for line in code.lines() {
        out.push_str(&format!(" {}│{}{}\n", ansi("90"), ansi(color), line));
    }

    out.push_str(&format!(
        " {}└{}─────┘\n",
        ansi("90"),
        "─".repeat(terminal_width().min(40).saturating_sub(6))
    ));
    out
}

/// Render a table with column alignment.
fn render_table(rows: &[Vec<String>], col_widths: &[usize]) -> String {
    if rows.is_empty() || col_widths.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    let header_widths: Vec<usize> = rows[0].iter().map(|c| c.chars().count()).collect();

    // Use the wider of header vs column widths
    let widths: Vec<usize> = (0..col_widths.len())
        .map(|i| {
            let hw = header_widths.get(i).copied().unwrap_or(0);
            let cw = col_widths.get(i).copied().unwrap_or(0);
            hw.max(cw)
        })
        .collect();

    // Top border
    out.push_str(&format!(
        " {}{} ",
        ansi("90"),
        widths
            .iter()
            .map(|w| "─".repeat(w + 2))
            .collect::<Vec<_>>()
            .join("┬")
    ));
    out.push('\n');

    // Header row
    for (i, cell) in rows[0].iter().enumerate() {
        let w = widths.get(i).copied().unwrap_or(0);
        out.push_str(&format!(
            " {}│ {} {}",
            ansi("1"),
            cell,
            " ".repeat(w.saturating_sub(cell.chars().count()))
        ));
    }
    out.push_str(&format!("│{}\n", ansi("0")));

    // Separator
    out.push_str(&format!(
        " {}{} ",
        ansi("90"),
        widths
            .iter()
            .map(|w| "═".repeat(w + 2))
            .collect::<Vec<_>>()
            .join("╪")
    ));
    out.push('\n');

    // Data rows
    for row in rows.iter().skip(1) {
        for (i, cell) in row.iter().enumerate() {
            let w = widths.get(i).copied().unwrap_or(0);
            out.push_str(&format!(
                " {}│ {} {}",
                ansi("90"),
                cell,
                " ".repeat(w.saturating_sub(cell.chars().count()))
            ));
        }
        out.push_str(&format!("│{}\n", ansi("0")));
    }

    // Bottom border
    out.push_str(&format!(
        " {}{} ",
        ansi("90"),
        widths
            .iter()
            .map(|w| "─".repeat(w + 2))
            .collect::<Vec<_>>()
            .join("┴")
    ));
    out.push('\n');

    out
}

/// Determine heading level (1-6) or None.
fn heading_level(trimmed: &str) -> Option<usize> {
    let mut count = 0;
    for ch in trimmed.chars() {
        if ch == '#' {
            count += 1;
        } else if ch == ' ' && count > 0 {
            return Some(count);
        } else {
            return None;
        }
    }
    None
}

/// Estimate terminal width (default 80, use actual if available).
fn terminal_width() -> usize {
    #[cfg(unix)]
    {
        use std::process::Command;
        if let Ok(out) = Command::new("stty").arg("size").output() {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if let Some((_, cols)) = stdout.trim().split_once(' ') {
                    if let Ok(c) = cols.parse::<usize>() {
                        return c;
                    }
                }
            }
        }
    }
    80
}

/// ANSI helper (same as the macro in chat.rs but as a function for use in this module).
fn ansi(code: &str) -> String {
    format!("\u{001B}[{}m", code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heading_levels() {
        assert_eq!(heading_level("# H1"), Some(1));
        assert_eq!(heading_level("## H2"), Some(2));
        assert_eq!(heading_level("###### H6"), Some(6));
        assert_eq!(heading_level("not a heading"), None);
        assert_eq!(heading_level("###"), None);
    }

    #[test]
    fn test_inline_code() {
        let result = render_inline("use `foo` bar");
        assert!(result.contains("foo"));
        assert!(result.contains("bar"));
        assert!(result.contains("\u{001B}["));
    }

    #[test]
    fn test_bold_and_italic() {
        let result = render_inline("**bold** and *italic*");
        assert!(result.contains("bold"));
        assert!(result.contains("italic"));
    }

    #[test]
    fn test_code_block() {
        let result = render_markdown("```rust\nfn main() {}\n```");
        assert!(result.contains("fn main()"));
        assert!(result.contains("rust"));
    }

    #[test]
    fn test_unordered_list() {
        let result = render_markdown("- item 1\n- item 2");
        assert!(result.contains("item 1"));
        assert!(result.contains("item 2"));
        assert!(result.contains("•"));
    }

    #[test]
    fn test_table() {
        let md = "| A | B |\n|---|---|\n| 1 | 2 |";
        let result = render_markdown(md);
        assert!(result.contains("A"));
        assert!(result.contains("B"));
        assert!(result.contains("1"));
        assert!(result.contains("2"));
    }

    #[test]
    fn test_heading_renders() {
        let result = render_markdown("# Title\n\nContent");
        assert!(result.contains("Title"));
        assert!(result.contains("Content"));
    }

    #[test]
    fn test_horizontal_rule() {
        let result = render_markdown("before\n\n---\n\nafter");
        assert!(result.contains("before"));
        assert!(result.contains("after"));
    }

    #[test]
    fn test_link() {
        let result = render_inline("click [here](https://go-on.dev)");
        assert!(result.contains("click"));
        assert!(result.contains("here"));
    }

    #[test]
    fn test_empty_text() {
        let result = render_markdown("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_blockquote() {
        let result = render_markdown("> quoted text");
        assert!(result.contains("quoted text"));
        assert!(result.contains("│"));
    }
}
