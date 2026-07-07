use super::super::*;
use crate::patch::PatchHunk;
use crate::repl::buffer::LineStyle;
use crate::repl::Mode;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crossterm::{
    cursor, queue,
    style::{self, Attribute, Color, Print, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal::{self, ClearType},
};
use similar::{ChangeTag, TextDiff};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

thread_local! {
    static MERGE_UNDO_STACK: RefCell<HashMap<String, Vec<String>>> = RefCell::new(HashMap::new());
}

fn push_undo(filename: &str, content: &str) {
    MERGE_UNDO_STACK.with(|s| {
        s.borrow_mut()
            .entry(filename.to_string())
            .or_default()
            .push(content.to_string());
    });
}

fn pop_undo(filename: &str) -> Option<String> {
    MERGE_UNDO_STACK.with(|s| s.borrow_mut().get_mut(filename).and_then(|v| v.pop()))
}

#[derive(Clone)]
pub(crate) enum DiffRow {
    Equal(String),
    Delete(String),
    Insert(String),
    Modified(String, String),
}

pub(crate) fn build_diff_rows(search: &[String], replace: &[String]) -> Vec<DiffRow> {
    let search_text = search.join("\n");
    let replace_text = replace.join("\n");
    let diff = TextDiff::from_lines(&search_text, &replace_text);

    let mut rows = Vec::new();
    let mut dels: Vec<String> = Vec::new();
    let mut inss: Vec<String> = Vec::new();

    fn flush(rows: &mut Vec<DiffRow>, dels: &mut Vec<String>, inss: &mut Vec<String>) {
        let n = dels.len().min(inss.len());
        for i in 0..n {
            rows.push(DiffRow::Modified(dels[i].clone(), inss[i].clone()));
        }
        for d in dels.drain(n..) {
            rows.push(DiffRow::Delete(d));
        }
        for i in inss.drain(n..) {
            rows.push(DiffRow::Insert(i));
        }
    }

    for change in diff.iter_all_changes() {
        let line = change.to_string().trim_end_matches('\n').to_string();
        match change.tag() {
            ChangeTag::Equal => {
                flush(&mut rows, &mut dels, &mut inss);
                rows.push(DiffRow::Equal(line));
            }
            ChangeTag::Delete => dels.push(line),
            ChangeTag::Insert => inss.push(line),
        }
    }
    flush(&mut rows, &mut dels, &mut inss);
    rows
}
/// Compares `search` (hunk search block) against `matched` (the current best-guess
/// block in the file) and returns a per-`matched`-line gutter char:
/// '=' equal, '~' modified, '+' extra line only in file, plus equal/total counts
/// used to compute a match percentage.
pub(crate) fn build_match_gutter(search: &[String], matched: &[String]) -> (Vec<char>, usize, usize) {
    let rows = build_diff_rows(search, matched);
    let mut gutter = Vec::with_capacity(matched.len());
    let mut equal_count = 0usize;
    let mut total = 0usize;
    for row in &rows {
        match row {
            DiffRow::Equal(_) => {
                gutter.push('=');
                equal_count += 1;
                total += 1;
            }
            DiffRow::Modified(_, _) => {
                gutter.push('~');
                total += 1;
            }
            DiffRow::Insert(_) => {
                gutter.push('+');
                total += 1;
            }
            DiffRow::Delete(_) => {
                // line only in search (missing from file) - no matched line to attach to
                total += 1;
            }
        }
    }
    (gutter, equal_count, total)
}
pub(crate) fn word_diff(old: &str, new: &str) -> (Vec<(String, bool)>, Vec<(String, bool)>) {
    let diff = TextDiff::from_words(old, new);
    let mut left = Vec::new();
    let mut right = Vec::new();
    for change in diff.iter_all_changes() {
        let s = change.to_string();
        match change.tag() {
            ChangeTag::Equal => {
                left.push((s.clone(), false));
                right.push((s, false));
            }
            ChangeTag::Delete => left.push((s, true)),
            ChangeTag::Insert => right.push((s, true)),
        }
    }
    (left, right)
}

fn render_spans(
    stdout: &mut io::Stdout,
    spans: &[(String, bool)],
    max_width: usize,
    normal_color: Color,
    highlight_color: Color,
) -> anyhow::Result<()> {
    let mut used = 0usize;
    'outer: for (text, changed) in spans {
        for g in text.graphemes(true) {
            let gw = UnicodeWidthStr::width(g);
            if used + gw > max_width {
                break 'outer;
            }
            let (fg, bg) = if *changed {
                (Color::Black, highlight_color)
            } else {
                (normal_color, Color::Black)
            };
            queue!(
                stdout,
                SetBackgroundColor(bg),
                SetForegroundColor(fg),
                Print(g)
            )?;
            used += gw;
        }
    }
    if used < max_width {
        queue!(
            stdout,
            SetBackgroundColor(Color::Black),
            Print(" ".repeat(max_width - used))
        )?;
    }
    Ok(())
}

impl Repl {
    fn build_right_rows(hunk: &PatchHunk) -> Vec<(String, Color, bool)> {
        let fold = 10usize;
        let mut right_rows: Vec<(String, Color, bool)> = Vec::new();
        right_rows.push(("<<<<<<< SEARCH".to_string(), Color::Magenta, true));
        let search = &hunk.search;
        if search.len() <= fold * 2 {
            for l in search {
                right_rows.push((l.clone(), Color::Red, false));
            }
        } else {
            for l in &search[..fold] {
                right_rows.push((l.clone(), Color::Red, false));
            }
            right_rows.push((
                format!("... placeholder ({}) ...", search.len() - fold * 2),
                Color::DarkGrey,
                false,
            ));
            for l in &search[search.len() - fold..] {
                right_rows.push((l.clone(), Color::Red, false));
            }
        }
        right_rows.push(("=======".to_string(), Color::Magenta, true));
        let replace = &hunk.replace;
        if replace.len() <= fold * 2 {
            for l in replace {
                right_rows.push((l.clone(), Color::Green, false));
            }
        } else {
            for l in &replace[..fold] {
                right_rows.push((l.clone(), Color::Green, false));
            }
            right_rows.push((
                format!("... placeholder ({}) ...", replace.len() - fold * 2),
                Color::DarkGrey,
                false,
            ));
            for l in &replace[replace.len() - fold..] {
                right_rows.push((l.clone(), Color::Green, false));
            }
        }
        right_rows.push((">>>>>>> REPLACE".to_string(), Color::Magenta, true));
        right_rows
    }
    fn get_right_rows(&self, hunk: &PatchHunk) -> Vec<(String, Color, bool)> {
        Self::build_right_rows(hunk)
    }

    fn calc_right_rows_len(hunk: &PatchHunk) -> usize {
        let fold = 10usize;
        let search_len = if hunk.search.len() <= fold * 2 {
            hunk.search.len()
        } else {
            fold * 2 + 1
        };
        let replace_len = if hunk.replace.len() <= fold * 2 {
            hunk.replace.len()
        } else {
            fold * 2 + 1
        };
        3 + search_len + replace_len
    }
    pub fn start_merge(&mut self, hunks: Vec<PatchHunk>) {
        if hunks.is_empty() {
            return;
        }
        for h in &hunks {
            MERGE_UNDO_STACK.with(|s| {
                s.borrow_mut().remove(&h.filename);
            });
        }
        // Remove this line - merge_rows is not used
        // self.merge_rows = build_diff_rows(&hunks[0].search, &hunks[0].replace);
        self.pending_merge = Some(hunks);
        self.merge_index = 0;
        self.merge_scroll = 0;
        self.merge_left_active = true;
        self.merge_right_cursor = 0;
        self.merge_search_query = None;
        self.calc_merge_file_scroll();
        self.mode = Mode::Merge;
        let idx = self.llm_buffer_idx();
        self.active_buffer = idx;
        self.push_info(
            "  🔀 Entering Merge Mode. [a]pply [r]eject  ma/mA set  [q]uit",
            LineStyle::Info,
        );
    }

    fn calc_merge_file_scroll(&mut self) {
        let hunk = &self.pending_merge.as_ref().unwrap()[self.merge_index];
        let project_root = std::path::PathBuf::from(&self.config.tools.project_root);
        let file_path = project_root.join(&hunk.filename);
        let file_content = if let Some(idx) = self.buffers.iter().position(|b| b.name() == hunk.filename) {
            self.buffers[idx].lines().iter().map(|l| l.content().clone()).collect::<Vec<String>>().join("\n")
        } else {
            std::fs::read_to_string(&file_path).unwrap_or_default()
        };
        let file_lines: Vec<String> = file_content.lines().map(String::from).collect();

        let search = &hunk.search;
        let mut best_match_idx = 0;

        if !search.is_empty() && !file_lines.is_empty() {
            let mut max_score = 0;
            // Use a sliding window to find the best match location
            for i in 0..=file_lines.len().saturating_sub(search.len()) {
                let mut score = 0;
                for j in 0..search.len() {
                    if i + j < file_lines.len()
                        && file_lines[i + j].trim_end() == search[j].trim_end()
                    {
                        score += 1;
                    }
                }
                if score > max_score {
                    max_score = score;
                    best_match_idx = i;
                }
                // If exact match is found, break early
                if max_score == search.len() {
                    break;
                }
            }
        }

        self.merge_match_idx = best_match_idx;
        self.merge_match_end = best_match_idx + search.len().max(1);
        self.merge_cursor = best_match_idx;
        self.merge_file_scroll = best_match_idx.saturating_sub(2);
    }

    pub(super) fn handle_merge_key(
        &mut self,
        key: KeyEvent,
        stdout: &mut io::Stdout,
    ) -> anyhow::Result<()> {
        if self.pending_merge.is_none() {
            self.mode = Mode::Insert;
            return Ok(());
        }
        if self.pending == Some('m') {
            self.pending = None;
            match key.code {
                KeyCode::Char('a') => {
                    self.merge_match_idx = self.merge_cursor;
                    if self.merge_match_end <= self.merge_match_idx {
                        self.merge_match_end = self.merge_match_idx + 1;
                    }
                }
                KeyCode::Char('A') => {
                    self.merge_match_end = self.merge_cursor + 1;
                    if self.merge_match_idx >= self.merge_match_end {
                        self.merge_match_idx = self.merge_match_end.saturating_sub(1);
                    }
                }
                _ => {}
            }
            self.render(stdout)?;
            return Ok(());
        }
        match key.code {
            KeyCode::F(9) => {
                self.merge_buffer_apply = !self.merge_buffer_apply;
                if self.merge_buffer_apply {
                    self.push_info("  🔀 Buffer Mode ON. [a] will apply to buffer (Alt-w to save).", LineStyle::Info);
                } else {
                    self.push_info("  🔀 Buffer Mode OFF. [a] will apply to file.", LineStyle::Info);
                }
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                let hunk = self.pending_merge.as_ref().unwrap()[self.merge_index].clone();
                let project_root = std::path::PathBuf::from(&self.config.tools.project_root);
                let file_path = project_root.join(&hunk.filename);

                if self.merge_buffer_apply {
                    let temp_path = file_path.with_extension("codex_eyes_mergetmp");
                    let file_content = if let Some(idx) = self.buffers.iter().position(|b| b.name() == hunk.filename) {
                        self.buffers[idx].lines().iter().map(|l| l.content().clone()).collect::<Vec<String>>().join("\n")
                    } else {
                        std::fs::read_to_string(&file_path).unwrap_or_default()
                    };
                    let _ = std::fs::write(&temp_path, &file_content);
                    
                    let temp_filename = temp_path.to_string_lossy().to_string();
                    let patch_text = format!(
                        "<<<<<<< SEARCH\n{}\n=======\n{}\n>>>>>>> REPLACE",
                        hunk.search.join("\n"),
                        hunk.replace.join("\n")
                    );
                    match crate::patch::apply_patch(&temp_filename, &patch_text, &project_root, &self.config.tools.allow_paths) {
                        Ok(_) => {
                            if let Ok(modified_content) = std::fs::read_to_string(&temp_path) {
                                let buf_idx = if let Some(idx) = self.buffers.iter().position(|b| b.name() == hunk.filename) {
                                    idx
                                } else {
                                    let idx = self.buffers.len();
                                    self.buffers.push(ResponseBuffer::with_name(&hunk.filename));
                                    idx
                                };
                                self.active_buffer = buf_idx;
                                self.buffers[buf_idx].clear();
                                self.buffers[buf_idx].push_str(&modified_content, LineStyle::Plain);
                                self.modified_buffers.insert(hunk.filename.clone());
                                self.push_info("  ✅ Applied to buffer. Press Alt-w to save.", LineStyle::ToolResult);
                                self.next_merge();
                            }
                        }
                        Err(e) => {
                            self.push_info(format!("  ❌ Merge to buffer failed: {}", e), LineStyle::Error);
                        }
                    }
                    let _ = std::fs::remove_file(&temp_path);
                } else {
                    if let Ok(file_content) = std::fs::read_to_string(&file_path) {
                        push_undo(&hunk.filename, &file_content);
                    }
                    let patch_text = format!(
                        "<<<<<<< SEARCH\n{}\n=======\n{}\n>>>>>>> REPLACE",
                        hunk.search.join("\n"),
                        hunk.replace.join("\n")
                    );
                    match crate::patch::apply_patch(
                        &hunk.filename,
                        &patch_text,
                        &project_root,
                        &self.config.tools.allow_paths,
                    ) {
                        Ok(msg) => self.push_info(format!("  ✅ {}", msg), LineStyle::ToolResult),
                        Err(e) => self.push_info(format!("  ❌ Merge failed: {}", e), LineStyle::Error),
                    }
                    self.next_merge();
                }
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.push_info("  🚫 Rejected hunk.", LineStyle::Error);
                self.next_merge();
            }
            // KeyCode::Char('n') | KeyCode::Char('N') => {
            // self.next_merge();
            // }
            KeyCode::Tab => {
                self.merge_left_active = !self.merge_left_active;
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                if self.merge_index > 0 {
                    self.merge_index -= 1;
                    self.merge_scroll = 0;
                    self.merge_right_cursor = 0;
                    self.merge_search_query = None;
                    self.calc_merge_file_scroll();
                }
            }
            KeyCode::Char('l') | KeyCode::Char('L') => {
                let len = self.pending_merge.as_ref().unwrap().len();
                if len > 1 {
                    self.merge_index = (self.merge_index + 1) % len;
                    self.merge_scroll = 0;
                    self.merge_right_cursor = 0;
                    self.merge_search_query = None;
                    self.calc_merge_file_scroll();
                    self.push_info(
                        format!(
                            "  ⏭️ Skipped to hunk [{}/{}] (no change applied).",
                            self.merge_index + 1,
                            len
                        ),
                        LineStyle::Dim,
                    );
                } else {
                    self.push_info("  ⏭️ Only 1 hunk, nothing to skip to.", LineStyle::Dim);
                }
            }
            KeyCode::Char('m') => {
                self.pending = Some('m');
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                // Goto the hunk: recenter left panel & cursor on the current match block
                self.merge_cursor = self.merge_match_idx;
                self.merge_file_scroll = self.merge_match_idx.saturating_sub(2);
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if self.merge_left_active {
                    self.merge_cursor += 1;
                } else {
                    let hunk = &self.pending_merge.as_ref().unwrap()[self.merge_index];
                    let right_rows_len = Self::calc_right_rows_len(hunk);
                    if self.merge_right_cursor < right_rows_len.saturating_sub(1) {
                        self.merge_right_cursor += 1;
                    }
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.merge_left_active {
                    self.merge_cursor = self.merge_cursor.saturating_sub(1);
                } else {
                    self.merge_right_cursor = self.merge_right_cursor.saturating_sub(1);
                }
            }
            KeyCode::Char('J') => {
                if self.merge_left_active {
                    self.merge_match_end += 1;
                } else {
                    let vis = self.response_area_height().saturating_sub(2);
                    self.merge_scroll += vis;
                }
            }
            KeyCode::Char('K') => {
                if self.merge_left_active {
                    if self.merge_match_end > self.merge_match_idx + 1 {
                        self.merge_match_end -= 1;
                    }
                } else {
                    let vis = self.response_area_height().saturating_sub(2);
                    self.merge_scroll = self.merge_scroll.saturating_sub(vis);
                }
            }
            KeyCode::PageDown => {
                let vis = self.response_area_height().saturating_sub(2);
                if self.merge_left_active {
                    self.merge_file_scroll += vis;
                    self.merge_cursor += vis;
                } else {
                    self.merge_scroll += vis;
                    let hunk = &self.pending_merge.as_ref().unwrap()[self.merge_index];
                    let right_rows_len = Self::calc_right_rows_len(hunk);
                    self.merge_right_cursor =
                        (self.merge_right_cursor + vis).min(right_rows_len.saturating_sub(1));
                }
            }
            KeyCode::PageUp => {
                let vis = self.response_area_height().saturating_sub(2);
                if self.merge_left_active {
                    self.merge_file_scroll = self.merge_file_scroll.saturating_sub(vis);
                    self.merge_cursor = self.merge_cursor.saturating_sub(vis);
                } else {
                    self.merge_scroll = self.merge_scroll.saturating_sub(vis);
                    self.merge_right_cursor = self.merge_right_cursor.saturating_sub(vis);
                }
            }
            KeyCode::Enter => {
                let hunk = self.pending_merge.as_ref().unwrap()[self.merge_index].clone();
                let project_root = std::path::PathBuf::from(&self.config.tools.project_root);
                let file_path = project_root.join(&hunk.filename);
                let file_content = std::fs::read_to_string(&file_path).unwrap_or_default();
                let file_lines: Vec<String> = file_content.lines().map(String::from).collect();

                let mut target_line: Option<String> = None;
                let start_idx = self.merge_cursor + 1;

                if !self.merge_left_active {
                    let cursor = self.merge_right_cursor;
                    let right_rows = self.get_right_rows(&hunk);
                    if cursor < right_rows.len() {
                        let (text, _color, is_marker) = &right_rows[cursor];
                        if !*is_marker && !text.starts_with("... placeholder") {
                            target_line = Some(text.clone());
                        }
                    }

                    if let Some(line) = &target_line {
                        self.merge_search_query = Some(line.clone());
                    } else {
                        self.merge_search_query = None;
                        self.push_info(
                            "  Cannot search for marker or placeholder.",
                            LineStyle::Error,
                        );
                        self.render(stdout)?;
                        return Ok(());
                    }
                } else {
                    target_line = self.merge_search_query.clone();
                    if target_line.is_none() {
                        self.push_info(
                            "  No search query. Select a line on right panel.",
                            LineStyle::Error,
                        );
                        self.render(stdout)?;
                        return Ok(());
                    }
                }

                if let Some(line) = target_line {
                    let mut found_idx = None;
                    for i in start_idx..file_lines.len() {
                        if file_lines[i].trim_end() == line.trim_end() {
                            found_idx = Some(i);
                            break;
                        }
                    }
                    if found_idx.is_none() {
                        let end = start_idx.min(file_lines.len());
                        for i in 0..end {
                            if file_lines[i].trim_end() == line.trim_end() {
                                found_idx = Some(i);
                                break;
                            }
                        }
                    }

                    if let Some(idx) = found_idx {
                        self.merge_cursor = idx;
                        self.merge_match_idx = idx;
                        self.merge_match_end = idx + 1;
                        self.push_info(format!("  Matched line at {}", idx + 1), LineStyle::Info);
                    } else {
                        self.push_info("  No match found.", LineStyle::Error);
                    }
                }
            }
            KeyCode::Char('o') => {
                let hunk = self.pending_merge.as_ref().unwrap()[self.merge_index].clone();
                let project_root = std::path::PathBuf::from(&self.config.tools.project_root);
                let file_path = project_root.join(&hunk.filename);
                let file_content = std::fs::read_to_string(&file_path).unwrap_or_default();

                push_undo(&hunk.filename, &file_content);

                let mut file_lines: Vec<String> = file_content.lines().map(String::from).collect();

                let line_idx = (self.merge_cursor + 1).min(file_lines.len());
                file_lines.insert(line_idx, String::new());
                std::fs::write(&file_path, file_lines.join("\n") + "\n")?;

                if self.merge_cursor < self.merge_match_idx {
                    self.merge_match_idx += 1;
                    self.merge_match_end += 1;
                } else if self.merge_cursor < self.merge_match_end {
                    self.merge_match_end += 1;
                }
                self.merge_cursor = line_idx;
            }
            KeyCode::Char('d') => {
                let hunk = self.pending_merge.as_ref().unwrap()[self.merge_index].clone();
                let project_root = std::path::PathBuf::from(&self.config.tools.project_root);
                let file_path = project_root.join(&hunk.filename);
                let file_content = std::fs::read_to_string(&file_path).unwrap_or_default();

                let mut file_lines: Vec<String> = file_content.lines().map(String::from).collect();

                if !file_lines.is_empty() {
                    push_undo(&hunk.filename, &file_content);

                    let line_idx = self.merge_cursor.min(file_lines.len() - 1);
                    file_lines.remove(line_idx);
                    std::fs::write(&file_path, file_lines.join("\n") + "\n")?;

                    if self.merge_match_idx > line_idx {
                        self.merge_match_idx = self.merge_match_idx.saturating_sub(1);
                        self.merge_match_end = self.merge_match_end.saturating_sub(1);
                    } else if self.merge_match_end > line_idx {
                        self.merge_match_end = self.merge_match_end.saturating_sub(1);
                    }
                    if self.merge_cursor >= file_lines.len() {
                        self.merge_cursor = file_lines.len().saturating_sub(1);
                    }
                    self.push_info("  🗑️ Line deleted.", LineStyle::Info);
                }
            }
            KeyCode::Char('u') => {
                let hunk = self.pending_merge.as_ref().unwrap()[self.merge_index].clone();
                let project_root = std::path::PathBuf::from(&self.config.tools.project_root);
                let file_path = project_root.join(&hunk.filename);

                if let Some(undo_content) = pop_undo(&hunk.filename) {
                    std::fs::write(&file_path, &undo_content)?;
                    self.push_info("  ↩️ Undo successful.", LineStyle::Info);
                    let old_cursor = self.merge_cursor;
                    self.calc_merge_file_scroll();
                    self.merge_cursor = old_cursor;
                } else {
                    self.push_info("  Nothing to undo.", LineStyle::Error);
                }
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => {
                self.pending_merge = None;
                self.mode = Mode::Insert;
                self.push_info("  Exited Merge Mode.", LineStyle::Dim);
            }
            _ => {}
        }
        self.render(stdout)?;
        Ok(())
    }

    fn next_merge(&mut self) {
        if self.merge_index + 1 < self.pending_merge.as_ref().unwrap().len() {
            self.merge_index += 1;
            self.merge_scroll = 0;
            self.merge_right_cursor = 0;
            self.merge_search_query = None;
            self.calc_merge_file_scroll();
        } else {
            if self.merge_buffer_apply {
                self.push_info(
                    "  ✅ All hunks processed. Press Alt-w to save & exit.",
                    LineStyle::Info,
                );
            } else {
                self.pending_merge = None;
                self.mode = Mode::Insert;
                self.push_info(
                    "  ✅ All hunks processed. Exited Merge Mode.",
                    LineStyle::Info,
                );
            }
        }
    }

    pub(crate) fn render_merge(&mut self, stdout: &mut io::Stdout) -> anyhow::Result<()> {
        let ra_height = self.response_area_height();
        let term_width = self.term_width();
        let split_x = (term_width * 6) / 10; // 6:4 ratio (file left, patch right)

        let hunks = match &self.pending_merge {
            Some(h) => h,
            None => return Ok(()),
        };
        let hunk = &hunks[self.merge_index];

        for i in 0..ra_height {
            queue!(
                stdout,
                cursor::MoveTo(0, i as u16),
                terminal::Clear(ClearType::CurrentLine)
            )?;
        }

        let left_width = split_x.saturating_sub(1);
        let right_width = term_width.saturating_sub(split_x).saturating_sub(1);

        fn trunc(s: &str, max_w: usize) -> String {
            if UnicodeWidthStr::width(s) <= max_w {
                return s.to_string();
            }
            let mut w = 0;
            let mut out = String::new();
            for g in s.graphemes(true) {
                let gw = UnicodeWidthStr::width(g);
                if w + gw + 3 > max_w {
                    break;
                }
                out.push_str(g);
                w += gw;
            }
            out.push_str("...");
            out
        }

        // ── header row: description (left) | filename (right) ──
        // ── header row: description (left) | filename (right) ──
        let desc = " detail full content sync the line to left";
        let desc_disp = trunc(desc, left_width.saturating_sub(1));
        let desc_pad = left_width.saturating_sub(UnicodeWidthStr::width(desc_disp.as_str()) + 1);
        let fname_disp = trunc(&hunk.filename, right_width.saturating_sub(1));
        let fname_pad = right_width.saturating_sub(UnicodeWidthStr::width(fname_disp.as_str()) + 1);

        let left_hdr_bg = if self.merge_left_active {
            Color::Cyan
        } else {
            Color::DarkGrey
        };
        let left_hdr_fg = if self.merge_left_active {
            Color::Black
        } else {
            Color::White
        };
        let right_hdr_bg = if !self.merge_left_active {
            Color::Cyan
        } else {
            Color::DarkGrey
        };
        let right_hdr_fg = if !self.merge_left_active {
            Color::Black
        } else {
            Color::Cyan
        };

        queue!(
            stdout,
            cursor::MoveTo(0, 0),
            SetBackgroundColor(left_hdr_bg),
            SetForegroundColor(left_hdr_fg),
            SetAttribute(Attribute::Bold),
            Print(format!(" {}{}", desc_disp, " ".repeat(desc_pad))),
            cursor::MoveTo(split_x as u16, 0),
            SetBackgroundColor(Color::DarkGrey),
            SetForegroundColor(Color::DarkGrey),
            Print("│"),
            cursor::MoveTo((split_x + 1) as u16, 0),
            SetBackgroundColor(right_hdr_bg),
            SetForegroundColor(right_hdr_fg),
            Print(format!(" {}{}", fname_disp, " ".repeat(fname_pad))),
            style::ResetColor,
            SetAttribute(Attribute::Reset)
        )?;

        // ── build right panel rows (the patch) ──────────────────
        let right_rows: Vec<(String, Color, bool)> = Self::build_right_rows(hunk);
        let project_root = std::path::PathBuf::from(&self.config.tools.project_root);
        let file_path = project_root.join(&hunk.filename);
        let file_content = if let Some(idx) = self.buffers.iter().position(|b| b.name() == hunk.filename) {
            self.buffers[idx].lines().iter().map(|l| l.content().clone()).collect::<Vec<String>>().join("\n")
        } else {
            std::fs::read_to_string(&file_path).unwrap_or_default()
        };
        let file_lines: Vec<String> = file_content.lines().map(String::from).collect();

        if self.merge_match_end <= self.merge_match_idx {
            self.merge_match_end = self.merge_match_idx + 1;
        }
        if !file_lines.is_empty() && self.merge_cursor >= file_lines.len() {
            self.merge_cursor = file_lines.len() - 1;
        }
        let actual_match_idx = self.merge_match_idx;
        let matched_end = self.merge_match_end.min(file_lines.len());
        let cursor_line = self.merge_cursor;
        // ── LCS diff: search block vs the currently matched block in the file ──
        let matched_lines: Vec<String> = if actual_match_idx < matched_end {
            file_lines[actual_match_idx..matched_end].to_vec()
        } else {
            Vec::new()
        };
        let (match_gutter, equal_count, total_cmp) =
            build_match_gutter(&hunk.search, &matched_lines);
        let match_percent = if total_cmp > 0 {
            (equal_count * 100) / total_cmp
        } else {
            100
        };
        // ── status row ─────────────────────────────────────────
        queue!(
            stdout,
            cursor::MoveTo(0, 1),
            SetBackgroundColor(Color::DarkGrey),
            SetForegroundColor(Color::Yellow),
            Print(format!(
                " 🔀 [{}/{}] match:{}%  [a]pply [r]eject [l]skip [n]goto [q]uit [Tab]panel [ma/mA]set [Enter]search ",
                self.merge_index + 1,
                hunks.len(),
                match_percent
            )),
            style::ResetColor
        )?;

        // ── calculate scroll limits ────────────────────────────
        let start_y = 2;
        let visible_height = ra_height.saturating_sub(start_y);
        let max_scroll = right_rows.len().saturating_sub(visible_height);
        if self.merge_scroll > max_scroll {
            self.merge_scroll = max_scroll;
        }

        if !self.merge_left_active {
            if self.merge_right_cursor < self.merge_scroll {
                self.merge_scroll = self.merge_right_cursor;
            } else if self.merge_right_cursor >= self.merge_scroll + visible_height {
                self.merge_scroll = self.merge_right_cursor + 1 - visible_height;
            }
        }
        if self.merge_scroll > max_scroll {
            self.merge_scroll = max_scroll;
        }

        let max_file_scroll = file_lines.len().saturating_sub(visible_height);
        if self.merge_file_scroll > max_file_scroll {
            self.merge_file_scroll = max_file_scroll;
        }

        // Auto-scroll left panel if the anchor (cursor) moves out of view
        // Auto-scroll left panel if the cursor moves out of view
        if cursor_line < self.merge_file_scroll {
            self.merge_file_scroll = cursor_line.saturating_sub(2);
        } else if cursor_line >= self.merge_file_scroll + visible_height {
            self.merge_file_scroll = cursor_line + 1 - visible_height;
        }
        if self.merge_file_scroll > max_file_scroll {
            self.merge_file_scroll = max_file_scroll;
        }

        let left_panel_content_w = left_width.saturating_sub(7); // 4 line num, 1 cursor mark, 1 diff gutter, 1 padding
        let target_right_w = right_width.saturating_sub(2);

        for i in 0..visible_height {
            let y = (start_y + i) as u16;

            // Left panel (File Content)
            let f_idx = self.merge_file_scroll + i;
            if f_idx < file_lines.len() {
                let line = &file_lines[f_idx];
                let is_in_match = f_idx >= actual_match_idx && f_idx < matched_end;
                let is_cursor = f_idx == cursor_line;
                let line_num = f_idx + 1;
                let cursor_mark = if is_cursor { ">" } else { " " };
                let line_num_str = format!("{:>4}{}", line_num, cursor_mark);
                let diff_char = if is_in_match {
                    match_gutter
                        .get(f_idx - actual_match_idx)
                        .copied()
                        .unwrap_or(' ')
                } else {
                    ' '
                };
                let diff_color = match diff_char {
                    '=' => Color::DarkGrey,
                    '~' => Color::Yellow,
                    '+' => Color::Green,
                    '-' => Color::Red,
                    _ => Color::DarkGrey,
                };
                let disp = trunc(line, left_panel_content_w);
                let pad =
                    left_panel_content_w.saturating_sub(UnicodeWidthStr::width(disp.as_str()));
                let (fg, bg) = if is_cursor {
                    (Color::Black, Color::Cyan)
                } else if is_in_match {
                    (Color::Black, Color::Yellow)
                } else {
                    (Color::White, Color::Black)
                };
                queue!(
                    stdout,
                    cursor::MoveTo(0, y),
                    SetBackgroundColor(bg),
                    SetForegroundColor(Color::DarkGrey),
                    Print(&line_num_str),
                    SetForegroundColor(diff_color),
                    Print(diff_char),
                    SetForegroundColor(fg),
                    Print(format!(" {}{}", disp, " ".repeat(pad))),
                    style::ResetColor
                )?;
            } else {
                queue!(
                    stdout,
                    cursor::MoveTo(0, y),
                    SetBackgroundColor(Color::Black),
                    Print(" ".repeat(left_width))
                )?;
            }

            // Divider
            queue!(
                stdout,
                cursor::MoveTo(split_x as u16, y),
                SetForegroundColor(Color::DarkGrey),
                Print("│"),
                style::ResetColor
            )?;

            // Right panel (Patch)
            let idx = self.merge_scroll + i;
            if idx < right_rows.len() {
                let (text, color, is_marker) = &right_rows[idx];
                let disp = trunc(text, target_right_w);
                let pad = target_right_w.saturating_sub(UnicodeWidthStr::width(disp.as_str()));
                let is_right_cursor = !self.merge_left_active && idx == self.merge_right_cursor;
                let bg = if is_right_cursor {
                    Color::Cyan
                } else if *is_marker {
                    Color::DarkGrey
                } else {
                    Color::Black
                };
                let fg = if is_right_cursor {
                    Color::Black
                } else {
                    *color
                };
                queue!(
                    stdout,
                    cursor::MoveTo((split_x + 1) as u16, y),
                    if *is_marker || is_right_cursor {
                        SetAttribute(Attribute::Bold)
                    } else {
                        SetAttribute(Attribute::Reset)
                    },
                    SetBackgroundColor(bg),
                    SetForegroundColor(fg),
                    Print(format!(" {}{}", disp, " ".repeat(pad))),
                    style::ResetColor,
                    SetAttribute(Attribute::Reset)
                )?;
            } else {
                queue!(
                    stdout,
                    cursor::MoveTo((split_x + 1) as u16, y),
                    SetBackgroundColor(Color::Black),
                    Print(" ".repeat(right_width))
                )?;
            }
        }

        Ok(())
    }
}