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
fn undo_stack_len(filename: &str) -> usize {
    MERGE_UNDO_STACK.with(|s| s.borrow().get(filename).map(|v| v.len()).unwrap_or(0))
}
/// True if any file still has undoable history. Used as a "smart dirty"
/// check: `merge_applied`/`modified_buffers` flags can remain stale/true
/// even after the user has undone every change back to the original
/// on-disk content, in which case there is nothing left to lose and we
/// should still allow quitting.
fn any_undo_available() -> bool {
    MERGE_UNDO_STACK.with(|s| s.borrow().values().any(|v| !v.is_empty()))
}
/// True if `key` is an Alt (or Meta, for terminals/OSes that report Option/Cmd
/// as Meta) combo with the given character, matched case-insensitively so
/// callers don't need to check both `Char('d')` and `Char('D')` themselves.
/// Shared so Alt-shortcut detection stays consistent across normal mode,
/// merge mode, and any future mode, instead of each handler rolling its
/// own slightly-different modifier check.
pub(crate) fn is_alt_combo(key: &KeyEvent, ch: char) -> bool {
    (key.modifiers.contains(KeyModifiers::ALT) || key.modifiers.contains(KeyModifiers::META))
        && matches!(key.code, KeyCode::Char(c) if c.eq_ignore_ascii_case(&ch))
}
/// Finds the bracket matching the one under `(start_line, start_col)`,
/// vim `%`-style. Supports `()`, `[]`, `{}` in either direction (opening
/// bracket searches forward, closing bracket searches backward), tracking
/// nesting depth so it skips over any matching inner pairs along the way.
pub(crate) fn find_matching_bracket(
    file_lines: &[String],
    start_line: usize,
    start_col: usize,
) -> Option<(usize, usize)> {
    let line = file_lines.get(start_line)?;
    let graphemes: Vec<&str> = line.graphemes(true).collect();
    if graphemes.is_empty() {
        return None;
    }
    let col = start_col.min(graphemes.len() - 1);
    let open_g = graphemes.get(col)?;
    let open = open_g.chars().next()?;
    let (target_open, target_close, dir) = match open {
        '(' => ('(', ')', 1),
        '[' => ('[', ']', 1),
        '{' => ('{', '}', 1),
        ')' => ('(', ')', -1),
        ']' => ('[', ']', -1),
        '}' => ('{', '}', -1),
        _ => return None,
    };
    let mut depth = 0;
    if dir == 1 {
        for l in start_line..file_lines.len() {
            let line_g: Vec<&str> = file_lines[l].graphemes(true).collect();
            let start_c = if l == start_line { col } else { 0 };
            for c in start_c..line_g.len() {
                let ch = line_g[c].chars().next().unwrap_or(' ');
                if ch == target_open {
                    depth += 1;
                }
                if ch == target_close {
                    depth -= 1;
                    if depth == 0 {
                        return Some((l, c));
                    }
                }
            }
        }
    } else {
        for l in (0..=start_line).rev() {
            let line_g: Vec<&str> = file_lines[l].graphemes(true).collect();
            let end_c = if l == start_line {
                col
            } else {
                line_g.len().saturating_sub(1)
            };
            for c in (0..=end_c).rev() {
                let ch = line_g[c].chars().next().unwrap_or(' ');
                if ch == target_close {
                    depth += 1;
                }
                if ch == target_open {
                    depth -= 1;
                    if depth == 0 {
                        return Some((l, c));
                    }
                }
            }
        }
    }
    None
}
/// Fallback for when `find_best_match`'s strict head/tail boundary
/// requirement rejects every candidate window (common on short search
/// blocks where the differing line falls inside the anchor region).
/// Scores every window in the same size range by raw LCS match count,
/// with no boundary requirement, so we always surface a best-effort
/// match instead of silently giving up.
fn loose_best_match(search: &[String], file: &[String]) -> crate::diff::MatchResult {
    use crate::diff::{build_rows, lcs_diff, MatchResult, RowKind};
    let search_len = search.len();
    if search.is_empty() || file.is_empty() {
        return MatchResult {
            score: 0.0,
            file_start: 0,
            file_end: 0,
            rows: vec![],
            candidates: vec![],
        };
    }
    let min_window = search_len.saturating_sub(5).max(1).min(file.len());
    let max_window = (search_len + 6).min(file.len());
    let mut all_candidates: Vec<(usize, usize, f32)> = Vec::new();
    let mut best_score = -1.0_f32;
    let mut best_start = 0;
    let mut best_end = 0;
    let mut best_raw = Vec::new();
    for window_size in min_window..=max_window {
        if window_size == 0 || window_size > file.len() {
            continue;
        }
        for start in 0..=file.len() - window_size {
            let end = start + window_size;
            let window = &file[start..end];
            let raw = lcs_diff(search, window, false);
            let matched = raw.iter().filter(|(k, _, _)| *k == RowKind::Equal).count();
            let score = (matched as f32 / search_len.max(1) as f32) * 100.0;
            all_candidates.push((start, end, score));
            if score > best_score {
                best_score = score;
                best_start = start;
                best_end = end;
                best_raw = raw;
            }
        }
    }
    let rows = build_rows(&best_raw, 1, best_start + 1);
    all_candidates.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    let mut candidates: Vec<(usize, usize, f32)> = Vec::new();
    for (s, e, sc) in all_candidates {
        if sc < 30.0 {
            continue;
        }
        let overlaps = candidates.iter().any(|(rs, re, _)| s < *re && e > *rs);
        if !overlaps {
            candidates.push((s, e, sc));
        }
    }
    candidates.truncate(20);
    MatchResult {
        score: best_score.clamp(0.0, 100.0),
        file_start: best_start,
        file_end: best_end,
        rows,
        candidates,
    }
}
thread_local! {
    static MERGE_LAST_APPLIED_RANGE: RefCell<HashMap<String, (usize, usize)>> = RefCell::new(HashMap::new());
}
fn set_last_applied_range(filename: &str, start: usize, end: usize) {
    MERGE_LAST_APPLIED_RANGE.with(|s| {
        s.borrow_mut().insert(filename.to_string(), (start, end));
    });
}
fn get_last_applied_range(filename: &str) -> Option<(usize, usize)> {
    MERGE_LAST_APPLIED_RANGE.with(|s| s.borrow().get(filename).copied())
}
fn clear_last_applied_range(filename: &str) {
    MERGE_LAST_APPLIED_RANGE.with(|s| {
        s.borrow_mut().remove(filename);
    });
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
        dels.clear();
        inss.clear();
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
pub(crate) fn build_match_gutter(
    search: &[String],
    matched: &[String],
) -> (Vec<char>, usize, usize) {
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
    fn build_right_rows(hunk: &PatchHunk, applied: bool) -> Vec<(String, Color, bool)> {
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
                format!("        ... ({}) ...", search.len() - fold * 2),
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
                format!("        ... ({}) ...", replace.len() - fold * 2),
                Color::DarkGrey,
                false,
            ));
            for l in &replace[replace.len() - fold..] {
                right_rows.push((l.clone(), Color::Green, false));
            }
        }
        let end_marker = if applied {
            ">>>>>>> APPLIED".to_string()
        } else {
            ">>>>>>> REPLACE".to_string()
        };
        let end_color = if applied { Color::Red } else { Color::Magenta };
        right_rows.push((end_marker, end_color, true));
        right_rows
    }
    fn get_right_rows(&self, hunk: &PatchHunk) -> Vec<(String, Color, bool)> {
        let applied = self
            .merge_applied
            .get(self.merge_index)
            .copied()
            .unwrap_or(false);
        Self::build_right_rows(hunk, applied)
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
    pub fn start_merge(&mut self, mut hunks: Vec<PatchHunk>) {
        hunks.retain(|h| !h.filename.trim().is_empty());
        if hunks.is_empty() {
            self.push_info(
                "  ❌ No valid patches with filenames found.",
                LineStyle::Error,
            );
            return;
        }
        for h in &hunks {
            MERGE_UNDO_STACK.with(|s| {
                s.borrow_mut().remove(&h.filename);
            });
            clear_last_applied_range(&h.filename);
        }
        // Remove this line - merge_rows is not used
        // self.merge_rows = build_diff_rows(&hunks[0].search, &hunks[0].replace);
        self.pending_merge = Some(hunks);
        self.merge_index = 0;
        self.merge_scroll = 0;
        self.merge_left_active = true;
        self.merge_right_cursor = 0;
        self.merge_cursor_col = 0;
        self.merge_search_query = None;
        self.merge_last_modified = None;
        self.merge_applied = vec![false; self.pending_merge.as_ref().unwrap().len()];
        self.merge_last_applied_idx = None;
        self.merge_candidates = Vec::new();
        self.merge_candidate_idx = 0;
        self.calc_merge_file_scroll();
        self.mode = Mode::Merge;
        self.push_info(
            "  🔀 Entering Merge Mode. [a]pply [r]ecalc  ma/mA set  [q]uit",
            LineStyle::Info,
        );
        if self.config.repl.auto_show_merge_summary {
            self.show_merge_summary_popup();
        }
    }

    fn calc_merge_file_scroll(&mut self) {
        let hunk = &self.pending_merge.as_ref().unwrap()[self.merge_index];
        let project_root = std::path::PathBuf::from(&self.config.tools.project_root);
        let file_path = project_root.join(&hunk.filename);
        let file_content =
            if let Some(idx) = self.buffers.iter().position(|b| b.name() == hunk.filename) {
                self.buffers[idx]
                    .lines()
                    .iter()
                    .map(|l| l.content().clone())
                    .collect::<Vec<String>>()
                    .join("\n")
            } else {
                std::fs::read_to_string(&file_path).unwrap_or_default()
            };
        let file_lines: Vec<String> = file_content.lines().map(String::from).collect();
        let search = &hunk.search;

        const NO_MATCH_IDX: usize = usize::MAX / 2;

        if search.is_empty() || file_lines.is_empty() {
            self.merge_candidates = Vec::new();
            self.merge_candidate_idx = 0;
            self.merge_match_idx = NO_MATCH_IDX;
            self.merge_match_end = NO_MATCH_IDX;
            self.merge_cursor = 0;
            self.merge_cursor_col = 0;
            self.merge_file_scroll = 0;
            return;
        }

        // Prefer the boundary-anchored window search from diff.rs: it
        // correctly grows/shrinks the window when lines are inserted or
        // deleted inside the match, instead of assuming a fixed length.
        // But its head/tail anchors must match *exactly*, so on a short
        // search block where the changed line falls inside the anchor
        // region, it can reject every window and return zero candidates
        // even though the block matches well overall. Fall back to a
        // loose, score-only sliding window search in that case so we
        // still surface a best-effort match instead of "match:none".
        let mut result = crate::diff::find_best_match(search, &file_lines, false);
        if result.candidates.is_empty() {
            result = loose_best_match(search, &file_lines);
        }

        self.merge_candidates = result.candidates.iter().map(|(s, e, _)| (*s, *e)).collect();
        self.merge_candidate_idx = 0;

        if result.candidates.is_empty() || result.score <= 0.0 {
            self.merge_match_idx = NO_MATCH_IDX;
            self.merge_match_end = NO_MATCH_IDX;
            self.merge_cursor = 0;
            self.merge_file_scroll = 0;
        } else {
            self.merge_match_idx = result.file_start;
            self.merge_match_end = result.file_end;
            self.merge_cursor = result.file_start;
            self.merge_cursor_col = 0;
            self.merge_file_scroll = result.file_start.saturating_sub(2);
        }
    }

    /// Writes every buffer currently marked as modified to its file on disk,
    /// ensuring a trailing newline, without leaving Merge mode. Used by
    /// Alt-w while a merge is in progress (buffer-apply mode), since the
    /// normal ":w" write command only knows about `self.active_buffer` and
    /// would otherwise exit Merge mode as a side effect.
    pub(crate) fn save_merge_buffers(&mut self, stdout: &mut io::Stdout) -> anyhow::Result<()> {
        if self.modified_buffers.is_empty() {
            self.push_info("  Nothing to save.", LineStyle::Dim);
            self.render(stdout)?;
            return Ok(());
        }
        let project_root = std::path::PathBuf::from(&self.config.tools.project_root);
        let names: Vec<String> = self.modified_buffers.iter().cloned().collect();
        let mut saved = Vec::new();
        for name in names {
            if let Some(idx) = self.buffers.iter().position(|b| b.name() == name) {
                let mut content = self.buffers[idx]
                    .lines()
                    .iter()
                    .map(|l| l.content().clone())
                    .collect::<Vec<String>>()
                    .join("\n");
                let file_path = project_root.join(&name);
                // Preserve the original file's trailing-newline state
                // instead of unconditionally forcing one: only append '\n'
                // if the file on disk already ended with one (or doesn't
                // exist yet, defaulting to POSIX convention). Forcing it
                // regardless of the original state was spuriously adding a
                // trailing newline to files that intentionally have none.
                let had_trailing_newline = std::fs::read_to_string(&file_path)
                    .map(|c| c.ends_with('\n'))
                    .unwrap_or(true);
                if !content.is_empty() && !content.ends_with('\n') && had_trailing_newline {
                    content.push('\n');
                }
                std::fs::write(&file_path, &content)?;
                self.modified_buffers.remove(&name);
                saved.push(name);
            }
        }
        if !saved.is_empty() {
            self.push_info(
                format!("  💾 Saved {} file(s): {}", saved.len(), saved.join(", ")),
                LineStyle::ToolResult,
            );
        } else {
            self.push_info("  Nothing to save.", LineStyle::Dim);
        }
        self.render(stdout)?;
        Ok(())
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

        let is_ctrl_o =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('o');
        let is_alt_w = is_alt_combo(&key, 'w');
        let is_alt_x = is_alt_combo(&key, 'x');
        let is_alt_d = is_alt_combo(&key, 'd');
        let is_alt_q = is_alt_combo(&key, 'q');
        if key.modifiers.contains(KeyModifiers::CONTROL)
            || key.modifiers.contains(KeyModifiers::ALT)
            || key.modifiers.contains(KeyModifiers::META)
        {
            if !is_ctrl_o && !is_alt_w && !is_alt_x && !is_alt_d && !is_alt_q {
                self.render(stdout)?;
                return Ok(());
            }
        }

        if is_alt_q {
            let has_applied = self.merge_applied.iter().any(|&x| x);
            let has_modified = !self.modified_buffers.is_empty();
            let dirty = (has_applied || has_modified) && any_undo_available();
            if !dirty {
                self.editor.save_history(&self.config.repl.history_file);
                self.cmd_editor
                    .save_history(&self.config.repl.command_history_file);
                if let Some(handle) = self.agent_handle.take() {
                    handle.abort();
                }
                return Err(anyhow::anyhow!("__QUIT__"));
            } else {
                self.push_info(
                    "  ⚠️ Cannot quit. Pending changes. Press 'q' to exit merge mode, or Alt-w to save.",
                    LineStyle::Error,
                );
                self.render(stdout)?;
                return Ok(());
            }
        }
        if is_alt_d && self.merge_left_active {
            let hunk = self.pending_merge.as_ref().unwrap()[self.merge_index].clone();
            let project_root = std::path::PathBuf::from(&self.config.tools.project_root);
            let file_path = project_root.join(&hunk.filename);
            let buf_idx = self.buffers.iter().position(|b| b.name() == hunk.filename);
            let is_buffer = self.merge_buffer_apply || buf_idx.is_some();
            let old_content = if let Some(idx) = buf_idx {
                self.buffers[idx]
                    .lines()
                    .iter()
                    .map(|l| l.content().clone())
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                std::fs::read_to_string(&file_path).unwrap_or_default()
            };
            let mut file_lines: Vec<String> = old_content.lines().map(String::from).collect();
            if file_lines.is_empty() {
                self.push_info("  Nothing to delete.", LineStyle::Dim);
                self.render(stdout)?;
                return Ok(());
            }
            let line_idx = self.merge_cursor.min(file_lines.len() - 1);
            push_undo(&hunk.filename, &old_content);
            self.merge_last_modified = Some((hunk.filename.clone(), is_buffer));
            file_lines.remove(line_idx);
            let new_content = if file_lines.is_empty() {
                String::new()
            } else {
                let mut c = file_lines.join("\n");
                if old_content.ends_with('\n') {
                    c.push('\n');
                }
                c
            };
            if let Some(idx) = buf_idx {
                self.buffers[idx].clear();
                self.buffers[idx].push_str(&new_content, LineStyle::Plain);
            } else if self.merge_buffer_apply {
                let idx = self.buffers.len();
                self.buffers.push(ResponseBuffer::with_name(&hunk.filename));
                self.buffers[idx].push_str(&new_content, LineStyle::Plain);
            }
            if !self.merge_buffer_apply {
                std::fs::write(&file_path, &new_content)?;
            }
            self.modified_buffers.insert(hunk.filename.clone());
            // Shift/shrink the match range to account for the removed line.
            if line_idx < self.merge_match_idx {
                self.merge_match_idx = self.merge_match_idx.saturating_sub(1);
                self.merge_match_end = self.merge_match_end.saturating_sub(1);
            } else if line_idx < self.merge_match_end {
                self.merge_match_end = self.merge_match_end.saturating_sub(1);
                if self.merge_match_end <= self.merge_match_idx {
                    self.merge_match_end = self.merge_match_idx + 1;
                }
            }
            let new_len = file_lines.len();
            self.merge_cursor = if new_len == 0 {
                0
            } else {
                line_idx.min(new_len - 1)
            };
            self.merge_cursor_col = 0;
            self.push_info("  🗑️  Deleted line", LineStyle::Dim);
            self.render(stdout)?;
            return Ok(());
        }
        if is_ctrl_o {
            let hunk = self.pending_merge.as_ref().unwrap()[self.merge_index].clone();
            let project_root = std::path::PathBuf::from(&self.config.tools.project_root);
            let file_path = project_root.join(&hunk.filename);
            let line_num = self.merge_cursor + 1;
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());

            let _ = terminal::disable_raw_mode();
            let _ = execute!(stdout, terminal::LeaveAlternateScreen, cursor::Show);
            let _ = stdout.flush();

            let mut cmd = std::process::Command::new(&editor);
            if editor == "code" || editor == "cursor" {
                cmd.arg("-g")
                    .arg(format!("{}:{}", file_path.display(), line_num));
            } else {
                cmd.arg(format!("+{}", line_num)).arg(&file_path);
            }

            match cmd.status() {
                Ok(_) => {
                    if let Ok(content) = std::fs::read_to_string(&file_path) {
                        if let Some(idx) =
                            self.buffers.iter().position(|b| b.name() == hunk.filename)
                        {
                            self.buffers[idx].clear();
                            self.buffers[idx].push_str(&content, LineStyle::Plain);
                        }
                    }
                }
                Err(e) => {
                    self.push_info(
                        format!("  ❌ Failed to open editor: {}", e),
                        LineStyle::Error,
                    );
                }
            }

            let _ = execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide);
            let _ = terminal::enable_raw_mode();
            self.render(stdout)?;
            return Ok(());
        }

        if self.fkey_help {
            self.fkey_help = false;
            self.render(stdout)?;
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
                    self.push_info(
                        "  🔀 Buffer Mode ON. [a] will apply to buffer (Alt-w to save).",
                        LineStyle::Info,
                    );
                } else {
                    self.push_info(
                        "  🔀 Buffer Mode OFF. [a] will apply to file.",
                        LineStyle::Info,
                    );
                }
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                let hunk = self.pending_merge.as_ref().unwrap()[self.merge_index].clone();
                let project_root = std::path::PathBuf::from(&self.config.tools.project_root);
                let file_path = project_root.join(&hunk.filename);
                let file_content = if let Some(idx) =
                    self.buffers.iter().position(|b| b.name() == hunk.filename)
                {
                    self.buffers[idx]
                        .lines()
                        .iter()
                        .map(|l| l.content().clone())
                        .collect::<Vec<String>>()
                        .join("\n")
                } else {
                    std::fs::read_to_string(&file_path).unwrap_or_default()
                };
                let file_lines: Vec<String> = file_content.lines().map(String::from).collect();
                let start = self.merge_match_idx.min(file_lines.len());
                let end = self.merge_match_end.min(file_lines.len());
                let actual_search = if start < end {
                    file_lines[start..end].join("\n")
                } else {
                    hunk.search.join("\n")
                };
                if self.merge_buffer_apply {
                    let temp_path = file_path.with_extension("codex_eyes_mergetmp");
                    push_undo(&hunk.filename, &file_content);
                    self.merge_last_modified = Some((hunk.filename.clone(), true));
                    let _ = std::fs::write(&temp_path, &file_content);
                    let temp_filename = temp_path.to_string_lossy().to_string();
                    let patch_text = format!(
                        "<<<<<<< SEARCH\n{}\n=======\n{}\n>>>>>>> REPLACE",
                        actual_search,
                        hunk.replace.join("\n")
                    );
                    match crate::patch::apply_patch(
                        &temp_filename,
                        &patch_text,
                        &project_root,
                        &self.config.tools.allow_paths,
                    ) {
                        Ok(_) => {
                            if let Ok(modified_content) = std::fs::read_to_string(&temp_path) {
                                let buf_idx = if let Some(idx) =
                                    self.buffers.iter().position(|b| b.name() == hunk.filename)
                                {
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
                                self.push_info(
                                    "  ✅ Applied to buffer. Press Alt-w to save.",
                                    LineStyle::ToolResult,
                                );
                                self.merge_applied[self.merge_index] = true;
                                self.merge_last_applied_idx = Some(self.merge_index);
                                set_last_applied_range(
                                    &hunk.filename,
                                    start,
                                    start + hunk.replace.len(),
                                );
                                self.merge_match_idx = usize::MAX / 2;
                                self.merge_match_end = usize::MAX / 2;
                                self.merge_candidates.clear();
                                self.merge_candidate_idx = 0;
                            }
                        }
                        Err(e) => {
                            let _ = pop_undo(&hunk.filename);
                            self.merge_last_modified = None;
                            self.merge_applied[self.merge_index] = false;
                            self.merge_last_applied_idx = None;
                            self.push_info(
                                format!("  ❌ Merge to buffer failed: {}", e),
                                LineStyle::Error,
                            );
                        }
                    }
                    let _ = std::fs::remove_file(&temp_path);
                } else {
                    if let Ok(disk_content) = std::fs::read_to_string(&file_path) {
                        push_undo(&hunk.filename, &disk_content);
                        self.merge_last_modified = Some((hunk.filename.clone(), false));
                    }
                    let patch_text = format!(
                        "<<<<<<< SEARCH\n{}\n=======\n{}\n>>>>>>> REPLACE",
                        actual_search,
                        hunk.replace.join("\n")
                    );
                    match crate::patch::apply_patch(
                        &hunk.filename,
                        &patch_text,
                        &project_root,
                        &self.config.tools.allow_paths,
                    ) {
                        Ok(msg) => {
                            self.push_info(format!("  ✅ {}", msg), LineStyle::ToolResult);
                            self.merge_applied[self.merge_index] = true;
                            self.merge_last_applied_idx = Some(self.merge_index);
                            set_last_applied_range(
                                &hunk.filename,
                                start,
                                start + hunk.replace.len(),
                            );
                            self.merge_match_idx = usize::MAX / 2;
                            self.merge_match_end = usize::MAX / 2;
                            self.merge_candidates.clear();
                            self.merge_candidate_idx = 0;
                        }
                        Err(e) => {
                            let _ = pop_undo(&hunk.filename);
                            self.merge_last_modified = None;
                            self.merge_applied[self.merge_index] = false;
                            self.merge_last_applied_idx = None;
                            self.push_info(format!("  ❌ Merge failed: {}", e), LineStyle::Error)
                        }
                    }
                }
            }
            KeyCode::Char('?') => {
                self.fkey_help = !self.fkey_help;
            }
            KeyCode::Char('w') => {
                // Save all modified buffers to disk without leaving Merge
                // mode, same as Alt-w. Kept separate from Alt-w so both the
                // modifier and plain-key forms work identically here.
                self.save_merge_buffers(stdout)?;
                return Ok(());
            }
            KeyCode::Char('r') => {
                self.calc_merge_file_scroll();
                self.push_info(
                    "  🔄 Recalculated best-match block from scratch.",
                    LineStyle::Info,
                );
            }
            KeyCode::Char('s') => {
                self.show_merge_summary_popup();
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                if self.merge_candidates.is_empty() {
                    self.push_info("  No candidates found.", LineStyle::Error);
                } else {
                    self.merge_candidate_idx =
                        (self.merge_candidate_idx + 1) % self.merge_candidates.len();
                    let (start, end) = self.merge_candidates[self.merge_candidate_idx];
                    self.merge_match_idx = start;
                    self.merge_match_end = end;
                    self.merge_cursor = start;
                    self.merge_file_scroll = start.saturating_sub(2);
                    self.push_info(
                        format!(
                            "  ➡️ Candidate {}/{}",
                            self.merge_candidate_idx + 1,
                            self.merge_candidates.len()
                        ),
                        LineStyle::Info,
                    );
                }
            }
            KeyCode::Tab => {
                self.merge_left_active = !self.merge_left_active;
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                if self.merge_index > 0 {
                    self.merge_index -= 1;
                    self.merge_scroll = 0;
                    self.merge_right_cursor = 0;
                    self.merge_search_query = None;
                    self.merge_last_modified = None;
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
                    self.merge_last_modified = None;
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
            KeyCode::Char('%') => {
                if self.merge_left_active {
                    let hunk = self.pending_merge.as_ref().unwrap()[self.merge_index].clone();
                    let project_root = std::path::PathBuf::from(&self.config.tools.project_root);
                    let file_path = project_root.join(&hunk.filename);
                    let file_content = if let Some(idx) =
                        self.buffers.iter().position(|b| b.name() == hunk.filename)
                    {
                        self.buffers[idx]
                            .lines()
                            .iter()
                            .map(|l| l.content().clone())
                            .collect::<Vec<String>>()
                            .join("\n")
                    } else {
                        std::fs::read_to_string(&file_path).unwrap_or_default()
                    };
                    let file_lines: Vec<String> = file_content.lines().map(String::from).collect();
                    if let Some((l, c)) =
                        find_matching_bracket(&file_lines, self.merge_cursor, self.merge_cursor_col)
                    {
                        self.merge_cursor = l;
                        self.merge_cursor_col = c;
                        if let Some(line) = file_lines.get(l) {
                            let len = line.graphemes(true).count();
                            self.merge_cursor_col =
                                self.merge_cursor_col.min(len.saturating_sub(1));
                        }
                        if l < self.merge_file_scroll {
                            self.merge_file_scroll = l.saturating_sub(2);
                        }
                    }
                }
            }
            KeyCode::Left => {
                if self.merge_left_active {
                    if self.merge_cursor_col > 0 {
                        self.merge_cursor_col -= 1;
                    }
                }
            }
            KeyCode::Right => {
                if self.merge_left_active {
                    self.merge_cursor_col += 1;
                }
            }
            KeyCode::Home => {
                if self.merge_left_active {
                    self.merge_cursor_col = 0;
                }
            }
            KeyCode::End => {
                if self.merge_left_active {
                    let hunk = self.pending_merge.as_ref().unwrap()[self.merge_index].clone();
                    let project_root = std::path::PathBuf::from(&self.config.tools.project_root);
                    let file_path = project_root.join(&hunk.filename);
                    let file_content = if let Some(idx) =
                        self.buffers.iter().position(|b| b.name() == hunk.filename)
                    {
                        self.buffers[idx]
                            .lines()
                            .iter()
                            .map(|l| l.content().clone())
                            .collect::<Vec<String>>()
                            .join("\n")
                    } else {
                        std::fs::read_to_string(&file_path).unwrap_or_default()
                    };
                    let file_lines: Vec<String> = file_content.lines().map(String::from).collect();
                    if let Some(line) = file_lines.get(self.merge_cursor) {
                        let len = line.graphemes(true).count();
                        self.merge_cursor_col = len.saturating_sub(1);
                    }
                }
            }
            KeyCode::Char('m') => {
                self.pending = Some('m');
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
            KeyCode::Char('J') | KeyCode::Char('=') => {
                if self.merge_left_active {
                    let dist_to_start = self.merge_cursor.saturating_sub(self.merge_match_idx);
                    let dist_to_end =
                        (self.merge_match_end.saturating_sub(1)).saturating_sub(self.merge_cursor);
                    if dist_to_start <= dist_to_end {
                        self.merge_match_idx += 1;
                        if self.merge_match_end <= self.merge_match_idx {
                            self.merge_match_end = self.merge_match_idx + 1;
                        }
                        self.merge_cursor = self.merge_match_idx;
                    } else {
                        self.merge_match_end += 1;
                        self.merge_cursor = self.merge_match_end.saturating_sub(1);
                    }
                } else {
                    let vis = self.response_area_height().saturating_sub(2);
                    self.merge_scroll += vis;
                }
            }
            KeyCode::Char('K') | KeyCode::Char('-') => {
                if self.merge_left_active {
                    let dist_to_start = self.merge_cursor.saturating_sub(self.merge_match_idx);
                    let dist_to_end =
                        (self.merge_match_end.saturating_sub(1)).saturating_sub(self.merge_cursor);
                    if dist_to_start <= dist_to_end {
                        if self.merge_match_idx > 0 {
                            self.merge_match_idx -= 1;
                            self.merge_cursor = self.merge_match_idx;
                        }
                    } else {
                        if self.merge_match_end > self.merge_match_idx + 1 {
                            self.merge_match_end -= 1;
                            self.merge_cursor = self.merge_match_end.saturating_sub(1);
                        }
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
                        if !*is_marker && !text.starts_with("... ") {
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

                let buf_idx = self.buffers.iter().position(|b| b.name() == hunk.filename);
                let is_buffer = self.merge_buffer_apply || buf_idx.is_some();

                let old_content = if let Some(idx) = buf_idx {
                    self.buffers[idx]
                        .lines()
                        .iter()
                        .map(|l| l.content().clone())
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    std::fs::read_to_string(&file_path).unwrap_or_default()
                };

                push_undo(&hunk.filename, &old_content);
                self.merge_last_modified = Some((hunk.filename.clone(), is_buffer));

                let mut file_lines: Vec<String> = old_content.lines().map(String::from).collect();
                let line_idx = (self.merge_cursor + 1).min(file_lines.len());
                file_lines.insert(line_idx, String::new());
                let mut new_content = file_lines.join("\n");
                if old_content.ends_with('\n') {
                    new_content.push('\n');
                }

                if let Some(idx) = buf_idx {
                    self.buffers[idx].clear();
                    self.buffers[idx].push_str(&new_content, LineStyle::Plain);
                } else if self.merge_buffer_apply {
                    let idx = self.buffers.len();
                    self.buffers.push(ResponseBuffer::with_name(&hunk.filename));
                    self.buffers[idx].push_str(&new_content, LineStyle::Plain);
                }

                if !self.merge_buffer_apply {
                    std::fs::write(&file_path, &new_content)?;
                }
                self.modified_buffers.insert(hunk.filename.clone());

                if self.merge_cursor < self.merge_match_idx {
                    self.merge_match_idx += 1;
                    self.merge_match_end += 1;
                } else if self.merge_cursor < self.merge_match_end {
                    self.merge_match_end += 1;
                }
                self.merge_cursor = line_idx;
            }
            KeyCode::Char('u') => {
                let hunk_filename = self.pending_merge.as_ref().unwrap()[self.merge_index]
                    .filename
                    .clone();
                let (target_file, was_buffer) = self
                    .merge_last_modified
                    .clone()
                    .unwrap_or((hunk_filename.clone(), self.merge_buffer_apply));
                if let Some(undo_content) = pop_undo(&target_file) {
                    clear_last_applied_range(&target_file);
                    let project_root = std::path::PathBuf::from(&self.config.tools.project_root);
                    let file_path = project_root.join(&target_file);

                    if was_buffer {
                        let buf_idx = if let Some(idx) =
                            self.buffers.iter().position(|b| b.name() == target_file)
                        {
                            idx
                        } else {
                            let idx = self.buffers.len();
                            self.buffers.push(ResponseBuffer::with_name(&target_file));
                            idx
                        };
                        self.buffers[buf_idx].clear();
                        self.buffers[buf_idx].push_str(&undo_content, LineStyle::Plain);
                        self.push_info(
                            format!("  ↩️ Undo successful for {} (buffer).", target_file),
                            LineStyle::Info,
                        );
                        let disk_content = std::fs::read_to_string(&file_path).unwrap_or_default();
                        let buf_content = self.buffers[buf_idx]
                            .lines()
                            .iter()
                            .map(|l| l.content().clone())
                            .collect::<Vec<String>>()
                            .join("\n");
                        if buf_content == disk_content {
                            self.modified_buffers.remove(&target_file);
                        }
                    } else {
                        std::fs::write(&file_path, &undo_content)?;
                        self.push_info(
                            format!("  ↩️ Undo successful for {} (disk).", target_file),
                            LineStyle::Info,
                        );
                        if let Some(idx) = self.buffers.iter().position(|b| b.name() == target_file)
                        {
                            let buf_content = self.buffers[idx]
                                .lines()
                                .iter()
                                .map(|l| l.content().clone())
                                .collect::<Vec<String>>()
                                .join("\n");
                            if buf_content == undo_content {
                                self.modified_buffers.remove(&target_file);
                            }
                        }
                    }
                    if undo_stack_len(&target_file) == 0 {
                        self.merge_last_modified = None;
                        if let Some(idx) = self.merge_last_applied_idx.take() {
                            if idx < self.merge_applied.len() {
                                self.merge_applied[idx] = false;
                            }
                        }
                    }

                    if hunk_filename == target_file {
                        let old_cursor = self.merge_cursor;
                        let old_match_idx = self.merge_match_idx;
                        self.calc_merge_file_scroll();
                        let new_match_idx = self.merge_match_idx;

                        if old_match_idx != usize::MAX / 2 && new_match_idx != usize::MAX / 2 {
                            let diff = new_match_idx as i64 - old_match_idx as i64;
                            self.merge_cursor = (old_cursor as i64 + diff).max(0) as usize;
                        } else if new_match_idx != usize::MAX / 2 {
                            if old_cursor == 0 {
                                self.merge_cursor = new_match_idx;
                            } else {
                                self.merge_cursor = old_cursor;
                            }
                        } else {
                            self.merge_cursor = old_cursor;
                        }

                        let content = if was_buffer {
                            self.buffers
                                .iter()
                                .find(|b| b.name() == target_file)
                                .map(|b| {
                                    b.lines()
                                        .iter()
                                        .map(|l| l.content().clone())
                                        .collect::<Vec<_>>()
                                        .join("\n")
                                })
                                .unwrap_or_default()
                        } else {
                            std::fs::read_to_string(&file_path).unwrap_or_default()
                        };
                        let file_lines: Vec<String> = content.lines().map(String::from).collect();
                        if self.merge_cursor >= file_lines.len() && !file_lines.is_empty() {
                            self.merge_cursor = file_lines.len() - 1;
                        } else if file_lines.is_empty() {
                            self.merge_cursor = 0;
                        }
                    }
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

    /// Shows a compact popup summarizing ALL patch hunks: match percentage
    /// and candidate count for each, formatted 3-items-per-line.
    /// Triggered by pressing `S` in merge mode.
    fn show_merge_summary_popup(&mut self) {
        let hunks = match &self.pending_merge {
            Some(h) => h.clone(),
            None => return,
        };
        let project_root = std::path::PathBuf::from(&self.config.tools.project_root);

        // Pre-compute (score, candidate_count, applied) for every hunk
        let mut summaries: Vec<(u32, usize, bool)> = Vec::with_capacity(hunks.len());
        for (i, hunk) in hunks.iter().enumerate() {
            let file_path = project_root.join(&hunk.filename);
            let file_content =
                if let Some(idx) = self.buffers.iter().position(|b| b.name() == hunk.filename) {
                    self.buffers[idx]
                        .lines()
                        .iter()
                        .map(|l| l.content().clone())
                        .collect::<Vec<String>>()
                        .join("\n")
                } else {
                    std::fs::read_to_string(&file_path).unwrap_or_default()
                };
            let file_lines: Vec<String> = file_content.lines().map(String::from).collect();

            let search = &hunk.search;
            let (score, cand_count) = if search.is_empty() || file_lines.is_empty() {
                (0u32, 0usize)
            } else {
                let mut result = crate::diff::find_best_match(search, &file_lines, false);
                if result.candidates.is_empty() {
                    result = loose_best_match(search, &file_lines);
                }
                // Only count candidates with a score strictly greater than 50%
                let count = result
                    .candidates
                    .iter()
                    .filter(|(_, _, sc)| *sc > 50.0)
                    .count();
                (result.score.clamp(0.0, 100.0) as u32, count)
            };

            let applied = self.merge_applied.get(i).copied().unwrap_or(false);
            summaries.push((score, cand_count, applied));
        }

        // Pack as many items as possible per line based on terminal width
        let max_w = self.term_width().saturating_sub(6);
        let mut items: Vec<crate::repl::helper::PopupItem> = Vec::new();
        let mut current_line = String::new();

        for (i, (score, cand_count, _applied)) in summaries.iter().enumerate() {
            // Add green color for 100% using ANSI escape codes
            let score_str = if *score == 100 {
                "\x1b[32m100%\x1b[0m".to_string()
            } else {
                format!("{}%", score)
            };

            let item_str = if *score == 100 {
                format!("\x1b[32m{}(100%|{})\x1b[0m", i + 1, cand_count)
            } else {
                format!("{}({}%|{})", i + 1, score, cand_count)
            };

            if current_line.is_empty() {
                current_line = item_str;
            } else {
                let test_line = format!("{}  {}", current_line, item_str);
                // Check visual width without ANSI escape codes
                let clean_test = test_line.replace("\x1b[32m", "").replace("\x1b[0m", "");
                let width = UnicodeWidthStr::width(clean_test.as_str());

                if width > max_w {
                    items.push(crate::repl::helper::PopupItem {
                        text: std::mem::take(&mut current_line),
                        is_active: true,
                        id: None,
                    });
                    current_line = item_str;
                } else {
                    current_line = test_line;
                }
            }
        }

        if !current_line.is_empty() {
            items.push(crate::repl::helper::PopupItem {
                text: current_line,
                is_active: true,
                id: None,
            });
        }

        if items.is_empty() {
            items.push(crate::repl::helper::PopupItem {
                text: "No hunks found.".to_string(),
                is_active: true,
                id: None,
            });
        }

        let all_100 = !summaries.is_empty() && summaries.iter().all(|(s, _, _)| *s == 100);
        let title = if all_100 {
            "(All 100%) Hunk Summary"
        } else {
            "Hunk Summary"
        };

        self.popup_mode = crate::repl::PopupMode::Message;
        self.popup
            .show(title, items, 0, crate::repl::helper::PopupPosition::Center);
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
                self.push_info(
                    "  ✅ All hunks processed. Press 'u' to undo the last change, or 'q' to quit.",
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
        
        let is_applied = self
            .merge_applied
            .get(self.merge_index)
            .copied()
            .unwrap_or(false);
        let right_rows: Vec<(String, Color, bool)> = Self::build_right_rows(hunk, is_applied);
        let project_root = std::path::PathBuf::from(&self.config.tools.project_root);
        let file_path = project_root.join(&hunk.filename);
        let file_content =
            if let Some(idx) = self.buffers.iter().position(|b| b.name() == hunk.filename) {
                self.buffers[idx]
                    .lines()
                    .iter()
                    .map(|l| l.content().clone())
                    .collect::<Vec<String>>()
                    .join("\n")
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
        let (match_label, match_color) = if equal_count == 0 {
            ("match:none".to_string(), Color::Red)
        } else if match_percent == 100 {
            (format!("match:{}%", match_percent), Color::Green)
        } else {
            (format!("match:{}%", match_percent), Color::Yellow)
        };

        let search_loc = hunk.search.len();
        let match_loc = matched_lines.len();
        
        let fname_disp = trunc(&hunk.filename, right_width.saturating_sub(1));
        let fname_pad = right_width.saturating_sub(UnicodeWidthStr::width(fname_disp.as_str()) + 1);
        let left_hdr_bg = Color::DarkGrey;
        let left_hdr_fg = if self.merge_left_active {
            Color::Cyan
        } else {
            Color::White
        };
        let right_hdr_bg = Color::DarkGrey;
        let right_hdr_fg = if !self.merge_left_active {
            Color::Cyan
        } else {
            Color::White
        };
        
        let loc_color = Color::Cyan;
        let hunk_idx_str = format!("[{}/{}]", self.merge_index + 1, hunks.len());
        let left_hdr_parts = vec![
            (" ".to_string(), left_hdr_fg),
            (hunk_idx_str, Color::Yellow),
            (" match: ".to_string(), left_hdr_fg),
            (format!("{}%", match_percent), match_color),
            (" (".to_string(), left_hdr_fg),
            (format!("{}", search_loc), loc_color),
            (" vs ".to_string(), left_hdr_fg),
            (format!("{}", match_loc), loc_color),
            (" LOC)".to_string(), left_hdr_fg),
        ];
        
        let mut left_hdr_w = 0;
        for (s, _) in &left_hdr_parts {
            left_hdr_w += UnicodeWidthStr::width(s.as_str());
        }
        let left_hdr_pad = left_width.saturating_sub(left_hdr_w + 1);
        
        queue!(
            stdout,
            cursor::MoveTo(0, 0),
            SetBackgroundColor(left_hdr_bg),
            SetAttribute(Attribute::Bold),
        )?;
        for (s, c) in &left_hdr_parts {
            queue!(stdout, SetForegroundColor(*c), Print(s))?;
        }
        queue!(
            stdout,
            SetForegroundColor(left_hdr_fg),
            Print(" ".repeat(left_hdr_pad)),
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
        let hunk_summary = if hunks.len() > 1 {
            let mut s = String::new();
            for (i, applied) in self.merge_applied.iter().enumerate() {
                if i > 0 {
                    s.push(' ');
                }
                s.push_str(&format!("{}{}", i + 1, if *applied { "✅" } else { "⏳" }));
            }
            format!("[{}] ", s)
        } else {
            String::new()
        };
        let hint_y = self.height.saturating_sub(1) as u16;
        queue!(
            stdout,
            cursor::MoveTo(0, hint_y),
            SetBackgroundColor(Color::DarkGrey),
            SetForegroundColor(Color::Yellow),
            Print(format!(
                " 🔀 {}[?] Help [a]pply [w]rite [l]next [s]ummary ",
                hunk_summary
            )),
            style::ResetColor
        )?;
        let start_y = 1;
        let visible_height = hint_y as usize - start_y;
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
                let is_ghost_anchor = !is_in_match && f_idx == actual_match_idx;
                let is_applied_line = get_last_applied_range(&hunk.filename)
                    .map(|(s, e)| f_idx >= s && f_idx < e)
                    .unwrap_or(false);
                let line_num = f_idx + 1;
                let cursor_mark = if is_cursor {
                    ">"
                } else if is_ghost_anchor {
                    "?"
                } else {
                    " "
                };
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
                let (fg, bg) = if is_cursor {
                    (Color::Black, Color::Cyan)
                } else if is_applied_line {
                    (Color::Black, Color::DarkYellow)
                } else if is_in_match {
                    (Color::White, Color::Blue)
                } else if is_ghost_anchor {
                    (Color::DarkGrey, Color::Black)
                } else {
                    (Color::White, Color::Black)
                };
                if is_cursor && self.merge_left_active {
                    let graphemes: Vec<&str> = line.graphemes(true).collect();
                    let col = self.merge_cursor_col.min(graphemes.len().saturating_sub(1));
                    let mut left_part = String::new();
                    let mut mid_part = String::new();
                    let mut right_part = String::new();
                    for (i, g) in graphemes.iter().enumerate() {
                        if i < col {
                            left_part.push_str(g);
                        } else if i == col {
                            mid_part.push_str(g);
                        } else {
                            right_part.push_str(g);
                        }
                    }
                    let left_w = UnicodeWidthStr::width(left_part.as_str());
                    let mid_str = if mid_part.is_empty() {
                        " ".to_string()
                    } else {
                        mid_part.clone()
                    };
                    let mid_w = UnicodeWidthStr::width(mid_str.as_str());
                    let right_w = UnicodeWidthStr::width(right_part.as_str());
                    let total_w = left_w + mid_w + right_w + 1;
                    let pad = left_panel_content_w.saturating_sub(total_w);
                    queue!(
                        stdout,
                        cursor::MoveTo(0, y),
                        SetBackgroundColor(bg),
                        SetForegroundColor(Color::DarkGrey),
                        Print(&line_num_str),
                        SetForegroundColor(diff_color),
                        Print(diff_char),
                        SetBackgroundColor(bg),
                        SetForegroundColor(fg),
                        Print(format!(" {}", left_part)),
                        SetBackgroundColor(Color::White),
                        SetForegroundColor(Color::Black),
                        Print(&mid_str),
                        SetBackgroundColor(bg),
                        SetForegroundColor(fg),
                        Print(&right_part),
                        Print(" ".repeat(pad)),
                        style::ResetColor
                    )?;
                } else {
                    let disp = trunc(line, left_panel_content_w);
                    let pad =
                        left_panel_content_w.saturating_sub(UnicodeWidthStr::width(disp.as_str()));
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
                }
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
                } else if *is_marker && !is_applied {
                    Color::DarkGrey
                } else {
                    Color::Black
                };
                let fg = if is_right_cursor {
                    Color::Black
                } else if is_applied && !*is_marker {
                    Color::DarkGrey
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
        if self.fkey_help {
            let term_w = self.term_width() as u16;
            let margin = 2;
            let box_w = term_w
                .saturating_sub(margin * 2 + 1)
                .min(term_w.saturating_sub(margin * 2));
            let x = margin;
            let y_bot = self.height.saturating_sub(3);
            let y_l2 = y_bot.saturating_sub(1);
            let y_l1 = y_l2.saturating_sub(1);
            let y_top = y_l1.saturating_sub(1);
            let inner_w = (box_w as usize).saturating_sub(2);
            let pad = |s: &str| -> String {
                let current_w = UnicodeWidthStr::width(s);
                if current_w < inner_w {
                    format!("{}{}", s, " ".repeat(inner_w - current_w))
                } else {
                    s.to_string()
                }
            };
            let pad_item = |s: &str| -> String {
                let target_w = 14;
                let w = UnicodeWidthStr::width(s);
                if w < target_w {
                    format!("{}{}", s, " ".repeat(target_w - w))
                } else {
                    s.to_string()
                }
            };
            let line1_str = format!(
                "{}{}{}{}{}{}{}",
                pad_item(" a: Apply"),
                pad_item(" l: Next Hunk"),
                pad_item(" r: Recalc"),
                pad_item(" n: Next Cand"),
                pad_item(" u: Undo"),
                pad_item(" q: Quit"),
                pad_item(" ?: Toggle Help")
            );
            let line2_str = format!(
                "{}{}{}{}{}{}{}",
                pad_item(" Tab: Panel"),
                pad_item(" -: Shrink"),
                pad_item(" =: Expand"),
                pad_item(" ma: Mark S"),
                pad_item(" mA: Mark E"),
                pad_item(" Ent: Search"),
                pad_item(" o: Open Line")
            );
            let l1 = pad(&line1_str);
            let l2 = pad(&line2_str);
            queue!(
                stdout,
                SetForegroundColor(Color::Yellow),
                SetAttribute(Attribute::Bold),
                cursor::MoveTo(x, y_top),
                Print(format!("╭{}╮", "─".repeat(inner_w))),
                cursor::MoveTo(x, y_bot),
                Print(format!("╰{}╯", "─".repeat(inner_w))),
                cursor::MoveTo(x, y_l1),
                Print(format!("│{}│", l1)),
                cursor::MoveTo(x, y_l2),
                Print(format!("│{}│", l2)),
                style::ResetColor,
                SetAttribute(Attribute::Reset),
                cursor::Hide
            )?;
        }
        Ok(())
    }
}