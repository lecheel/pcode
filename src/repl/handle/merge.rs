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
use std::io;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

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
    pub fn start_merge(&mut self, hunks: Vec<PatchHunk>) {
        if hunks.is_empty() {
            return;
        }
        // Remove this line - merge_rows is not used
        // self.merge_rows = build_diff_rows(&hunks[0].search, &hunks[0].replace);
        self.pending_merge = Some(hunks);
        self.merge_index = 0;
        self.merge_scroll = 0;
        self.calc_merge_file_scroll();
        self.mode = Mode::Merge;
        let idx = self.llm_buffer_idx();
        self.active_buffer = idx;
        self.push_info(
            "  🔀 Entering Merge Mode. [a]pply [r]eject [n]ext/[p]rev hunk []/[ jump change [q]uit",
            LineStyle::Info,
        );
    }

    fn calc_merge_file_scroll(&mut self) {
        let hunk = &self.pending_merge.as_ref().unwrap()[self.merge_index];
        let project_root = std::path::PathBuf::from(&self.config.tools.project_root);
        let file_path = project_root.join(&hunk.filename);
        let file_content = std::fs::read_to_string(&file_path).unwrap_or_default();
        let file_lines: Vec<String> = file_content.lines().map(String::from).collect();
        
        // Use diff::find_best_match to accurately locate the anchor
        let match_result = crate::diff::find_best_match(&hunk.search, &file_lines, true);
        self.merge_match_idx = match_result.file_start;
        self.merge_file_scroll = match_result.file_start.saturating_sub(2);
        self.merge_anchor_offset = 0;
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

        match key.code {
            KeyCode::Char('a') | KeyCode::Char('A') => {
                let hunk = self.pending_merge.as_ref().unwrap()[self.merge_index].clone();
                let project_root = std::path::PathBuf::from(&self.config.tools.project_root);
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
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.push_info("  🚫 Rejected hunk.", LineStyle::Error);
                self.next_merge();
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Tab => {
                self.next_merge();
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                if self.merge_index > 0 {
                    self.merge_index -= 1;
                    self.merge_scroll = 0;
                    self.calc_merge_file_scroll();
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.merge_scroll += 1;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.merge_scroll = self.merge_scroll.saturating_sub(1);
            }
            KeyCode::Char('<') => {
                self.merge_anchor_offset -= 1;
            }
            KeyCode::Char('>') => {
                self.merge_anchor_offset += 1;
            }
            KeyCode::PageDown => {
                let vis = self.response_area_height().saturating_sub(2);
                self.merge_scroll += vis;
            }
            KeyCode::PageUp => {
                let vis = self.response_area_height().saturating_sub(2);
                self.merge_scroll = self.merge_scroll.saturating_sub(vis);
            }
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => {
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
            self.calc_merge_file_scroll();
        } else {
            self.pending_merge = None;
            self.mode = Mode::Insert;
            self.push_info(
                "  ✅ All hunks processed. Exited Merge Mode.",
                LineStyle::Info,
            );
        }
    }

    pub(crate) fn render_merge(&mut self, stdout: &mut io::Stdout) -> anyhow::Result<()> {
        let ra_height = self.response_area_height();
        let term_width = self.term_width();
        let split_x = (term_width * 3) / 10; // 3:7 ratio

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

        // ── header row: filename (left) | description (right) ──
        let fname_disp = trunc(&hunk.filename, left_width.saturating_sub(1));
        let fname_pad = left_width
            .saturating_sub(UnicodeWidthStr::width(fname_disp.as_str()) + 1);
        let desc = " detail full content sync the line to left";
        let desc_disp = trunc(desc, right_width.saturating_sub(1));
        let desc_pad = right_width
            .saturating_sub(UnicodeWidthStr::width(desc_disp.as_str()) + 1);

        queue!(
            stdout,
            cursor::MoveTo(0, 0),
            SetBackgroundColor(Color::DarkGrey),
            SetForegroundColor(Color::Cyan),
            SetAttribute(Attribute::Bold),
            Print(format!(" {}{}", fname_disp, " ".repeat(fname_pad))),
            cursor::MoveTo(split_x as u16, 0),
            SetForegroundColor(Color::DarkGrey),
            Print("│"),
            cursor::MoveTo((split_x + 1) as u16, 0),
            SetForegroundColor(Color::White),
            Print(format!(" {}{}", desc_disp, " ".repeat(desc_pad))),
            style::ResetColor,
            SetAttribute(Attribute::Reset)
        )?;

        // ── status row ─────────────────────────────────────────
        queue!(
            stdout,
            cursor::MoveTo(0, 1),
            SetBackgroundColor(Color::DarkGrey),
            SetForegroundColor(Color::Yellow),
            Print(format!(
                " 🔀 [{}/{}]  [a]pply [r]eject [n]ext [p]rev [q]uit  [J/K]scroll [</>]anchor ",
                self.merge_index + 1,
                hunks.len()
            )),
            style::ResetColor
        )?;

        // ── build left panel rows (the patch) ──────────────────
        let fold = 5usize;
        let mut left_rows: Vec<(String, Color, bool)> = Vec::new();
        left_rows.push(("<<<<<<< SEARCH".to_string(), Color::Magenta, true));

        let search = &hunk.search;
        if search.len() <= fold * 2 {
            for l in search {
                left_rows.push((l.clone(), Color::Red, false));
            }
        } else {
            for l in &search[..fold] {
                left_rows.push((l.clone(), Color::Red, false));
            }
            left_rows.push((
                format!("... placeholder ({}) ...", search.len() - fold * 2),
                Color::DarkGrey,
                false,
            ));
            for l in &search[search.len() - fold..] {
                left_rows.push((l.clone(), Color::Red, false));
            }
        }

        left_rows.push(("=======".to_string(), Color::Magenta, true));

        let replace = &hunk.replace;
        if replace.len() <= fold * 2 {
            for l in replace {
                left_rows.push((l.clone(), Color::Green, false));
            }
        } else {
            for l in &replace[..fold] {
                left_rows.push((l.clone(), Color::Green, false));
            }
            left_rows.push((
                format!("... placeholder ({}) ...", replace.len() - fold * 2),
                Color::DarkGrey,
                false,
            ));
            for l in &replace[replace.len() - fold..] {
                left_rows.push((l.clone(), Color::Green, false));
            }
        }

        left_rows.push((">>>>>>> REPLACE".to_string(), Color::Magenta, true));

        // ── read file and find match for right panel ───────────
        let project_root = std::path::PathBuf::from(&self.config.tools.project_root);
        let file_path = project_root.join(&hunk.filename);
        let file_content = std::fs::read_to_string(&file_path).unwrap_or_default();
        let file_lines: Vec<String> = file_content.lines().map(String::from).collect();

        let actual_match_idx = (self.merge_match_idx as i32 + self.merge_anchor_offset).max(0) as usize;
        let matched_end = actual_match_idx + hunk.search.len();

        // ── render left panel with scroll ──────────────────────
        let start_y = 2;
        let visible_height = ra_height.saturating_sub(start_y);
        let max_scroll = left_rows.len().saturating_sub(visible_height);
        if self.merge_scroll > max_scroll {
            self.merge_scroll = max_scroll;
        }

        let max_file_scroll = file_lines.len().saturating_sub(visible_height);
        if self.merge_file_scroll > max_file_scroll {
            self.merge_file_scroll = max_file_scroll;
        }

        let target_left_w = left_width.saturating_sub(2);
        let right_panel_content_w = right_width.saturating_sub(6); // 4 for line num, 2 for padding

        for i in 0..visible_height {
            let idx = self.merge_scroll + i;
            let y = (start_y + i) as u16;

            // Left panel
            if idx < left_rows.len() {
                let (text, color, is_marker) = &left_rows[idx];
                let disp = trunc(text, target_left_w);
                let pad = target_left_w.saturating_sub(UnicodeWidthStr::width(disp.as_str()));
                let bg = if *is_marker { Color::DarkGrey } else { Color::Black };

                queue!(
                    stdout,
                    cursor::MoveTo(0, y),
                    SetBackgroundColor(bg),
                    SetForegroundColor(*color),
                    if *is_marker { SetAttribute(Attribute::Bold) } else { SetAttribute(Attribute::Reset) },
                    Print(format!(" {}{}", disp, " ".repeat(pad))),
                    style::ResetColor,
                    SetAttribute(Attribute::Reset)
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

            // Right panel
            let f_idx = self.merge_file_scroll + i;
            if f_idx < file_lines.len() {
                let line = &file_lines[f_idx];
                let is_in_match = f_idx >= actual_match_idx && f_idx < matched_end;
                
                let line_num = f_idx + 1;
                let line_num_str = format!("{:>4} ", line_num);

                let disp = trunc(line, right_panel_content_w);
                let pad = right_panel_content_w.saturating_sub(UnicodeWidthStr::width(disp.as_str()));
                
                let (fg, bg) = if is_in_match {
                    (Color::Black, Color::Yellow)
                } else {
                    (Color::White, Color::Black)
                };

                queue!(
                    stdout,
                    cursor::MoveTo((split_x + 1) as u16, y),
                    SetBackgroundColor(bg),
                    SetForegroundColor(Color::DarkGrey),
                    Print(&line_num_str),
                    SetForegroundColor(fg),
                    Print(format!(" {}{}", disp, " ".repeat(pad))),
                    style::ResetColor
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