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
                Mode::Merge => self.handle_merge_key(key, stdout)?,
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
                    if self.mode == Mode::Merge {
                        let mut saved_count = 0;
                        let modified_files: Vec<String> =
                            self.modified_buffers.iter().cloned().collect();
                        for file in &modified_files {
                            if let Some(idx) = self.buffers.iter().position(|b| b.name() == file) {
                                let root =
                                    std::path::PathBuf::from(&self.config.tools.project_root);
                                let path = root.join(file);
                                let content: String = self.buffers[idx]
                                    .lines()
                                    .iter()
                                    .map(|l| l.content().clone())
                                    .collect::<Vec<String>>()
                                    .join("\n");
                                if std::fs::write(&path, content).is_ok() {
                                    saved_count += 1;
                                }
                            }
                        }
                        self.modified_buffers.clear();
                        self.pending_merge = None;
                        self.mode = Mode::Normal;
                        self.push_info(
                            format!(
                                "  💾 Saved {} buffer(s) to disk. Exited Merge Mode.",
                                saved_count
                            ),
                            LineStyle::ToolResult,
                        );
                        self.render(stdout)?;
                        return Ok(());
                    }
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
            if self.pending_snippet.take().is_some() {
                self.editor.clear();
                self.push_info("  ❌ Cancelled snippet input.", LineStyle::Dim);
                self.scroll_to_bottom();
                self.render(stdout)?;
                return Ok(());
            }
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
        if key.code == KeyCode::F(7) {
            if !self.waiting {
                if self.popup.active {
                    self.popup.hide();
                } else {
                    self.show_function_list_popup_legacy();
                }
                self.render(stdout)?;
            }
            return Ok(());
        }

        if key.code == KeyCode::F(8) {
            if !self.waiting {
                if self.popup.active {
                    self.popup.hide();
                } else {
                    self.show_function_list_popup();
                }
                self.render(stdout)?;
            }
            return Ok(());
        }

        if key.code == KeyCode::F(9) {
            if self.mode == Mode::Merge {
                return self.handle_merge_key(key, stdout);
            }
            if !self.waiting {
                let content: String = self
                    .buffer()
                    .lines()
                    .iter()
                    .map(|l| l.content())
                    .collect::<Vec<String>>()
                    .join("\n");
                let hunks = crate::patch::parse_patches(&content);
                if !hunks.is_empty() {
                    self.merge_buffer_apply = true;
                    self.start_merge(hunks);
                } else {
                    self.push_info("  ❌ No patches found in current buffer.", LineStyle::Error);
                    self.scroll_to_bottom();
                }
                self.render(stdout)?;
            }
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
        if key.code == KeyCode::F(1) {
            self.show_git_status(stdout, None)?;
            return Ok(());
        }

        // 1. Check if any skill group explicitly claims this F-key
        let key_str = match key.code {
            KeyCode::F(n) => Some(format!("F{}", n)),
            _ => None,
        };

        if let Some(k) = key_str {
            if !self.waiting {
                if let Some(idx) = self
                    .skill_groups()
                    .iter()
                    .position(|g| g.key.as_deref() == Some(k.as_str()))
                {
                    let (emoji, name, description) = self
                        .skill_groups()
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
                    return Ok(());
                }
            }
        }

        // 2. Fallback to default aliases for F1, F2, F3 if not explicitly mapped
        let target_skill_name = match key.code {
            KeyCode::F(1) => Some("chat"),
            KeyCode::F(2) => Some("edit"),
            KeyCode::F(3) => Some("full"),
            _ => None,
        };

        if let Some(name) = target_skill_name {
            if !self.waiting {
                if let Some(idx) = self.agent_mut().set_skill_group_by_name(name) {
                    self.cached_skill_group = idx;
                    self.popup.hide();

                    let (emoji, skill_name, description) = self
                        .skill_groups()
                        .get(idx)
                        .map(|g| (g.emoji.clone(), g.name.clone(), g.description.clone()))
                        .unwrap_or_default();

                    self.push_info(
                        format!("  {} {} — {}", emoji, skill_name, description),
                        LineStyle::ToolResult,
                    );
                    self.scroll_to_bottom();
                    self.render(stdout)?;
                }
            }
            return Ok(());
        }
        if self.popup.active && self.popup_mode != PopupMode::WhichKey {
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
            Mode::Merge => self.handle_merge_key(key, stdout)?,
        }
        Ok(())
    }
}
