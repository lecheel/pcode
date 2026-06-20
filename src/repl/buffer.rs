use crossterm::style::Color;
use unicode_width::UnicodeWidthChar;
#[derive(Debug, Clone)]
pub enum LineStyle {
    User,
    Assistant,
    Tool,
    ToolResult,
    Info,
    Error,
    Dim,
    Plain,
    Separator,
}
#[derive(Debug, Clone)]
pub struct BufferLine {
    pub content: String,
    pub style: LineStyle,
}
impl BufferLine {
    pub fn new(content: impl Into<String>, style: LineStyle) -> Self {
        Self {
            content: content.into(),
            style,
        }
    }
    pub fn fg_color(&self) -> Color {
        match &self.style {
            LineStyle::User => Color::Green,
            LineStyle::Assistant => Color::White,
            LineStyle::Tool => Color::Yellow,
            LineStyle::ToolResult => Color::Cyan,
            LineStyle::Info => Color::Magenta,
            LineStyle::Error => Color::Red,
            LineStyle::Dim => Color::DarkGrey,
            LineStyle::Plain => Color::Grey,
            LineStyle::Separator => Color::DarkCyan,
        }
    }
    pub fn is_bold(&self) -> bool {
        matches!(
            self.style,
            LineStyle::User | LineStyle::Error | LineStyle::Assistant
        )
    }
}
pub struct VisualRow {
    pub logical_line: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub content: String,
    pub fg_color: Color,
    pub is_bold: bool,
}

#[derive(Debug, Clone)]
struct UndoDelete {
    index: usize,
    lines: Vec<BufferLine>,
}

pub struct ResponseBuffer {
    name: String,
    lines: Vec<BufferLine>,
    visual_scroll: usize,
    cursor_line: usize,
    cursor_col: usize,
    undo_stack: Vec<UndoDelete>,
}
impl ResponseBuffer {
    pub fn new() -> Self {
        Self {
            name: "Chat".to_string(),
            lines: Vec::new(),
            visual_scroll: 0,
            cursor_line: 0,
            cursor_col: 0,
            undo_stack: Vec::new(),
        }
    }
    pub fn with_name(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            lines: Vec::new(),
            visual_scroll: 0,
            cursor_line: 0,
            cursor_col: 0,
            undo_stack: Vec::new(),
        }
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }
    pub fn push(&mut self, line: BufferLine) {
        self.lines.push(line);
    }
    pub fn push_str(&mut self, text: &str, style: LineStyle) {
        for line in text.lines() {
            self.push(BufferLine::new(line.to_string(), style.clone()));
        }
    }
    pub fn push_separator(&mut self) {
        self.push(BufferLine::new("─".repeat(60), LineStyle::Separator));
    }
    pub fn push_blank(&mut self) {
        self.push(BufferLine::new("", LineStyle::Plain));
    }
    pub fn lines(&self) -> &[BufferLine] {
        &self.lines
    }
    pub fn len(&self) -> usize {
        self.lines.len()
    }
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
    pub fn cursor_line(&self) -> usize {
        self.cursor_line
    }
    pub fn cursor_col(&self) -> usize {
        self.cursor_col
    }
    pub fn scroll_offset(&self) -> usize {
        self.visual_scroll
    }
    pub fn visual_rows(&self, width: usize) -> Vec<VisualRow> {
        let mut rows = Vec::new();
        if width == 0 {
            return rows;
        }
        for (line_idx, line) in self.lines.iter().enumerate() {
            let chars: Vec<char> = line.content.chars().collect();
            if chars.is_empty() {
                rows.push(VisualRow {
                    logical_line: line_idx,
                    start_col: 0,
                    end_col: 0,
                    content: String::new(),
                    fg_color: line.fg_color(),
                    is_bold: line.is_bold(),
                });
                continue;
            }
            let mut col = 0usize;
            while col < chars.len() {
                let mut current_width = 0;
                let mut end = col;
                let mut last_space = None;
                while end < chars.len() {
                    let ch = chars[end];
                    let ch_w = UnicodeWidthChar::width(ch).unwrap_or(0);
                    if current_width + ch_w > width && end > col {
                        break;
                    }
                    current_width += ch_w;
                    if ch.is_whitespace() {
                        last_space = Some(end + 1);
                    }
                    end += 1;
                }
                if end == chars.len() || current_width <= width {
                } else {
                    if let Some(space) = last_space {
                        if space > col {
                            end = space;
                        }
                    }
                }
                if end == col {
                    end += 1;
                }
                let content: String = chars[col..end].iter().collect();
                rows.push(VisualRow {
                    logical_line: line_idx,
                    start_col: col,
                    end_col: end,
                    content,
                    fg_color: line.fg_color(),
                    is_bold: line.is_bold(),
                });
                col = end;
            }
        }
        rows
    }
    pub fn cursor_visual_row(&self, width: usize) -> usize {
        let rows = self.visual_rows(width);
        for (i, row) in rows.iter().enumerate() {
            if row.logical_line == self.cursor_line {
                let in_range = if row.start_col == row.end_col {
                    self.cursor_col == 0
                } else {
                    self.cursor_col >= row.start_col && self.cursor_col < row.end_col
                };
                if in_range {
                    return i;
                }
                if self.cursor_col == row.end_col {
                    let is_last =
                        i + 1 >= rows.len() || rows[i + 1].logical_line != self.cursor_line;
                    if is_last {
                        return i;
                    }
                }
            }
        }
        0
    }
    pub fn move_up(&mut self, amount: usize) {
        self.cursor_line = self.cursor_line.saturating_sub(amount);
        self.clamp_cursor_col();
    }
    pub fn move_down(&mut self, amount: usize) {
        if self.lines.is_empty() {
            return;
        }
        self.cursor_line = (self.cursor_line + amount).min(self.lines.len() - 1);
        self.clamp_cursor_col();
    }
    pub fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_line > 0 {
            self.cursor_line -= 1;
            if let Some(line) = self.lines.get(self.cursor_line) {
                self.cursor_col = line.content.chars().count().saturating_sub(1);
            } else {
                self.cursor_col = 0;
            }
        }
    }
    pub fn move_right(&mut self) {
        if self.lines.is_empty() {
            return;
        }
        if let Some(line) = self.lines.get(self.cursor_line) {
            let max = line.content.chars().count().saturating_sub(1);
            if self.cursor_col < max {
                self.cursor_col += 1;
            } else if self.cursor_line < self.lines.len() - 1 {
                self.cursor_line += 1;
                self.cursor_col = 0;
            }
        }
    }
    pub fn set_cursor(&mut self, line: usize, col: usize) {
        if line < self.lines.len() {
            self.cursor_line = line;
            let max = self.lines[line].content.chars().count().saturating_sub(1);
            self.cursor_col = col.min(max);
        } else if self.lines.is_empty() {
            self.cursor_line = 0;
            self.cursor_col = 0;
        }
    }
    pub fn remove_lines(&mut self, start: usize, end: usize) {
        if start < end && end <= self.lines.len() {
            let removed: Vec<BufferLine> = self.lines.drain(start..end).collect();
            self.undo_stack.push(UndoDelete {
                index: start,
                lines: removed,
            });
            if self.cursor_line >= self.lines.len() {
                self.cursor_line = self.lines.len().saturating_sub(1);
            }
            self.clamp_cursor_col();
        }
    }
    pub fn undo(&mut self) -> bool {
        if let Some(action) = self.undo_stack.pop() {
            let UndoDelete { index, lines } = action;
            if index <= self.lines.len() {
                self.cursor_line = index;
                self.cursor_col = 0;
                for (i, line) in lines.into_iter().enumerate() {
                    self.lines.insert(index + i, line);
                }
                self.clamp_cursor_col();
                return true;
            }
        }
        false
    }
    pub fn scroll_to_bottom_view(&mut self, visible_height: usize, width: usize) {
        let rows = self.visual_rows(width);
        if rows.len() > visible_height {
            self.visual_scroll = rows.len() - visible_height;
        } else {
            self.visual_scroll = 0;
        }
    }
    pub fn scroll_to_bottom(&mut self, visible_height: usize, width: usize) {
        self.scroll_to_bottom_view(visible_height, width);
        if !self.lines.is_empty() {
            self.cursor_line = self.lines.len() - 1;
            self.clamp_cursor_col();
        }
    }
    pub fn move_top(&mut self) {
        self.cursor_line = 0;
        self.visual_scroll = 0;
        self.clamp_cursor_col();
    }
    pub fn move_bottom(&mut self, visible_height: usize, width: usize) {
        if self.lines.is_empty() {
            return;
        }
        self.cursor_line = self.lines.len() - 1;
        self.clamp_cursor_col();
        self.scroll_to_bottom(visible_height, width);
    }
    pub fn half_page_up(&mut self, visible_height: usize, width: usize) {
        let amount = (visible_height / 2).max(1);
        self.visual_scroll = self.visual_scroll.saturating_sub(amount);
        let cursor_vrow = self.cursor_visual_row(width);
        if cursor_vrow < self.visual_scroll {
            let rows = self.visual_rows(width);
            if self.visual_scroll < rows.len() {
                self.cursor_line = rows[self.visual_scroll].logical_line;
                self.clamp_cursor_col();
            }
        }
    }
    pub fn half_page_down(&mut self, visible_height: usize, width: usize) {
        let rows = self.visual_rows(width);
        let amount = (visible_height / 2).max(1);
        let max_scroll = rows.len().saturating_sub(visible_height);
        self.visual_scroll = (self.visual_scroll + amount).min(max_scroll);
        let cursor_vrow = self.cursor_visual_row(width);
        if visible_height > 0 && cursor_vrow >= self.visual_scroll + visible_height {
            let target = self.visual_scroll + visible_height - 1;
            if target < rows.len() {
                self.cursor_line = rows[target].logical_line;
                self.cursor_col = rows[target].start_col;
            }
        }
    }
    pub fn ensure_cursor_visible(&mut self, visible_height: usize, width: usize) {
        let cursor_vrow = self.cursor_visual_row(width);
        if cursor_vrow < self.visual_scroll {
            self.visual_scroll = cursor_vrow;
        } else if visible_height > 0 && cursor_vrow >= self.visual_scroll + visible_height {
            self.visual_scroll = cursor_vrow - visible_height + 1;
        }
        let rows = self.visual_rows(width);
        let max_scroll = rows.len().saturating_sub(visible_height);
        self.visual_scroll = self.visual_scroll.min(max_scroll);
    }
    fn clamp_cursor_col(&mut self) {
        if let Some(line) = self.lines.get(self.cursor_line) {
            let max_col = line.content.chars().count().saturating_sub(1);
            self.cursor_col = self.cursor_col.min(max_col);
        } else {
            self.cursor_col = 0;
        }
    }
    pub fn clear(&mut self) {
        self.lines.clear();
        self.visual_scroll = 0;
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.undo_stack.clear();
    }
}
