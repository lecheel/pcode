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

impl Repl {
    pub fn start_merge(&mut self, hunks: Vec<PatchHunk>) {
        if hunks.is_empty() {
            return;
        }
        self.pending_merge = Some(hunks);
        self.merge_index = 0;
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
                }
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
        } else {
            self.pending_merge = None;
            self.mode = Mode::Insert;
            self.push_info("  ✅ All hunks processed. Exited Merge Mode.", LineStyle::Info);
        }
    }

    pub(crate) fn render_merge(&mut self, stdout: &mut io::Stdout) -> anyhow::Result<()> {
        let ra_height = self.response_area_height();
        let term_width = self.term_width();
        let half_width = term_width / 2;

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
            cursor::MoveTo(half_width as u16, 0),
            SetForegroundColor(Color::DarkGrey),
            Print(format!(" [a]pply [r]eject [n]ext [p]rev [q]uit ")),
            style::ResetColor
        )?;

        queue!(
            stdout,
            cursor::MoveTo(0, 1),
            SetBackgroundColor(Color::DarkGrey),
            SetForegroundColor(Color::White),
            SetAttribute(Attribute::Bold),
            Print(" SEARCH"),
            cursor::MoveTo(half_width as u16, 1),
            Print("│"),
            cursor::MoveTo(half_width as u16 + 1, 1),
            Print(" REPLACE"),
            style::ResetColor,
            SetAttribute(Attribute::Reset)
        )?;

        let max_lines = hunk.search.len().max(hunk.replace.len());
        let start_y = 2;

        for i in 0..max_lines {
            let y = start_y + i as u16;
            if y as usize >= ra_height {
                break;
            }

            let left_line = hunk.search.get(i).cloned().unwrap_or_default();
            let left_color = if i < hunk.search.len() { Color::White } else { Color::DarkGrey };
            
            queue!(
                stdout,
                cursor::MoveTo(0, y),
                SetBackgroundColor(Color::Black),
                SetForegroundColor(left_color),
                Print(format!(" {}", left_line)),
                style::ResetColor
            )?;

            queue!(
                stdout,
                cursor::MoveTo(half_width as u16, y),
                SetForegroundColor(Color::DarkGrey),
                Print("│"),
                style::ResetColor
            )?;

            let right_line = hunk.replace.get(i).cloned().unwrap_or_default();
            let right_color = if i < hunk.replace.len() { Color::Green } else { Color::DarkGrey };
            
            queue!(
                stdout,
                cursor::MoveTo(half_width as u16 + 1, y),
                SetBackgroundColor(Color::Black),
                SetForegroundColor(right_color),
                Print(format!(" {}", right_line)),
                style::ResetColor
            )?;
        }

        Ok(())
    }
}