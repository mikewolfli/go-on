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
///
/// Render markdown text to ANSI-colored output (test-only convenience).
///
/// Production code uses `StreamMarkdownRenderer` for incremental rendering.
/// This function is kept under `#[cfg(test)]` as a testing helper.
#[cfg(test)]
pub(crate) fn render_markdown(text: &str) -> String {
    // Pre-allocate ~10% more than input for ANSI escape overhead
    let mut out = String::with_capacity(text.len() + text.len() / 10 + 16);
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
    // Pre-allocate for ANSI overhead
    let mut out = String::with_capacity(text.len() + text.len() / 10 + 16);
    let s = text;
    let mut pos = 0;

    while pos < s.len() {
        let remaining = &s[pos..];

        // Inline code: `...`
        if let Some(rest) = remaining.strip_prefix('`') {
            pos += 1;
            let end = rest.find('`').map(|i| i + 1).unwrap_or_else(|| rest.len());
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

        // Plain character — compute once to avoid iterating twice
        let c = s[pos..].chars().next().unwrap();
        out.push(c);
        pos += c.len_utf8();
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
/// Cached with OnceLock to avoid spawning a subprocess on every call.
fn terminal_width() -> usize {
    static WIDTH: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *WIDTH.get_or_init(|| {
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
    })
}

/// ANSI helper (same as the macro in chat.rs but as a function for use in this module).
fn ansi(code: &str) -> String {
    format!("\u{001B}[{}m", code)
}

// ── Streaming Renderer ────────────────────────────────────────────────────

/// Streaming markdown renderer that processes tokens incrementally.
///
/// Instead of rendering a complete string at once (like `render_markdown`),
/// this state machine accepts tokens one at a time and outputs ANSI-formatted
/// fragments as soon as complete lines are available.
///
/// # How it works
///
/// 1. Tokens are buffered until a `\n` boundary is reached
/// 2. Complete lines are processed through the same line-based parser as `render_markdown`
/// 3. Code blocks and tables are fully buffered before rendering (they need complete data)
/// 4. Regular lines are rendered and flushed immediately
/// 5. A raw copy of the response is maintained for tool call detection
pub struct StreamMarkdownRenderer {
    /// Buffer for accumulating partial lines between newlines
    line_buf: String,
    /// Raw response text (for tool call detection and follow-up)
    raw_response: String,
    /// State: in code block
    in_code_block: bool,
    /// Code block language
    code_lang: String,
    /// Code block content buffer
    code_content: String,
    /// State: in table
    in_table: bool,
    /// Table column widths
    table_col_widths: Vec<usize>,
    /// Table rows buffer
    table_rows: Vec<Vec<String>>,
    /// Output buffer for completed ANSI fragments
    out_buf: String,
}

impl Default for StreamMarkdownRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamMarkdownRenderer {
    pub fn new() -> Self {
        Self {
            line_buf: String::new(),
            raw_response: String::new(),
            in_code_block: false,
            code_lang: String::new(),
            code_content: String::new(),
            in_table: false,
            table_col_widths: Vec::new(),
            table_rows: Vec::new(),
            out_buf: String::new(),
        }
    }

    /// Feed a token into the renderer. Returns the raw text for tool/reasoning detection.
    pub fn feed(&mut self, token: &str) -> &str {
        self.raw_response.push_str(token);
        self.line_buf.push_str(token);

        // Process all complete lines in the buffer
        while let Some(newline_pos) = self.line_buf.find('\n') {
            let line = self.line_buf[..newline_pos].to_string();
            // Keep the remainder (after \n) in the buffer
            self.line_buf = self.line_buf[newline_pos + 1..].to_string();
            self.process_line(&line);
        }
        self.raw_response.as_str()
    }

    /// Flush any pending output (partial line, code block, table).
    /// Returns (formatted_output, is_complete) where is_complete indicates
    /// whether the renderer has fully flushed (no pending state).
    pub fn flush(&mut self) -> (String, bool) {
        let mut flushed = String::new();
        std::mem::swap(&mut flushed, &mut self.out_buf);

        // Flush pending code block
        if self.in_code_block {
            flushed.push_str(&render_code_block(&self.code_content, &self.code_lang));
            self.code_content.clear();
            self.code_lang.clear();
            self.in_code_block = false;
        }

        // Flush pending table
        if self.in_table {
            flushed.push_str(&render_table(&self.table_rows, &self.table_col_widths));
            self.in_table = false;
            self.table_rows.clear();
            self.table_col_widths.clear();
        }

        // Flush partial line
        if !self.line_buf.is_empty() {
            let line = std::mem::take(&mut self.line_buf);
            Self::render_regular_line(&line, &mut flushed);
            flushed.push('\n');
        }

        (
            flushed,
            self.line_buf.is_empty() && !self.in_code_block && !self.in_table,
        )
    }

    /// Consume the raw response and reset it.
    pub fn take_raw_response(&mut self) -> String {
        std::mem::take(&mut self.raw_response)
    }

    fn process_line(&mut self, line: &str) {
        // ── Code block fences ──
        if line.trim_start().starts_with("```") {
            if self.in_code_block {
                // End code block — render accumulated content
                let rendered = render_code_block(&self.code_content, &self.code_lang);
                self.out_buf.push_str(&rendered);
                self.code_content.clear();
                self.code_lang.clear();
                self.in_code_block = false;
            } else {
                // Start code block
                self.in_code_block = true;
                self.code_lang = line
                    .trim_start()
                    .trim_start_matches("```")
                    .trim()
                    .to_string();
            }
            return;
        }

        if self.in_code_block {
            self.code_content.push_str(line);
            self.code_content.push('\n');
            return;
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
                return; // skip separator row
            }

            if !self.in_table {
                self.in_table = true;
                self.table_rows.clear();
                self.table_col_widths = cells.iter().map(|c| c.chars().count()).collect();
            } else {
                for (i, cell) in cells.iter().enumerate() {
                    if i < self.table_col_widths.len() {
                        self.table_col_widths[i] =
                            self.table_col_widths[i].max(cell.chars().count());
                    }
                }
            }
            self.table_rows
                .push(cells.iter().map(|s| s.to_string()).collect());
            return;
        } else if self.in_table {
            // End of table — render it
            let rendered = render_table(&self.table_rows, &self.table_col_widths);
            self.out_buf.push_str(&rendered);
            self.in_table = false;
            self.table_rows.clear();
            self.table_col_widths.clear();
        }

        Self::render_regular_line(line, &mut self.out_buf);
    }

    fn render_regular_line(trimmed: &str, out: &mut String) {
        // ── Horizontal rules ──
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            out.push_str(&format!(
                "{}{}{}\n",
                ansi("90"),
                "─".repeat(terminal_width().min(60)),
                ansi("0")
            ));
            return;
        }

        // ── Headings ──
        if let Some(level) = heading_level(trimmed) {
            let content = trimmed.trim_start_matches('#').trim();
            let color = match level {
                1 => "1;36",
                2 => "1;34",
                3 => "1;33",
                _ => "1;90",
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
            return;
        }

        // ── Blockquotes ──
        if let Some(content) = trimmed.strip_prefix('>') {
            out.push_str(&format!(
                " {}│ {}\n",
                ansi("90"),
                render_inline(content.trim())
            ));
            return;
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
            return;
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
                return;
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
