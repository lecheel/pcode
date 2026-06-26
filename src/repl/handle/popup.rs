// src/repl/handle/popup.rs
//! Popup key handling + file/buffer/task pickers + file loading.

use super::super::*;
use crate::repl::buffer::{BufferLine, LineStyle, ResponseBuffer};
use crate::repl::helper::{PopupItem, PopupPosition};
use crate::repl::misc::{list_impl_files, list_project_files};
use crate::repl::{Mode, PopupMode};
use crossterm::event::{KeyCode, KeyEvent};
use std::io;

impl Repl {
    pub(super) fn handle_popup_key(
        &mut self,
        key: KeyEvent,
        stdout: &mut io::Stdout,
    ) -> anyhow::Result<()> {
        match key.code {
            KeyCode::Down | KeyCode::Tab => {
                self.popup.move_down();
            }
            KeyCode::Up | KeyCode::BackTab => {
                self.popup.move_up();
            }
            KeyCode::Enter => {
                if !self.waiting && !self.popup.items.is_empty() {
                    match self.popup_mode {
                        PopupMode::SkillGroups => {
                            if let Some(item) = self.popup.items.get(self.popup.cursor) {
                                if let Some(idx) = item.id {
                                    self.set_skill_group(idx, stdout)?;
                                }
                            }
                        }
                        PopupMode::FilePicker | PopupMode::TaskFilePicker => {
                            if let Some(item) = self.popup.items.get(self.popup.cursor) {
                                let path = item.text.clone();
                                self.load_file_to_buffer(&path, stdout)?;
                            }
                        }
                        PopupMode::Buffers => {
                            if let Some(item) = self.popup.items.get(self.popup.cursor) {
                                if let Some(idx) = item.id {
                                    if idx < self.buffers.len() {
                                        self.active_buffer = idx;
                                        self.scroll_to_bottom();
                                    }
                                }
                            }
                        }
                        PopupMode::GitHunks => {
                            if let Some(item) = self.popup.items.get(self.popup.cursor) {
                                if let Some(line_num) = item.id {
                                    let target_line = line_num - 1;
                                    self.buffer_mut().set_cursor(target_line, 0);
                                    self.center_cursor();
                                }
                            }
                        }
                    }
                }
                self.popup.hide();
            }
            KeyCode::Esc => {
                self.popup.hide();
            }
            KeyCode::Char('j') if matches!(self.popup_mode, PopupMode::SkillGroups) => {
                self.popup.move_down();
            }
            KeyCode::Char('k') if matches!(self.popup_mode, PopupMode::SkillGroups) => {
                self.popup.move_up();
            }
            KeyCode::Char('q') if matches!(self.popup_mode, PopupMode::SkillGroups) => {
                self.popup.hide();
            }
            KeyCode::Char(c) => {
                if self.popup.show_filter {
                    self.popup.filter.push(c);
                    let f = self.popup.filter.clone();
                    self.popup.update_filter(&f);
                }
            }
            KeyCode::Backspace => {
                if self.popup.show_filter {
                    self.popup.filter.pop();
                    let f = self.popup.filter.clone();
                    self.popup.update_filter(&f);
                }
            }
            _ => {}
        }
        self.render(stdout)
    }

    pub(super) fn show_file_picker(&mut self) {
        let root = std::path::PathBuf::from(&self.config.tools.project_root);
        let files = list_project_files(&root);
        let items: Vec<PopupItem> = files
            .iter()
            .map(|f| PopupItem {
                text: f.clone(),
                is_active: false,
                id: None,
            })
            .collect();
        self.popup_mode = PopupMode::FilePicker;
        self.popup
            .show("Open File", items, 0, PopupPosition::Center);
    }

    pub(super) fn show_task_file_picker(&mut self) {
        let root = std::path::PathBuf::from(&self.config.tools.project_root);
        let impl_dir = root.join(".impl");
        let files = list_impl_files(&root, &impl_dir);
        let items: Vec<PopupItem> = files
            .iter()
            .map(|f| PopupItem {
                text: f.clone(),
                is_active: false,
                id: None,
            })
            .collect();
        self.popup_mode = PopupMode::TaskFilePicker;
        self.popup
            .show("Task Files (.impl)", items, 0, PopupPosition::Center);
    }

    pub(super) fn show_buffer_picker(&mut self) {
        let items: Vec<PopupItem> = self
            .buffers
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let is_active = i == self.active_buffer;
                let name = if b.name().is_empty() {
                    "Untitled".to_string()
                } else {
                    b.name().to_string()
                };
                let text = format!("[{}] {} ({} lines)", i + 1, name, b.len());
                PopupItem {
                    text,
                    is_active,
                    id: Some(i),
                }
            })
            .collect();
        self.popup_mode = PopupMode::Buffers;
        self.popup
            .show("Buffers", items, self.active_buffer, PopupPosition::Center);
    }

    pub(super) fn load_file_to_buffer(
        &mut self,
        path: &str,
        stdout: &mut io::Stdout,
    ) -> anyhow::Result<()> {
        let root = std::path::PathBuf::from(&self.config.tools.project_root);
        let raw_path = std::path::Path::new(path);
        let resolved = if raw_path.is_absolute() {
            raw_path.to_path_buf()
        } else {
            root.join(path)
        };
        let canonical_target = match resolved.canonicalize() {
            Ok(p) => p,
            Err(e) => {
                self.push_command_info(
                    format!("  ❌ Failed to resolve {}: {}", path, e),
                    LineStyle::Error,
                );
                self.scroll_to_bottom();
                self.render(stdout)?;
                return Ok(());
            }
        };
        let canonical_root = match root.canonicalize() {
            Ok(p) => p,
            Err(e) => {
                self.push_command_info(
                    format!("  ❌ Invalid project root: {}", e),
                    LineStyle::Error,
                );
                self.scroll_to_bottom();
                self.render(stdout)?;
                return Ok(());
            }
        };
        if !canonical_target.starts_with(&canonical_root) {
            self.push_command_info(
                format!("  ❌ Access denied: '{}' is outside the project root", path),
                LineStyle::Error,
            );
            self.scroll_to_bottom();
            self.render(stdout)?;
            return Ok(());
        }
        match std::fs::read_to_string(&canonical_target) {
            Ok(content) => {
                let new_buf_idx = self.buffers.len();
                self.buffers.push(ResponseBuffer::with_name(path));
                self.active_buffer = new_buf_idx;

                let c_idx = self.console_buffer_idx();
                self.buffers[c_idx].push(BufferLine::new(
                    format!("  📄 Opened: {}", path),
                    LineStyle::Info,
                ));

                self.buffer_mut().push_str(&content, LineStyle::Plain);
                self.scroll_to_bottom();
            }
            Err(e) => {
                self.push_command_info(
                    format!("  ❌ Failed to read {}: {}", path, e),
                    LineStyle::Error,
                );
                self.scroll_to_bottom();
            }
        }
        self.render(stdout)
    }
}
