// src/repl/handle/key.rs
//! Top-level key dispatcher.

use super::super::*;
use crate::repl::buffer::LineStyle;
use crate::repl::{Mode, PopupMode};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::io;

impl Repl {
    pub(super) fn handle_key(
        &mut self,
        key: KeyEvent,
        stdout: &mut io::Stdout,
    ) -> anyhow::Result<()> {
        if self.fkey_help && key.code != KeyCode::Char('?') {
            self.fkey_help = false;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.waiting {
                self.push_info(
                    "  ⏳ Still processing… press :q to force quit or F12 to abort",
                    LineStyle::Dim,
                );
                self.scroll_to_bottom();
                self.render(stdout)?;
                return Ok(());
            }
            match self.mode {
                Mode::Normal => self.handle_normal_key(key, stdout)?,
                Mode::Insert => self.handle_insert_key(key, stdout)?,
                Mode::Command => self.handle_command_key(key, stdout)?,
                Mode::Search => self.handle_search_key(key, stdout)?,
                Mode::Visual | Mode::VisualLine => self.handle_visual_key(key, stdout)?,
            }
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('e') {
            if !self.waiting {
                if self.popup.active {
                    self.popup.hide();
                } else {
                    self.show_task_file_picker();
                }
                self.render(stdout)?;
            }
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Char('b') {
            if self.popup.active {
                self.popup.hide();
            } else {
                self.show_buffer_picker();
            }
            self.render(stdout)?;
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            match key.code {
                KeyCode::Char('q') => {
                    self.editor.save_history(&self.config.repl.history_file);
                    self.cmd_editor
                        .save_history(&self.config.repl.command_history_file);
                    if let Some(handle) = self.agent_handle.take() {
                        handle.abort();
                    }
                    return Err(anyhow::anyhow!("__QUIT__"));
                }
                KeyCode::Char('e') => {
                    if !self.waiting {
                        if self.popup.active {
                            self.popup.hide();
                        } else {
                            self.show_file_picker();
                        }
                        self.render(stdout)?;
                    }
                    return Ok(());
                }
                KeyCode::Char('w') => {
                    if self.buffer().name() == "SedChanges" {
                        let _ = self.apply_sed_changes(stdout);
                    } else {
                        self.execute_command("write", stdout)?;
                    }
                    self.render(stdout)?;
                    return Ok(());
                }
                KeyCode::Char('d') => {
                    if !self.waiting {
                        let amount = self.count.unwrap_or(1);
                        self.do_dd(amount)?;
                        self.count = None;
                        self.pending = None;
                        self.render(stdout)?;
                    }
                    return Ok(());
                }
                KeyCode::Char('x') => {
                    self.close_buffer();
                    self.render(stdout)?;
                    return Ok(());
                }
                KeyCode::Char('-') => {
                    self.switch_buffer(-1);
                    self.render(stdout)?;
                    return Ok(());
                }
                KeyCode::Char('=') => {
                    self.switch_buffer(1);
                    self.render(stdout)?;
                    return Ok(());
                }
                _ => {}
            }
        }
        if key.code == KeyCode::F(12) {
            if self.waiting {
                if let Some(tx) = self.cancel_tx.take() {
                    let _ = tx.send(());
                }
                self.push_info("  ⛔ Cancelling agent task...", LineStyle::Error);
                self.scroll_to_bottom();
                self.render(stdout)?;
            }
            return Ok(());
        }
        if key.code == KeyCode::F(4) {
            self.show_git_hunk_popup();
            self.render(stdout)?;
            return Ok(());
        }
        if key.code == KeyCode::F(11) {
            self.execute_command("workflow", stdout)?;
            self.render(stdout)?;
            return Ok(());
        }
        if key.code == KeyCode::F(10) {
            if self.popup.active {
                self.popup.hide();
            } else {
                self.show_skill_group_popup();
            }
            self.render(stdout)?;
            return Ok(());
        }
        if key.code == KeyCode::F(9) {
            self.show_git_status(stdout, None)?;
            return Ok(());
        }

        let skill_idx = match key.code {
            KeyCode::F(1) => Some(0),
            KeyCode::F(2) => Some(4),
            KeyCode::F(3) => Some(7),
            _ => None,
        };
        if let Some(idx) = skill_idx {
            let groups = self.skill_groups();
            if idx < groups.len() && !self.waiting {
                let (emoji, name, description) = groups
                    .get(idx)
                    .map(|g| (g.emoji.clone(), g.name.clone(), g.description.clone()))
                    .unwrap_or_default();

                self.agent_mut().set_skill_group(idx);
                self.cached_skill_group = idx;
                self.popup.hide();

                self.push_info(
                    format!("  {} {} — {}", emoji, name, description),
                    LineStyle::ToolResult,
                );
                self.scroll_to_bottom();
                self.render(stdout)?;
            }
            return Ok(());
        }

        if self.popup.active {
            return self.handle_popup_key(key, stdout);
        }
        if matches!(self.mode, Mode::Command) {
            return self.handle_command_key(key, stdout);
        }
        if self.waiting {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    self.buffer_mut().move_down(1);
                    self.ensure_cursor_visible();
                    self.render(stdout)?;
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.buffer_mut().move_up(1);
                    self.ensure_cursor_visible();
                    self.render(stdout)?;
                }
                KeyCode::Char(':') => {
                    self.mode = Mode::Command;
                    self.cmd_editor.clear();
                    self.render_spinner_only(stdout)?;
                }
                _ => {}
            }
            return Ok(());
        }
        match self.mode {
            Mode::Normal => self.handle_normal_key(key, stdout)?,
            Mode::Insert => self.handle_insert_key(key, stdout)?,
            Mode::Command => self.handle_command_key(key, stdout)?,
            Mode::Search => self.handle_search_key(key, stdout)?,
            Mode::Visual | Mode::VisualLine => self.handle_visual_key(key, stdout)?,
        }
        Ok(())
    }
}
