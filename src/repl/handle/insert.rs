// src/repl/handle/insert.rs
//! Insert-mode key handling.

use super::super::*;
use crate::repl::misc::COMMAND_LIST;
use crate::repl::Mode;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::io;

impl Repl {
    pub(super) fn handle_insert_key(
        &mut self,
        key: KeyEvent,
        stdout: &mut io::Stdout,
    ) -> anyhow::Result<()> {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('a') | KeyCode::Char('A') => self.editor.move_home(),
                KeyCode::Char('e') | KeyCode::Char('E') => self.editor.move_end(),
                KeyCode::Char('u') | KeyCode::Char('U') => self.editor.kill_to_start(),
                KeyCode::Char('k') | KeyCode::Char('K') => self.editor.kill_to_end(),
                KeyCode::Char('w') | KeyCode::Char('W') => self.editor.kill_word_back(),
                _ => {}
            }
            self.render_spinner_only(stdout)?;
            return Ok(());
        }
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
            }
            KeyCode::Tab => {
                self.cmd_editor.tab_complete(COMMAND_LIST);
            }
            KeyCode::Enter => {
                if key.modifiers.contains(KeyModifiers::SHIFT)
                    || key.modifiers.contains(KeyModifiers::ALT)
                {
                    self.editor.insert_char('\n');
                } else {
                    let input = self.editor.submit();
                    self.submit_input(stdout, input)?;
                    return Ok(());
                }
            }
            KeyCode::Char(c) => {
                self.editor.insert_char(c);
            }
            KeyCode::Backspace => {
                self.editor.backspace();
            }
            KeyCode::Delete => {
                self.editor.delete();
            }
            KeyCode::Left => {
                self.editor.move_left();
            }
            KeyCode::Right => {
                self.editor.move_right();
            }
            KeyCode::Home => {
                self.editor.move_home();
            }
            KeyCode::End => {
                self.editor.move_end();
            }
            KeyCode::Up => {
                self.editor.history_up();
            }
            KeyCode::Down => {
                self.editor.history_down();
            }
            _ => {}
        }
        self.render_spinner_only(stdout)
    }
}
