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
                        PopupMode::FunctionList => {
                            if let Some(item) = self.popup.items.get(self.popup.cursor) {
                                if let Some(line_num) = item.id {
                                    self.buffer_mut().set_cursor(line_num, 0);
                                    self.center_cursor();
                                }
                            }
                        }
                        PopupMode::WhichKey => {
                            // WhichKey popup is not interactive; handled by normal mode
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

    pub(super) fn show_function_list_popup(&mut self) {
        let lines = self.buffer().lines();
        let content: String = lines
            .iter()
            .map(|l| l.content())
            .collect::<Vec<_>>()
            .join("\n");

        let mut items = Vec::new();
        let language: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
        let mut parser = tree_sitter::Parser::new();

        if parser.set_language(&language).is_ok() {
            if let Some(tree) = parser.parse(&content, None) {
                let mut nodes = Vec::new();
                let mut stack = vec![tree.root_node()];

                while let Some(node) = stack.pop() {
                    let kind = node.kind();
                    if matches!(
                        kind,
                        "function_item"
                            | "struct_item"
                            | "enum_item"
                            | "trait_item"
                            | "impl_item"
                            | "macro_definition"
                            | "mod_item"
                            | "const_item"
                    ) {
                        nodes.push(node);
                    }

                    for i in 0..node.child_count() {
                        if let Some(child) = node.child(i as u32) {
                            stack.push(child);
                        }
                    }
                }

                nodes.sort_by_key(|n| n.start_position().row);
                nodes.dedup_by_key(|n| n.start_position().row);

                for node in nodes {
                    let start_row = node.start_position().row;
                    if let Some(line) = lines.get(start_row) {
                        let line_text = line.content().trim().to_string();
                        let display_text = if line_text.chars().count() > 80 {
                            let truncated: String = line_text.chars().take(77).collect();
                            format!("{}...", truncated)
                        } else {
                            line_text
                        };
                        items.push(PopupItem {
                            text: format!("L{:>4}: {}", start_row + 1, display_text),
                            is_active: start_row == self.buffer().cursor_line(),
                            id: Some(start_row),
                        });
                    }
                }
            }
        }

        // Fallback to string matching if tree-sitter fails
        if items.is_empty() {
            for (i, line) in lines.iter().enumerate() {
                let line_content = line.content();
                let trimmed = line_content.trim_start();
                if trimmed.starts_with("fn ")
                    || trimmed.starts_with("pub fn ")
                    || trimmed.starts_with("pub(crate) fn ")
                    || trimmed.starts_with("pub(super) fn ")
                    || trimmed.starts_with("async fn ")
                    || trimmed.starts_with("pub async fn ")
                    || trimmed.starts_with("impl ")
                    || trimmed.starts_with("struct ")
                    || trimmed.starts_with("enum ")
                    || trimmed.starts_with("trait ")
                {
                    let display_text = if line_content.chars().count() > 80 {
                        let truncated: String = line_content.chars().take(77).collect();
                        format!("{}...", truncated)
                    } else {
                        line_content.clone()
                    };
                    items.push(PopupItem {
                        text: format!("L{:>4}: {}", i + 1, display_text),
                        is_active: i == self.buffer().cursor_line(),
                        id: Some(i),
                    });
                }
            }
        }

        self.popup_mode = PopupMode::FunctionList;
        self.popup.show("Symbols", items, 0, PopupPosition::Center);
    }

    pub(super) fn show_function_list_popup_legacy(&mut self) {
        let lines = self.buffer().lines();
        let mut items = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let content = line.content();
            let trimmed = content.trim_start();
            if trimmed.starts_with("fn ")
                || trimmed.starts_with("pub fn ")
                || trimmed.starts_with("pub(crate) fn ")
                || trimmed.starts_with("pub(super) fn ")
                || trimmed.starts_with("async fn ")
                || trimmed.starts_with("pub async fn ")
                || trimmed.starts_with("def ")
                || trimmed.starts_with("function ")
                || trimmed.starts_with("export function ")
                || trimmed.starts_with("export async function ")
                || trimmed.starts_with("impl ")
                || trimmed.starts_with("struct ")
                || trimmed.starts_with("enum ")
                || trimmed.starts_with("trait ")
                || trimmed.starts_with("interface ")
            {
                let display_text = if content.chars().count() > 80 {
                    let truncated: String = content.chars().take(77).collect();
                    format!("{}...", truncated)
                } else {
                    content.clone()
                };
                items.push(PopupItem {
                    text: format!("L{:>4}: {}", i + 1, display_text),
                    is_active: i == self.buffer().cursor_line(),
                    id: Some(i),
                });
            }
        }
        self.popup_mode = PopupMode::FunctionList;
        self.popup
            .show("Functions", items, 0, PopupPosition::Center);
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
