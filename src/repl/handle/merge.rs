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
use std::io;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

impl Repl {
    pub fn start_merge(&mut self, hunks: Vec<PatchHunk>) {
        if hunks.is_empty() {
            return;
        }
        self.pending_merge = Some(hunks);
        self.merge_index = 0;
        self.merge_scroll = 0;
        self.mode = Mode::Merge;
        let idx = self.llm_buffer_idx();
        self.active_buffer = idx;
        self.push_info("  🔀 Entering Merge Mode. [a]pply [r]eject [n]ext [p]rev [q]uit", LineStyle::Info);
    }

    pub(super) fn handle_merge_key(&mut self, key: KeyEvent, stdout: &mut io::Stdout) -> anyhow::Result<()> {
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
                
                match crate::patch::apply_patch(&hunk.filename, &patch_text, &project_root, &self.config.tools.allow_paths) {
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
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.merge_scroll += 1;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.merge_scroll = self.merge_scroll.saturating_sub(1);
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
        } else {
            self.pending_merge = None;
            self.mode = Mode::Insert;
            self.push_info("  ✅ All hunks processed. Exited Merge Mode.", LineStyle::Info);
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
            queue!(stdout, cursor::MoveTo(0, i as u16), terminal::Clear(ClearType::CurrentLine))?;
        }

        queue!(
            stdout,
            cursor::MoveTo(0, 0),
            SetForegroundColor(Color::Cyan),
            SetAttribute(Attribute::Bold),
            Print(format!(" 🔀 Merge: {} [{}/{}] ", hunk.filename, self.merge_index + 1, hunks.len())),
            style::ResetColor,
            SetAttribute(Attribute::Reset)
        )?;
        queue!(
            stdout,
            cursor::MoveTo(split_x as u16, 0),
            SetForegroundColor(Color::DarkGrey),
            Print(format!(" [a]pply [r]eject [n]ext [p]rev [q]uit ")),
            style::ResetColor
        )?;

        let left_width = split_x.saturating_sub(1);
        let right_width = term_width.saturating_sub(split_x).saturating_sub(1);

        queue!(
            stdout,
            cursor::MoveTo(0, 1),
            SetBackgroundColor(Color::DarkGrey),
            SetForegroundColor(Color::White),
            SetAttribute(Attribute::Bold),
            Print(format!("{:width$}", " SEARCH", width = left_width)),
            cursor::MoveTo(split_x as u16, 1),
            Print("│"),
            cursor::MoveTo(split_x as u16 + 1, 1),
            Print(format!("{:width$}", "  REPLACE", width = right_width)),
            style::ResetColor,
            SetAttribute(Attribute::Reset)
        )?;

        let max_lines = hunk.search.len().max(hunk.replace.len());
        let start_y = 2;
        let visible_height = ra_height.saturating_sub(start_y);
        
        let max_scroll = max_lines.saturating_sub(visible_height);
        if self.merge_scroll > max_scroll {
            self.merge_scroll = max_scroll;
        }

        let target_left_w = left_width.saturating_sub(1);
        let target_right_w = right_width.saturating_sub(2);

        for i in 0..visible_height {
            let hunk_line_idx = self.merge_scroll + i;
            if hunk_line_idx >= max_lines {
                break;
            }

            let y = (start_y + i) as u16;

            let left_line = hunk.search.get(hunk_line_idx).cloned().unwrap_or_default();
            let left_color = if hunk_line_idx < hunk.search.len() { Color::White } else { Color::DarkGrey };
            
            let mut left_display = left_line.clone();
            if UnicodeWidthStr::width(left_display.as_str()) > target_left_w {
                let mut current_width = 0;
                let mut truncated = String::new();
                for g in left_display.graphemes(true) {
                    let gw = UnicodeWidthStr::width(g);
                    if current_width + gw + 3 > target_left_w {
                        break;
                    }
                    truncated.push_str(g);
                    current_width += gw;
                }
                truncated.push_str("...");
                left_display = truncated;
            }
            let left_pad = target_left_w.saturating_sub(UnicodeWidthStr::width(left_display.as_str()));
            
            queue!(
                stdout,
                cursor::MoveTo(0, y),
                SetBackgroundColor(Color::Black),
                SetForegroundColor(left_color),
                Print(format!(" {}{}", left_display, " ".repeat(left_pad))),
                style::ResetColor
            )?;

            queue!(
                stdout,
                cursor::MoveTo(split_x as u16, y),
                SetForegroundColor(Color::DarkGrey),
                Print("│"),
                style::ResetColor
            )?;

            let right_line = hunk.replace.get(hunk_line_idx).cloned().unwrap_or_default();
            let right_color;
            let gutter_sym;
            
            if hunk_line_idx >= hunk.replace.len() {
                right_color = Color::DarkGrey;
                gutter_sym = " ";
            } else if hunk_line_idx >= hunk.search.len() {
                right_color = Color::Green;
                gutter_sym = "+";
            } else if hunk.search[hunk_line_idx] == hunk.replace[hunk_line_idx] {
                right_color = Color::White;
                gutter_sym = "=";
            } else {
                right_color = Color::Yellow;
                gutter_sym = "~";
            }
            
            let mut right_display = right_line.clone();
            if UnicodeWidthStr::width(right_display.as_str()) > target_right_w {
                let mut current_width = 0;
                let mut truncated = String::new();
                for g in right_display.graphemes(true) {
                    let gw = UnicodeWidthStr::width(g);
                    if current_width + gw + 3 > target_right_w {
                        break;
                    }
                    truncated.push_str(g);
                    current_width += gw;
                }
                truncated.push_str("...");
                right_display = truncated;
            }
            let right_pad = target_right_w.saturating_sub(UnicodeWidthStr::width(right_display.as_str()));
            
            queue!(
                stdout,
                cursor::MoveTo(split_x as u16 + 1, y),
                SetBackgroundColor(Color::Black),
                SetForegroundColor(right_color),
                Print(format!("{} {}{}", gutter_sym, right_display, " ".repeat(right_pad))),
                style::ResetColor
            )?;
        }

        Ok(())
    }
}