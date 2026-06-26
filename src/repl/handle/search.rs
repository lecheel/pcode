// src/repl/handle/search.rs
//! Search-mode key handling + incremental search.

use super::super::*;
use crate::repl::Mode;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::io;

impl Repl {
    pub(super) fn handle_search_key(
        &mut self,
        key: KeyEvent,
        stdout: &mut io::Stdout,
    ) -> anyhow::Result<()> {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('a') | KeyCode::Char('A') => self.cmd_editor.move_home(),
                KeyCode::Char('e') | KeyCode::Char('E') => self.cmd_editor.move_end(),
                KeyCode::Char('u') | KeyCode::Char('U') => self.cmd_editor.kill_to_start(),
                KeyCode::Char('k') | KeyCode::Char('K') => self.cmd_editor.kill_to_end(),
                KeyCode::Char('w') | KeyCode::Char('W') => self.cmd_editor.kill_word_back(),
                _ => {}
            }
            self.render_spinner_only(stdout)?;
            return Ok(());
        }
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.cmd_editor.clear();
            }
            KeyCode::Enter => {
                self.cmd_editor.submit();
                self.mode = Mode::Normal;
            }
            KeyCode::Backspace => {
                self.cmd_editor.backspace();
                if self.cmd_editor.is_empty() {
                    self.mode = Mode::Normal;
                }
                self.update_search();
            }
            KeyCode::Char(c) => {
                self.cmd_editor.insert_char(c);
                self.update_search();
            }
            KeyCode::Left => self.cmd_editor.move_left(),
            KeyCode::Right => self.cmd_editor.move_right(),
            KeyCode::Home => self.cmd_editor.move_home(),
            KeyCode::End => self.cmd_editor.move_end(),
            _ => {}
        }
        self.render(stdout)
    }

    fn update_search(&mut self) {
        let query = self.cmd_editor.content().to_string();
        if query.is_empty() {
            self.search_query = None;
            self.search_matches.clear();
            self.search_match_idx = None;
            return;
        }
        let q_lower = query.to_lowercase();
        let matches: Vec<usize> = self
            .buffer()
            .lines()
            .iter()
            .enumerate()
            .filter_map(|(i, l)| {
                if l.content().to_lowercase().contains(&q_lower) {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        let changed = matches != self.search_matches;
        self.search_query = Some(query);
        self.search_matches = matches;
        if changed {
            self.search_match_idx = if self.search_matches.is_empty() {
                None
            } else {
                Some(0)
            };
            if let Some(idx) = self.search_match_idx {
                let target_line = self.search_matches[idx];
                self.buffer_mut().set_cursor(target_line, 0);
                self.ensure_cursor_visible();
            }
        }
    }
}
