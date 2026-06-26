// src/repl/handle/visual.rs
//! Visual / Visual-Line mode + selection yank/delete + chat grab.

use super::super::*;
use crate::repl::buffer::LineStyle;
use crate::repl::misc::splice_line;
use crate::repl::Mode;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::io;

impl Repl {
    pub(super) fn handle_visual_key(
        &mut self,
        key: KeyEvent,
        stdout: &mut io::Stdout,
    ) -> anyhow::Result<()> {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if key.code == KeyCode::Char('c') {
                self.mode = Mode::Normal;
                self.selection_start = None;
                self.render(stdout)?;
                return Ok(());
            }
            match key.code {
                KeyCode::Char('d') => self.half_page_down(),
                KeyCode::Char('u') => self.half_page_up(),
                KeyCode::Char('b') => self.half_page_up(),
                KeyCode::Char('f') => self.half_page_down(),
                _ => {}
            }
            self.count = None;
            self.pending = None;
            self.render(stdout)?;
            return Ok(());
        }

        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.selection_start = None;
            }
            KeyCode::Char('>') => {
                self.grab_selection_to_chat();
                self.render(stdout)?;
                return Ok(());
            }
            KeyCode::Char(':') => {
                self.mode = Mode::Command;
                self.cmd_editor.clear();
                self.count = None;
                self.pending = None;
                self.render_spinner_only(stdout)?;
                return Ok(());
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.buffer_mut().move_down(1);
                self.ensure_cursor_visible();
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.buffer_mut().move_up(1);
                self.ensure_cursor_visible();
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.buffer_mut().move_left();
                self.ensure_cursor_visible();
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.buffer_mut().move_right();
                self.ensure_cursor_visible();
            }
            KeyCode::Char('w') => {
                self.buffer_mut().move_right();
                self.ensure_cursor_visible();
            }
            KeyCode::Char('b') => {
                self.buffer_mut().move_left();
                self.ensure_cursor_visible();
            }
            KeyCode::Char('0') => {
                let line_idx = self.buffer().cursor_line();
                self.buffer_mut().set_cursor(line_idx, 0);
                self.ensure_cursor_visible();
            }
            KeyCode::Char('$') => {
                let line_idx = self.buffer().cursor_line();
                if let Some(line) = self.buffer().lines().get(line_idx) {
                    let len = line.content().graphemes(true).count();
                    let col = if len > 0 { len - 1 } else { 0 };
                    self.buffer_mut().set_cursor(line_idx, col);
                    self.ensure_cursor_visible();
                }
            }
            KeyCode::Char('G') => {
                self.move_bottom();
            }
            KeyCode::Char('g') => {
                if self.pending == Some('g') {
                    self.buffer_mut().move_top();
                    self.pending = None;
                } else {
                    self.pending = Some('g');
                    self.render(stdout)?;
                    return Ok(());
                }
            }
            KeyCode::Char('y') => {
                self.yank_selection();
                self.mode = Mode::Normal;
                self.selection_start = None;
            }
            KeyCode::Char('d') | KeyCode::Char('x') => {
                self.delete_selection();
                self.mode = Mode::Normal;
                self.selection_start = None;
            }
            _ => {}
        }
        self.render(stdout)
    }

    fn yank_selection(&mut self) {
        if let Some(start) = self.selection_start {
            let end = (self.buffer().cursor_line(), self.buffer().cursor_col());
            let is_line_wise = matches!(self.mode, Mode::VisualLine);
            let text = self.extract_text(start, end, is_line_wise);
            if !text.is_empty() {
                match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text)) {
                    Ok(_) => self.push_info(
                        "  📋 Yanked selection to clipboard".to_string(),
                        LineStyle::Dim,
                    ),
                    Err(e) => {
                        self.push_info(format!("  ❌ Clipboard error: {}", e), LineStyle::Error)
                    }
                }
                self.scroll_to_bottom_view();
            }
        }
    }

    fn delete_selection(&mut self) {
        if let Some(start) = self.selection_start {
            let end = (self.buffer().cursor_line(), self.buffer().cursor_col());
            let is_line_wise = matches!(self.mode, Mode::VisualLine);
            let text = self.extract_text(start, end, is_line_wise);
            if !text.is_empty() {
                match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text)) {
                    Ok(_) => {}
                    Err(e) => {
                        self.push_info(format!("  ❌ Clipboard error: {}", e), LineStyle::Error)
                    }
                }
                self.delete_text(start, end);
                self.push_info("  🗑️  Deleted selection".to_string(), LineStyle::Dim);
                self.scroll_to_bottom_view();
            }
        }
    }

    fn extract_text(
        &self,
        start: (usize, usize),
        end: (usize, usize),
        _is_line_wise: bool,
    ) -> String {
        let (mut sl, mut sc) = start;
        let (mut el, mut ec) = end;
        if (sl, sc) > (el, ec) {
            std::mem::swap(&mut sl, &mut el);
            std::mem::swap(&mut sc, &mut ec);
        }

        let mut text = String::new();
        if matches!(self.mode, Mode::VisualLine) {
            for i in sl..=el {
                if let Some(line) = self.buffer().lines().get(i) {
                    text.push_str(&line.content());
                    text.push('\n');
                }
            }
        } else {
            for i in sl..=el {
                if let Some(line) = self.buffer().lines().get(i) {
                    let line_content = line.content();
                    let content: Vec<&str> = line_content.graphemes(true).collect();
                    if i == sl && i == el {
                        let end_idx = (ec + 1).min(content.len());
                        if sc < end_idx {
                            for c in &content[sc..end_idx] {
                                text.push_str(c);
                            }
                        }
                    } else if i == sl {
                        if sc < content.len() {
                            for c in &content[sc..] {
                                text.push_str(c);
                            }
                        }
                        text.push('\n');
                    } else if i == el {
                        let end_idx = (ec + 1).min(content.len());
                        for c in &content[..end_idx] {
                            text.push_str(c);
                        }
                    } else {
                        text.push_str(&line.content());
                        text.push('\n');
                    }
                }
            }
        }
        text
    }

    fn delete_text(&mut self, start: (usize, usize), end: (usize, usize)) {
        let (mut sl, mut sc) = start;
        let (mut el, mut ec) = end;
        if (sl, sc) > (el, ec) {
            std::mem::swap(&mut sl, &mut el);
            std::mem::swap(&mut sc, &mut ec);
        }

        if matches!(self.mode, Mode::VisualLine) {
            self.buffer_mut().remove_lines(sl, el + 1);
            let new_len = self.buffer().len();
            if sl < new_len {
                self.buffer_mut().set_cursor(sl, 0);
            } else if new_len > 0 {
                self.buffer_mut().set_cursor(new_len - 1, 0);
            }
        } else {
            let mut new_lines = Vec::new();
            let lines = self.buffer().lines();
            for i in 0..lines.len() {
                if i < sl || i > el {
                    new_lines.push(lines[i].clone());
                    continue;
                }
                let chars_count = lines[i].content().chars().count();

                if i == sl && i == el {
                    let end_idx = (ec + 1).min(chars_count);
                    let new_line = splice_line(&lines[i], sc, end_idx);
                    if !new_line.content().is_empty() {
                        new_lines.push(new_line);
                    }
                } else if i == sl {
                    let new_line = splice_line(&lines[i], sc, chars_count);
                    if !new_line.content().is_empty() {
                        new_lines.push(new_line);
                    }
                } else if i == el {
                    let end_idx = (ec + 1).min(chars_count);
                    let new_line = splice_line(&lines[i], 0, end_idx);
                    if !new_line.content().is_empty() {
                        new_lines.push(new_line);
                    }
                }
            }

            let cur_len = self.buffer().len();
            self.buffer_mut().remove_lines(0, cur_len);
            for l in new_lines {
                self.buffer_mut().push(l);
            }

            let target_line = sl.min(self.buffer().len().saturating_sub(1));
            if target_line < self.buffer().len() {
                let max_col = self.buffer().lines()[target_line]
                    .content()
                    .graphemes(true)
                    .count()
                    .saturating_sub(1);
                self.buffer_mut()
                    .set_cursor(target_line, sc.saturating_sub(1).min(max_col));
            } else if self.buffer().len() > 0 {
                self.buffer_mut().set_cursor(0, 0);
            }
        }
    }

    pub(super) fn grab_selection_to_chat(&mut self) {
        if let Some(start) = self.selection_start {
            let end = (self.buffer().cursor_line(), self.buffer().cursor_col());
            let is_line_wise = matches!(self.last_visual_mode, Some(Mode::VisualLine));
            let text = self.extract_text(start, end, is_line_wise);
            self.selection_start = None;

            let idx = self.llm_buffer_idx();
            self.active_buffer = idx;

            self.mode = Mode::Insert;
            self.editor.clear();

            self.pending_snippet = Some(text.clone());

            self.push_llm_line("```".to_string(), LineStyle::Info);
            for line in text.lines() {
                self.push_llm_line(line.to_string(), LineStyle::Dim);
            }
            self.push_llm_line("```".to_string(), LineStyle::Info);

            self.scroll_to_bottom();
        } else {
            let idx = self.llm_buffer_idx();
            self.active_buffer = idx;
            self.mode = Mode::Insert;
            self.editor.clear();
        }
    }
}
