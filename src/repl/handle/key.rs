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
        if self.mode == Mode::Merge {
            return self.handle_merge_key(key, stdout);
        }
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
                Mode::GitLog => self.handle_glog_key(key, stdout)?,
                Mode::GitDiff => self.handle_gdiff_key(key, stdout)?,
                Mode::FilePicker => self.handle_file_picker_key(key, stdout)?,
            }
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('e') {
            if !self.waiting {
                if self.mode == Mode::FilePicker {
                    self.mode = Mode::Normal;
                    self.file_picker = None;
                } else if self.popup.active {
                    self.popup.hide();
                } else {
                    self.show_task_file_picker();
                }
                self.render(stdout)?;
            }
            return Ok(());
        }
        if (key.modifiers.contains(KeyModifiers::ALT) || key.modifiers.contains(KeyModifiers::META))
            && (key.code == KeyCode::Char('b') || key.code == KeyCode::Char('B'))
        {
            if self.popup.active {
                self.popup.hide();
            } else {
                self.show_buffer_picker();
            }
            self.render(stdout)?;
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::ALT) || key.modifiers.contains(KeyModifiers::META) {
            match key.code {
                KeyCode::Char('q') | KeyCode::Char('Q') => {
                    self.editor.save_history(&self.config.repl.history_file);
                    self.cmd_editor
                        .save_history(&self.config.repl.command_history_file);
                    if let Some(handle) = self.agent_handle.take() {
                        handle.abort();
                    }
                    return Err(anyhow::anyhow!("__QUIT__"));
                }
                KeyCode::Char('e') | KeyCode::Char('E') => {
                    if !self.waiting {
                        if self.mode == Mode::FilePicker {
                            self.mode = Mode::Normal;
                            self.file_picker = None;
                        } else if self.popup.active {
                            self.popup.hide();
                        } else {
                            self.show_file_picker();
                        }
                        self.render(stdout)?;
                    }
                    return Ok(());
                }
                KeyCode::Char('w') | KeyCode::Char('W') => {
                    if self.buffer().name() == "SedChanges" {
                        let _ = self.apply_sed_changes(stdout);
                    } else {
                        self.execute_command("write", stdout)?;
                    }
                    self.render(stdout)?;
                    return Ok(());
                }
                KeyCode::Char('d') | KeyCode::Char('D') => {
                    if !self.waiting {
                        let amount = self.count.unwrap_or(1);
                        self.do_dd(amount)?;
                        self.count = None;
                        self.pending = None;
                        self.render(stdout)?;
                    }
                    return Ok(());
                }
                KeyCode::Char('x') | KeyCode::Char('X') => {
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
                self.enter_gdiff_mode(stdout)?;
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
        if key.code == KeyCode::F(6) {
            self.execute_command("glog", stdout)?;
            self.render(stdout)?;
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
        if key.code == KeyCode::Insert {
            if !self.waiting {
                match arboard::Clipboard::new().and_then(|mut cb| cb.get_text()) {
                    Ok(content) => {
                        if !content.trim().is_empty() {
                            let hunks = crate::patch::parse_patches(&content);
                            if !hunks.is_empty() {
                                self.merge_buffer_apply = true;
                                self.start_merge(hunks);
                            } else {
                                self.pending_snippet = Some(content);
                                self.push_info(
                                    "  📋 Pasted snippet. Press 'i' and type your prompt:",
                                    LineStyle::Dim,
                                );
                                self.scroll_to_bottom();
                            }
                        }
                    }
                    Err(e) => {
                        self.push_info(format!("  ❌ Clipboard Error: {}", e), LineStyle::Error);
                        self.scroll_to_bottom();
                    }
                }
                self.render(stdout)?;
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
            Mode::GitLog => self.handle_glog_key(key, stdout)?,
            Mode::GitDiff => self.handle_gdiff_key(key, stdout)?,
            Mode::FilePicker => self.handle_file_picker_key(key, stdout)?,
        }
        Ok(())
    }

    pub(super) fn handle_file_picker_key(
        &mut self,
        key: KeyEvent,
        stdout: &mut io::Stdout,
    ) -> anyhow::Result<()> {
        if let Some(picker) = self.file_picker.as_mut() {
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => picker.move_down(),
                KeyCode::Up | KeyCode::Char('k') => picker.move_up(),
                KeyCode::Char(' ') => picker.toggle_selection(),
                KeyCode::Tab | KeyCode::Char('l') => picker.toggle_expand(),
                KeyCode::Char('c') => {
                    let root = std::path::PathBuf::from(&self.config.tools.project_root);
                    let mut content = String::new();
                    let mut files = Vec::new();
                    for path in &picker.selected {
                        files.push(path.clone());
                    }
                    files.sort();
                    for path in &files {
                        content.push_str(&format!("// {}\n", path));
                        if let Ok(c) = std::fs::read_to_string(root.join(path)) {
                            content.push_str(&c);
                            if !c.ends_with('\n') {
                                content.push('\n');
                            }
                            content.push('\n');
                        }
                    }
                    match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(content)) {
                        Ok(_) => {
                            self.push_command_info(
                                format!("  📋 Copied {} files to clipboard:", files.len()),
                                LineStyle::ToolResult,
                            );
                            self.mode = Mode::Normal;
                            self.file_picker = None;
                        }
                        Err(e) => {
                            self.push_command_info(
                                format!("  ❌ Clipboard error: {}", e),
                                LineStyle::Error,
                            );
                        }
                    }
                }
                KeyCode::Enter => {
                    if let Some(node) = picker.flat_nodes.get(picker.cursor) {
                        if !node.is_dir {
                            let path = node.path.clone();
                            self.load_file_to_buffer(&path, stdout)?;
                            self.mode = Mode::Normal;
                            self.file_picker = None;
                        } else {
                            picker.toggle_expand();
                        }
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.mode = Mode::Normal;
                    self.file_picker = None;
                }
                KeyCode::Char(c) => {
                    picker.filter.push(c);
                    picker.update_flat();
                }
                KeyCode::Backspace => {
                    picker.filter.pop();
                    picker.update_flat();
                }
                _ => {}
            }
        }
        self.render(stdout)?;
        Ok(())
    }
    pub(super) fn handle_gdiff_key(
        &mut self,
        key: KeyEvent,
        stdout: &mut io::Stdout,
    ) -> anyhow::Result<()> {
        match key.code {
            KeyCode::Tab => {
                self.gdiff_left_active = !self.gdiff_left_active;
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                self.mode = Mode::Normal;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if self.gdiff_cursor < self.gdiff_rows.len().saturating_sub(1) {
                    self.gdiff_cursor += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.gdiff_cursor = self.gdiff_cursor.saturating_sub(1);
            }
            KeyCode::Char('l') => {
                let mut found = None;
                for i in (self.gdiff_cursor + 1)..self.gdiff_rows.len() {
                    if self.gdiff_rows[i].kind != crate::diff::RowKind::Equal {
                        found = Some(i);
                        break;
                    }
                }
                if let Some(idx) = found {
                    self.gdiff_cursor = idx;
                }
            }
            KeyCode::Char('h') => {
                if self.gdiff_cursor > 0 {
                    let mut found = None;
                    for i in (0..self.gdiff_cursor).rev() {
                        if self.gdiff_rows[i].kind != crate::diff::RowKind::Equal {
                            found = Some(i);
                            break;
                        }
                    }
                    if let Some(idx) = found {
                        self.gdiff_cursor = idx;
                    }
                }
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                let vis = self.response_area_height().saturating_sub(2);
                self.gdiff_cursor =
                    (self.gdiff_cursor + vis).min(self.gdiff_rows.len().saturating_sub(1));
            }
            KeyCode::PageUp => {
                let vis = self.response_area_height().saturating_sub(2);
                self.gdiff_cursor = self.gdiff_cursor.saturating_sub(vis);
            }
            _ => {}
        }
        self.render(stdout)?;
        Ok(())
    }

    pub(super) fn handle_glog_key(
        &mut self,
        key: KeyEvent,
        stdout: &mut io::Stdout,
    ) -> anyhow::Result<()> {
        match key.code {
            KeyCode::Tab => {
                self.glog_left_active = !self.glog_left_active;
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                self.mode = Mode::Normal;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if self.glog_left_active {
                    if self.glog_left_cursor < self.glog_commits.len().saturating_sub(1) {
                        self.glog_left_cursor += 1;
                        self.glog_right_scroll = 0;
                        self.glog_right_cursor = 0;
                    }
                } else {
                    self.glog_right_cursor += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.glog_left_active {
                    if self.glog_left_cursor > 0 {
                        self.glog_left_cursor -= 1;
                        self.glog_right_scroll = 0;
                        self.glog_right_cursor = 0;
                    }
                } else {
                    self.glog_right_cursor = self.glog_right_cursor.saturating_sub(1);
                }
            }
            KeyCode::PageDown => {
                let vis = self.response_area_height().saturating_sub(2);
                if self.glog_left_active {
                    self.glog_left_cursor = (self.glog_left_cursor + vis)
                        .min(self.glog_commits.len().saturating_sub(1));
                    self.glog_right_scroll = 0;
                    self.glog_right_cursor = 0;
                } else {
                    self.glog_right_cursor += vis;
                }
            }
            KeyCode::PageUp => {
                let vis = self.response_area_height().saturating_sub(2);
                if self.glog_left_active {
                    self.glog_left_cursor = self.glog_left_cursor.saturating_sub(vis);
                    self.glog_right_scroll = 0;
                    self.glog_right_cursor = 0;
                } else {
                    self.glog_right_cursor = self.glog_right_cursor.saturating_sub(vis);
                }
            }
            KeyCode::Char('c') => {
                if !self.glog_selected_commits.is_empty() {
                    let mut files = Vec::new();
                    for h in &self.glog_selected_commits {
                        let output = std::process::Command::new("git")
                            .arg("show")
                            .arg("--name-only")
                            .arg("--pretty=format:")
                            .arg(h)
                            .output();
                        if let Ok(out) = output {
                            let s = String::from_utf8_lossy(&out.stdout);
                            for f in s.lines() {
                                let f = f.trim();
                                if !f.is_empty() && !files.contains(&f.to_string()) {
                                    files.push(f.to_string());
                                }
                            }
                        }
                    }
                    let mut content = String::new();
                    for f in &files {
                        content.push_str(&format!("// {}\n", f));
                        if let Ok(c) = std::fs::read_to_string(f) {
                            content.push_str(&c);
                            if !c.ends_with('\n') {
                                content.push('\n');
                            }
                        } else {
                            content.push_str("<failed to read file>\n");
                        }
                    }
                    match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(content)) {
                        Ok(_) => {
                            self.push_command_info(
                                format!("  📋 Copied {} files to clipboard:", files.len()),
                                LineStyle::ToolResult,
                            );
                            for f in &files {
                                self.push_command_info(format!("    • {}", f), LineStyle::Dim);
                            }
                            self.glog_selected_commits.clear();
                        }
                        Err(e) => {
                            self.push_command_info(
                                format!("  ❌ Clipboard error: {}", e),
                                LineStyle::Error,
                            );
                        }
                    }
                    self.mode = Mode::Normal;
                } else {
                    self.mode = Mode::Normal;
                    self.push_command_info("  No commits selected", LineStyle::Dim);
                }
            }
            KeyCode::Char(' ') => {
                if self.glog_left_active {
                    let hash_short = self.glog_commits[self.glog_left_cursor]
                        .get(..7)
                        .unwrap_or(&self.glog_commits[self.glog_left_cursor])
                        .to_string();
                    if let Some(idx) = self
                        .glog_selected_commits
                        .iter()
                        .position(|c| c == &hash_short)
                    {
                        self.glog_selected_commits.remove(idx);
                    } else {
                        self.glog_selected_commits.push(hash_short);
                    }
                    if self.glog_left_cursor < self.glog_commits.len().saturating_sub(1) {
                        self.glog_left_cursor += 1;
                        self.glog_right_scroll = 0;
                        self.glog_right_cursor = 0;
                    }
                }
            }
            _ => {}
        }
        self.render(stdout)?;
        Ok(())
    }
}