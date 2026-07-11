// src/repl/handle/normal.rs
//! Normal-mode key handling, `dd` deletion, word lookup.

use super::super::*;
use crate::repl::buffer::{BufferLine, LineStyle};
use crate::repl::helper::{PopupItem, PopupPosition};
use crate::repl::misc;
use crate::repl::{CommandResult, Mode, RepeatAction};
use crossterm::{
    cursor,
    event::{KeyCode, KeyEvent, KeyModifiers},
    execute, terminal,
};
use std::io;

impl Repl {
    pub(super) fn handle_normal_repeat(
        &mut self,
        key: KeyEvent,
        stdout: &mut io::Stdout,
    ) -> anyhow::Result<()> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.buffer_mut().move_down(1);
                self.ensure_cursor_visible();
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.buffer_mut().move_up(1);
                self.ensure_cursor_visible();
            }
            _ => return Ok(()),
        }
        self.render(stdout)
    }

    pub(super) fn handle_normal_key(
        &mut self,
        key: KeyEvent,
        stdout: &mut io::Stdout,
    ) -> anyhow::Result<()> {
        if key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Char('w') {
            let result = self.execute_command("w", stdout)?;
            if let CommandResult::Quit = result {
                self.editor.save_history(&self.config.repl.history_file);
                self.cmd_editor
                    .save_history(&self.config.repl.command_history_file);
                if let Some(handle) = self.agent_handle.take() {
                    handle.abort();
                }
                return Err(anyhow::anyhow!("__QUIT__"));
            }
            self.render(stdout)?;
            return Ok(());
        }
        if let Some(target) = self.pending_reset_target.take() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    let out = std::process::Command::new("git")
                        .arg("checkout")
                        .arg("HEAD")
                        .arg("--")
                        .arg(&target)
                        .output();
                    if let Ok(o) = out {
                        let msg = String::from_utf8_lossy(&o.stdout);
                        let err = String::from_utf8_lossy(&o.stderr);
                        let c_idx = self.console_buffer_idx();
                        if !msg.trim().is_empty() {
                            self.buffers[c_idx].push(BufferLine::new(
                                format!("  ↩️  {}", msg.trim()),
                                LineStyle::Info,
                            ));
                        }
                        if !err.trim().is_empty() {
                            self.buffers[c_idx].push(BufferLine::new(
                                format!("  ❌ {}", err.trim()),
                                LineStyle::Error,
                            ));
                        }
                    }
                    self.show_git_status(stdout, Some(&target))?;
                }
                _ => {
                    self.push_line("  Cancelled reset.", LineStyle::Dim);
                    self.scroll_to_bottom();
                    self.render(stdout)?;
                }
            }
            return Ok(());
        }
        if let Some(target) = self.stash_pop_target.take() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    let out = std::process::Command::new("git")
                        .arg("stash")
                        .arg("pop")
                        .arg(&target)
                        .output();
                    if let Ok(o) = out {
                        let msg = String::from_utf8_lossy(&o.stdout);
                        let err = String::from_utf8_lossy(&o.stderr);
                        let c_idx = self.console_buffer_idx();
                        if !msg.trim().is_empty() {
                            self.buffers[c_idx].push(BufferLine::new(
                                format!("  📦 {}", msg.trim()),
                                LineStyle::Info,
                            ));
                        }
                        if !err.trim().is_empty() {
                            self.buffers[c_idx].push(BufferLine::new(
                                format!("  ❌ {}", err.trim()),
                                LineStyle::Error,
                            ));
                        }
                    }
                    self.show_git_status(stdout, None)?;
                }
                _ => {
                    self.push_line("  Cancelled stash pop.", LineStyle::Dim);
                    self.scroll_to_bottom();
                    self.render(stdout)?;
                }
            }
            return Ok(());
        }

        if key.code == KeyCode::Enter && self.buffer().name() == "fd" {
            let cursor_line = self.buffer().cursor_line();
            if let Some(line) = self.buffer().lines().get(cursor_line) {
                let content = line.content().trim().to_string();
                if !content.is_empty()
                    && !content.starts_with('[')
                    && !content.starts_with("No matches")
                {
                    self.load_file_to_buffer(&content, stdout)?;
                    return Ok(());
                }
            }
        }

        if key.code == KeyCode::Enter && self.buffer().name() == "rg" {
            let cursor_line = self.buffer().cursor_line();
            if let Some(line) = self.buffer().lines().get(cursor_line) {
                let content = line.content();

                if let Some(colon1) = content.find(':') {
                    if let Some(colon2) = content[colon1 + 1..].find(':') {
                        let file = content[..colon1].to_string();
                        let line_num_str = &content[colon1 + 1..colon1 + 1 + colon2];
                        if let Ok(line_num) = line_num_str.parse::<usize>() {
                            if !file.is_empty() && !file.contains(' ') {
                                self.load_file_to_buffer(&file, stdout)?;
                                let target_line = line_num.saturating_sub(1);
                                if target_line < self.buffer().len() {
                                    self.buffer_mut().set_cursor(target_line, 0);
                                    let h = self.response_area_height();
                                    let w = self.content_width();
                                    self.buffer_mut().ensure_cursor_visible(h, w);
                                }
                                self.render(stdout)?;
                                return Ok(());
                            }
                        }
                    }
                }

                if let Some(colon1) = content.find(':') {
                    let line_num_str = &content[..colon1];
                    if let Ok(line_num) = line_num_str.parse::<usize>() {
                        let mut file_to_open = None;
                        for i in (0..cursor_line).rev() {
                            if let Some(prev_line) = self.buffer().lines().get(i) {
                                let prev_content = prev_line.content();
                                if prev_content.is_empty() {
                                    continue;
                                }
                                if prev_content.starts_with(|c: char| c.is_ascii_digit())
                                    && prev_content.contains(':')
                                {
                                    continue;
                                }
                                file_to_open = Some(prev_content.clone());
                                break;
                            }
                        }

                        if let Some(file) = file_to_open {
                            self.load_file_to_buffer(&file, stdout)?;
                            let target_line = line_num.saturating_sub(1);
                            if target_line < self.buffer().len() {
                                self.buffer_mut().set_cursor(target_line, 0);
                                let h = self.response_area_height();
                                let w = self.content_width();
                                self.buffer_mut().ensure_cursor_visible(h, w);
                            }
                            self.render(stdout)?;
                            return Ok(());
                        }
                    }
                }
            }
        }

        if key.code == KeyCode::Enter && self.buffer().name() == "GitLog" {
            let cursor_line = self.buffer().cursor_line();
            if let Some(line) = self.buffer().lines().get(cursor_line) {
                let content = line.content().trim().to_string();
                let mut hash_opt = None;
                for part in content.split_whitespace() {
                    if part.len() >= 7 && part.chars().all(|c| c.is_ascii_hexdigit()) {
                        hash_opt = Some(part.to_string());
                        break;
                    }
                }

                if let Some(hash) = hash_opt {
                    let output = std::process::Command::new("git")
                        .arg("show")
                        .arg(&hash)
                        .output();

                    let new_buf_idx = if let Some(idx) =
                        self.buffers.iter().position(|b| b.name() == "GitCommit")
                    {
                        idx
                    } else {
                        let idx = self.buffers.len();
                        self.buffers.push(ResponseBuffer::with_name("GitCommit"));
                        idx
                    };
                    self.active_buffer = new_buf_idx;
                    self.buffers[new_buf_idx].clear();

                    if let Ok(out) = output {
                        let s = String::from_utf8_lossy(&out.stdout);
                        for line in s.lines() {
                            let style = if line.starts_with("commit ") {
                                LineStyle::User
                            } else if line.starts_with("Author:")
                                || line.starts_with("Date:")
                                || line.starts_with("Merge:")
                            {
                                LineStyle::Info
                            } else if line.starts_with("diff ") || line.starts_with("index ") {
                                LineStyle::Info
                            } else if line.starts_with("+++") || line.starts_with("---") {
                                LineStyle::Tool
                            } else if line.starts_with("@@") {
                                LineStyle::Tool
                            } else if line.starts_with("+") {
                                LineStyle::ToolResult
                            } else if line.starts_with("-") {
                                LineStyle::Error
                            } else {
                                LineStyle::Plain
                            };
                            let prefix = if line.starts_with("+") || line.starts_with("-") {
                                ""
                            } else {
                                ""
                            };
                            self.buffers[new_buf_idx]
                                .push(BufferLine::new(format!("{}{}", prefix, line), style));
                        }
                    } else {
                        self.buffers[new_buf_idx].push(BufferLine::new(
                            "  Failed to run git show",
                            LineStyle::Error,
                        ));
                    }
                    self.buffers[new_buf_idx].move_top();
                    self.ensure_cursor_visible();
                    self.render(stdout)?;
                    return Ok(());
                }
            }
        }

        if self.buffer().name() == "GitStatus" {
            match key.code {
                KeyCode::Char('q') => {
                    self.close_buffer();
                    self.render(stdout)?;
                    return Ok(());
                }
                KeyCode::Char('c') => {
                    let _ = std::process::Command::new("git")
                        .arg("add")
                        .arg("-u")
                        .output();
                    self.close_buffer();
                    self.mode = Mode::Insert;
                    self.editor.clear();
                    let msg = "Please review the staged changes, write a concise commit message, and commit them.";
                    for c in msg.chars() {
                        self.editor.insert_char(c);
                    }
                    self.render(stdout)?;
                    return Ok(());
                }
                KeyCode::Char('z') => {
                    let out = std::process::Command::new("git").arg("stash").output();
                    if let Ok(o) = out {
                        let msg = String::from_utf8_lossy(&o.stdout);
                        let err = String::from_utf8_lossy(&o.stderr);
                        let c_idx = self.console_buffer_idx();
                        if !msg.trim().is_empty() {
                            self.buffers[c_idx].push(BufferLine::new(
                                format!("  📦 {}", msg.trim()),
                                LineStyle::Info,
                            ));
                        } else if !err.trim().is_empty() {
                            self.buffers[c_idx].push(BufferLine::new(
                                format!("  ❌ {}", err.trim()),
                                LineStyle::Error,
                            ));
                        }
                    }
                    self.show_git_status(stdout, None)?;
                    return Ok(());
                }
                KeyCode::Char('s') => {
                    let cursor_line = self.buffer().cursor_line();
                    if let Some(line) = self.buffer().lines().get(cursor_line) {
                        let content = line.content();
                        if content.starts_with("    ") {
                            let file = if content.starts_with("    + ") {
                                content.trim_start_matches("    + ").trim().to_string()
                            } else {
                                content.trim_start_matches("    ").trim().to_string()
                            };

                            let status_out = std::process::Command::new("git")
                                .arg("status")
                                .arg("--porcelain")
                                .output();
                            if let Ok(so) = status_out {
                                let s = String::from_utf8_lossy(&so.stdout);
                                for l in s.lines() {
                                    if l.len() > 3 && l[3..].trim() == file {
                                        let x = l.chars().next().unwrap_or(' ');
                                        if x != ' ' && x != '?' {
                                            let _ = std::process::Command::new("git")
                                                .arg("restore")
                                                .arg("--staged")
                                                .arg(&file)
                                                .output();
                                        } else {
                                            let _ = std::process::Command::new("git")
                                                .arg("add")
                                                .arg(&file)
                                                .output();
                                        }
                                        break;
                                    }
                                }
                            }
                            self.show_git_status(stdout, Some(&file))?;
                            return Ok(());
                        }
                    }
                }
                KeyCode::Char('r') => {
                    let cursor_line = self.buffer().cursor_line();
                    if let Some(line) = self.buffer().lines().get(cursor_line) {
                        let content = line.content();
                        if content.starts_with("    ") && !content.contains("(none)") {
                            let file = if content.starts_with("    + ") {
                                content.trim_start_matches("    + ").trim().to_string()
                            } else {
                                content.trim_start_matches("    ").trim().to_string()
                            };

                            let out = std::process::Command::new("git")
                                .arg("diff")
                                .arg("--numstat")
                                .arg("HEAD")
                                .arg("--")
                                .arg(&file)
                                .output();
                            let mut loc_info = "0 LOC".to_string();
                            if let Ok(o) = out {
                                let stdout = String::from_utf8_lossy(&o.stdout);
                                if let Some(line) = stdout.lines().next() {
                                    let parts: Vec<&str> = line.split_whitespace().collect();
                                    if parts.len() >= 2 {
                                        let added = parts[0];
                                        let deleted = parts[1];
                                        loc_info = format!("+{} -{} LOC", added, deleted);
                                    }
                                }
                            }

                            self.pending_reset_target = Some(file.clone());
                            self.push_line(
                                format!("  Reset {} to HEAD? ({}) [n/Y]", file, loc_info),
                                LineStyle::Info,
                            );
                            self.scroll_to_bottom();
                            self.render(stdout)?;
                            return Ok(());
                        }
                    }
                }
                KeyCode::Enter => {
                    let cursor_line = self.buffer().cursor_line();
                    let mut stash_ref_opt = None;
                    let mut file_to_open = None;
                    if let Some(line) = self.buffer().lines().get(cursor_line) {
                        let content = line.content();
                        if content.contains("stash@{") {
                            if let Some(start) = content.find("stash@{") {
                                if let Some(end) = content[start..].find("}") {
                                    let stash_ref = &content[start..start + end + 1];
                                    stash_ref_opt = Some(stash_ref.to_string());
                                }
                            }
                        } else if content.starts_with("    ") {
                            let file = if content.starts_with("    + ") {
                                content.trim_start_matches("    + ").trim().to_string()
                            } else {
                                content.trim_start_matches("    ").trim().to_string()
                            };
                            file_to_open = Some(file);
                        }
                    }
                    if let Some(stash_ref) = stash_ref_opt {
                        self.stash_pop_target = Some(stash_ref.clone());
                        self.push_line(format!("  Pop {}? [n/Y]", stash_ref), LineStyle::Info);
                        self.scroll_to_bottom();
                        self.render(stdout)?;
                        return Ok(());
                    } else if let Some(file) = file_to_open {
                        self.load_file_to_buffer(&file, stdout)?;
                        return Ok(());
                    }
                }
                _ => {}
            }
        }

        let amount = self.count.unwrap_or(1);
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('d') => self.half_page_down(),
                KeyCode::Char('u') => self.half_page_up(),
                KeyCode::Char('b') => self.half_page_up(),
                KeyCode::Char('f') => self.half_page_down(),
                _ => {}
            }
            self.count = None;
            self.clear_pending();
            self.render(stdout)?;
            return Ok(());
        }

        match key.code {
            KeyCode::Char('w') => {
                if self.buffer().name() == "SedChanges" {
                    self.apply_sed_changes(stdout)?;
                    self.count = None;
                    self.clear_pending();
                    return Ok(());
                }
                self.count = None;
            }
            KeyCode::Char('i') => {
                self.mode = Mode::Insert;
                self.count = None;
            }
            KeyCode::Char('a') => {
                self.mode = Mode::Insert;
                self.editor.move_end();
                self.count = None;
            }
            KeyCode::Char('A') => {
                self.mode = Mode::Insert;
                self.editor.move_end();
                self.count = None;
            }
            KeyCode::Char('I') => {
                self.mode = Mode::Insert;
                self.editor.move_home();
                self.count = None;
            }
            KeyCode::Char('o') => {
                let buffer_name = self.buffer().name().to_string();
                if !buffer_name.is_empty()
                    && buffer_name != "Chat"
                    && buffer_name != "Console"
                    && buffer_name != "rg"
                    && buffer_name != "fd"
                    && buffer_name != "GitStatus"
                {
                    let root = std::path::PathBuf::from(&self.config.tools.project_root);
                    let raw_path = std::path::Path::new(&buffer_name);
                    let resolved = if raw_path.is_absolute() {
                        raw_path.to_path_buf()
                    } else {
                        root.join(&buffer_name)
                    };
                    let line_num = self.buffer().cursor_line() + 1;
                    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());

                    let _ = terminal::disable_raw_mode();
                    let _ = execute!(stdout, terminal::LeaveAlternateScreen, cursor::Show);
                    let _ = stdout.flush();

                    let mut cmd = std::process::Command::new(&editor);
                    if editor == "code" || editor == "cursor" {
                        cmd.arg("-g")
                            .arg(format!("{}:{}", resolved.display(), line_num));
                    } else {
                        cmd.arg(format!("+{}", line_num)).arg(&resolved);
                    }

                    match cmd.status() {
                        Ok(_) => {
                            if let Ok(content) = std::fs::read_to_string(&resolved) {
                                self.buffer_mut().clear();
                                self.buffer_mut().push_str(&content, LineStyle::Plain);
                                if line_num - 1 < self.buffer().len() {
                                    self.buffer_mut().set_cursor(line_num - 1, 0);
                                }
                            }
                        }
                        Err(e) => {
                            self.push_info(
                                format!("  ❌ Failed to open editor: {}", e),
                                LineStyle::Error,
                            );
                        }
                    }

                    let _ = execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide);
                    let _ = terminal::enable_raw_mode();
                    self.ensure_cursor_visible();
                }
                self.count = None;
            }
            KeyCode::Tab => {
                let buf_name = self.buffer().name();
                if buf_name == "GitCommit" {
                    if let Some(idx) = self.buffers.iter().position(|b| b.name() == "GitLog") {
                        self.active_buffer = idx;
                        self.ensure_cursor_visible();
                    }
                } else if buf_name == "GitLog" {
                    if let Some(idx) = self.buffers.iter().position(|b| b.name() == "GitCommit") {
                        self.active_buffer = idx;
                        self.ensure_cursor_visible();
                    }
                }
                self.count = None;
            }
            KeyCode::Char('>') => {
                self.mode = Mode::Insert;
                self.editor.clear();
                self.count = None;
            }
            KeyCode::Char(':') => {
                self.mode = Mode::Command;
                self.cmd_editor.clear();
                self.count = None;
                self.pending = None;
                self.render_spinner_only(stdout)?;
                return Ok(());
            }
            KeyCode::Left => {
                self.buffer_mut().move_left();
                self.ensure_cursor_visible();
                self.count = None;
            }
            KeyCode::Right => {
                self.buffer_mut().move_right();
                self.ensure_cursor_visible();
                self.count = None;
            }
            KeyCode::Home => {
                let line_idx = self.buffer().cursor_line();
                if let Some(line) = self.buffer().lines().get(line_idx) {
                    let len = line.content().graphemes(true).count();
                    let col = if len > 0 { len - 1 } else { 0 };
                    self.set_cursor(line_idx, col);
                    self.ensure_cursor_visible();
                }
                self.count = None;
            }
            KeyCode::End => {
                let line_idx = self.buffer().cursor_line();
                if let Some(line) = self.buffer().lines().get(line_idx) {
                    let len = line.content().graphemes(true).count();
                    let col = if len > 0 { len - 1 } else { 0 };
                    self.set_cursor(line_idx, col);
                    self.ensure_cursor_visible();
                }
                self.count = None;
            }
            KeyCode::Char('/') => {
                self.mode = Mode::Search;
                self.cmd_editor.clear();
                self.count = None;
                self.clear_pending();
                self.render_spinner_only(stdout)?;
                return Ok(());
            }
            KeyCode::Char('?') => {
                self.fkey_help = !self.fkey_help;
                self.count = None;
                self.clear_pending();
                self.render(stdout)?;
                return Ok(());
            }
            KeyCode::Char('n') => {
                if let Some(idx) = self.search_match_idx {
                    if !self.search_matches.is_empty() {
                        let next_idx = (idx + 1) % self.search_matches.len();
                        self.search_match_idx = Some(next_idx);
                        let target_line = self.search_matches[next_idx];
                        self.buffer_mut().set_cursor(target_line, 0);
                        self.ensure_cursor_visible();
                    }
                }
                self.count = None;
            }
            KeyCode::Char('N') => {
                if let Some(idx) = self.search_match_idx {
                    if !self.search_matches.is_empty() {
                        let prev_idx = if idx == 0 {
                            self.search_matches.len() - 1
                        } else {
                            idx - 1
                        };
                        self.search_match_idx = Some(prev_idx);
                        let target_line = self.search_matches[prev_idx];
                        self.buffer_mut().set_cursor(target_line, 0);
                        self.ensure_cursor_visible();
                    }
                }
                self.count = None;
            }
            KeyCode::PageDown => self.half_page_down(),
            KeyCode::PageUp => self.half_page_up(),
            KeyCode::Char(']') => {
                self.pending = Some(']');
                self.show_which_key_popup(']');
                self.render(stdout)?;
                return Ok(());
            }
            KeyCode::Char('[') => {
                self.pending = Some('[');
                self.show_which_key_popup('[');
                self.render(stdout)?;
                return Ok(());
            }
            KeyCode::Char('.') => {
                if let Some(action) = self.last_action {
                    match action {
                        RepeatAction::NextHunk => self.jump_hunk(1),
                        RepeatAction::PrevHunk => self.jump_hunk(-1),
                        RepeatAction::NextFunc => self.jump_func(1),
                        RepeatAction::PrevFunc => self.jump_func(-1),
                    }
                }
                self.count = None;
            }
            KeyCode::Char('h') => {
                if self.pending == Some(']') {
                    self.jump_hunk(1);
                    self.last_action = Some(RepeatAction::NextHunk);
                    self.clear_pending();
                    self.count = None;
                } else if self.pending == Some('[') {
                    self.jump_hunk(-1);
                    self.last_action = Some(RepeatAction::PrevHunk);
                    self.clear_pending();
                    self.count = None;
                } else {
                    self.buffer_mut().move_left();
                    self.ensure_cursor_visible();
                    self.count = None;
                }
            }
            KeyCode::Char('f') => {
                if self.pending == Some(']') {
                    self.jump_func(1);
                    self.last_action = Some(RepeatAction::NextFunc);
                    self.clear_pending();
                    self.count = None;
                } else if self.pending == Some('[') {
                    self.jump_func(-1);
                    self.last_action = Some(RepeatAction::PrevFunc);
                    self.clear_pending();
                    self.count = None;
                }
            }
            KeyCode::Char('l') => {
                if self.pending == Some('P') {
                    if !self.waiting {
                        let content: String = self
                            .buffer()
                            .lines()
                            .iter()
                            .map(|l| l.content())
                            .collect::<Vec<String>>()
                            .join("\n");
                        let hunks = crate::patch::parse_patches(&content);
                        let valid_hunks: Vec<_> = hunks.into_iter().filter(|h| !h.filename.trim().is_empty()).collect();
                        if !valid_hunks.is_empty() {
                            self.merge_buffer_apply = true;
                            self.start_merge(valid_hunks);
                        } else {
                            self.push_info("  ❌ No valid patches with filenames found in current buffer.", LineStyle::Error);
                            self.scroll_to_bottom();
                        }
                    }
                    self.clear_pending();
                    self.count = None;
                    self.render(stdout)?;
                    return Ok(());
                }

                if let Some(lines) = self.get_git_gutter_lines() {
                    if !lines.is_empty() {
                        let current_line = self.buffer().cursor_line() + 1;
                        let next = lines.iter().find(|&&l| l > current_line).or(lines.first());
                        if let Some(&target) = next {
                            self.buffer_mut().set_cursor(target - 1, 0);
                            self.center_cursor();
                        }
                    }
                }
                self.count = None;
            }
            KeyCode::Char('L') => {
                if let Some(lines) = self.get_git_gutter_lines() {
                    if !lines.is_empty() {
                        let current_line = self.buffer().cursor_line() + 1;
                        let prev = lines
                            .iter()
                            .rev()
                            .find(|&&l| l < current_line)
                            .or(lines.last());
                        if let Some(&target) = prev {
                            self.buffer_mut().set_cursor(target - 1, 0);
                            self.center_cursor();
                        }
                    }
                }
                self.count = None;
            }
            KeyCode::Char('z') => {
                if self.pending == Some('z') {
                    self.center_cursor();
                    self.clear_pending();
                    self.count = None;
                } else {
                    self.show_which_key_popup('z');
                    self.render(stdout)?;
                    return Ok(());
                }
            }

            KeyCode::Char('0') => {
                let line_idx = self.buffer().cursor_line();
                self.set_cursor(line_idx, 0);
                self.ensure_cursor_visible();
                self.count = None;
            }
            KeyCode::Char('$') => {
                let line_idx = self.buffer().cursor_line();
                if let Some(line) = self.buffer().lines().get(line_idx) {
                    let len = line.content().graphemes(true).count();
                    let col = if len > 0 { len - 1 } else { 0 };
                    self.set_cursor(line_idx, col);
                    self.ensure_cursor_visible();
                }
                self.count = None;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.buffer_mut().move_down(amount);
                self.ensure_cursor_visible();
                self.count = None;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.buffer_mut().move_up(amount);
                self.ensure_cursor_visible();
                self.count = None;
            }
            KeyCode::Char('K') => {
                if let Some(word) = self.get_word_under_cursor() {
                    let cmd = format!("rg {}", word);
                    let result = self.execute_command(&cmd, stdout)?;
                    match result {
                        CommandResult::Quit => {
                            self.editor.save_history(&self.config.repl.history_file);
                            self.cmd_editor
                                .save_history(&self.config.repl.command_history_file);
                            if let Some(handle) = self.agent_handle.take() {
                                handle.abort();
                            }
                            return Err(anyhow::anyhow!("__QUIT__"));
                        }
                        CommandResult::ClearScreen => {
                            self.buffer_mut().clear();
                            self.push_line("Screen cleared.", LineStyle::Dim);
                        }
                        CommandResult::Continue => {}
                    }
                }
                self.count = None;
            }
            KeyCode::Char('G') => {
                if self.count.is_some() {
                    let target = amount.saturating_sub(1);
                    if target < self.buffer().len() {
                        self.buffer_mut().move_top();
                        self.buffer_mut().move_down(target);
                    }
                } else {
                    self.move_bottom();
                }
                self.count = None;
            }
            KeyCode::Char('g') => {
                if self.pending == Some('g') {
                    self.buffer_mut().move_top();
                    self.clear_pending();
                    self.count = None;
                } else if self.pending == Some(',') {
                    self.pending = Some('G');
                    self.show_which_key_popup('G');
                    self.render(stdout)?;
                    return Ok(());
                } else {
                    self.pending = Some('g');
                    self.show_which_key_popup('g');
                    self.render(stdout)?;
                    return Ok(());
                }
            }
            KeyCode::Char(',') => {
                self.pending = Some(',');
                self.show_which_key_popup(',');
                self.render(stdout)?;
                return Ok(());
            }
            KeyCode::Char('r') => {
                if self.pending == Some('G') {
                    self.revert_current_hunk()?;
                    self.clear_pending();
                    self.count = None;
                } else {
                    self.clear_pending();
                    self.count = None;
                }
            }
            KeyCode::Char('p') => {
                if self.pending == Some(',') {
                    self.pending = Some('P');
                    self.show_which_key_popup('P');
                    self.render(stdout)?;
                    return Ok(());
                } else if self.pending == Some('P') {
                    if !self.waiting {
                        match arboard::Clipboard::new().and_then(|mut cb| cb.get_text()) {
                            Ok(content) => {
                                if !content.trim().is_empty() {
                                    let hunks = crate::patch::parse_patches(&content);
                                    let valid_hunks: Vec<_> = hunks.into_iter().filter(|h| !h.filename.trim().is_empty()).collect();
                                    if !valid_hunks.is_empty() {
                                        self.merge_buffer_apply = true;
                                        self.start_merge(valid_hunks);
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
                    }
                    self.clear_pending();
                    self.count = None;
                    self.render(stdout)?;
                    return Ok(());
                } else {
                    let paste_amount = self.count.unwrap_or(1);
                    if !self.yank_register.is_empty() {
                        let cursor_line = self.buffer().cursor_line();
                        let lines_to_paste = self.yank_register.clone();
                        let paste_len = lines_to_paste.len();
                        let mut insert_idx = cursor_line + 1;
                        for _ in 0..paste_amount {
                            self.buffer_mut().insert_lines(insert_idx, &lines_to_paste);
                            insert_idx += paste_len;
                        }
                        self.buffer_mut().set_cursor(cursor_line + 1, 0);
                        self.ensure_cursor_visible();
                        let buf_name = self.buffer().name().to_string();
                        if !buf_name.is_empty() && buf_name != "Chat" && buf_name != "Console" {
                            self.modified_buffers.insert(buf_name);
                        }
                    }
                    self.clear_pending();
                    self.count = None;
                }
            }
            KeyCode::Char('y') => {
                if self.pending == Some('y') {
                    let cursor_line = self.buffer().cursor_line();
                    let cursor_col = self.buffer().cursor_col();
                    let amount = self.count.unwrap_or(1);
                    let lines_len = self.buffer().len();
                    let end_line = (cursor_line + amount).min(lines_len);
                    let mut yanked_text = String::new();
                    self.yank_register.clear();
                    for i in cursor_line..end_line {
                        if let Some(line) = self.buffer().lines().get(i) {
                            yanked_text.push_str(&line.content());
                            yanked_text.push('\n');
                            self.yank_register.push(line.clone());
                        }
                    }
                    match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(yanked_text)) {
                        Ok(_) => {
                            self.push_info(
                                format!(
                                    "  📋 Yanked {} line(s) to clipboard",
                                    end_line - cursor_line
                                ),
                                LineStyle::Dim,
                            );
                        }
                        Err(e) => {
                            self.push_info(
                                format!("  ❌ Clipboard error: {}", e),
                                LineStyle::Error,
                            );
                        }
                    }
                    self.set_cursor(cursor_line, cursor_col);
                    self.clear_pending();
                    self.count = None;
                } else {
                    self.pending = Some('y');
                    self.show_which_key_popup('y');
                    self.render(stdout)?;
                    return Ok(());
                }
            }

            KeyCode::Char('d') => {
                if self.pending == Some('d') {
                    let amount = self.count.unwrap_or(1);
                    self.do_dd(amount)?;
                    self.clear_pending();
                    self.count = None;
                } else {
                    self.pending = Some('d');
                    self.show_which_key_popup('d');
                    self.render(stdout)?;
                    return Ok(());
                }
            }

            KeyCode::Char('u') => {
                if self.buffer_mut().undo() {
                    self.push_info("  ↩️  Undone line deletion", LineStyle::Dim);
                } else {
                    self.push_info("  Nothing to undo", LineStyle::Dim);
                }
                self.ensure_cursor_visible();
                self.count = None;
            }
            KeyCode::PageDown => self.half_page_down(),
            KeyCode::PageUp => self.half_page_up(),
            KeyCode::Home => self.buffer_mut().move_top(),
            KeyCode::End => self.move_bottom(),
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.mode = Mode::Insert;
                self.editor.clear();
                self.count = None;
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let digit = c.to_digit(10).unwrap() as usize;
                if self.count.is_some() || c != '0' {
                    self.count = Some(self.count.unwrap_or(0) * 10 + digit);
                    self.clear_pending();
                    self.render(stdout)?;
                    return Ok(());
                }
            }
            KeyCode::Char('v') => {
                self.mode = Mode::Visual;
                self.last_visual_mode = Some(Mode::Visual);
                self.selection_start =
                    Some((self.buffer().cursor_line(), self.buffer().cursor_col()));
                self.count = None;
                self.clear_pending();
                self.render(stdout)?;
                return Ok(());
            }
            KeyCode::Char('V') => {
                self.mode = Mode::VisualLine;
                self.last_visual_mode = Some(Mode::VisualLine);
                self.selection_start =
                    Some((self.buffer().cursor_line(), self.buffer().cursor_col()));
                self.count = None;
                self.clear_pending();
                self.render(stdout)?;
                return Ok(());
            }
            _ => {
                self.count = None;
            }
        }
        self.clear_pending();
        self.render(stdout)
    }

    fn get_word_under_cursor(&self) -> Option<String> {
        let line_idx = self.buffer().cursor_line();
        let col_idx = self.buffer().cursor_col();
        let line = self.buffer().lines().get(line_idx)?;
        let content = line.content();
        let graphemes: Vec<&str> = content.graphemes(true).collect();
        if graphemes.is_empty() {
            return None;
        }
        let col = col_idx.min(graphemes.len().saturating_sub(1));
        let first_char = graphemes[col].chars().next().unwrap_or(' ');
        if !first_char.is_alphanumeric() && first_char != '_' {
            return None;
        }

        let mut start = col;
        while start > 0 {
            let c = graphemes[start - 1].chars().next().unwrap_or(' ');
            if c.is_alphanumeric() || c == '_' {
                start -= 1;
            } else {
                break;
            }
        }

        let mut end = col;
        while end < graphemes.len() {
            let c = graphemes[end].chars().next().unwrap_or(' ');
            if c.is_alphanumeric() || c == '_' {
                end += 1;
            } else {
                break;
            }
        }

        let word: String = graphemes[start..end].join("");
        if word.is_empty() {
            None
        } else {
            Some(word)
        }
    }

    pub(super) fn jump_hunk(&mut self, dir: i32) {
        if let Some(hunks) = self.get_hunk_starts() {
            let current_line = self.buffer().cursor_line() + 1;
            let target = if dir > 0 {
                hunks.iter().find(|&&l| l > current_line).copied()
            } else {
                hunks.iter().rev().find(|&&l| l < current_line).copied()
            };
            if let Some(t) = target {
                self.buffer_mut().set_cursor(t - 1, 0);
                self.center_cursor();
            }
        }
    }

    pub(super) fn jump_func(&mut self, dir: i32) {
        let functions = self.get_function_starts();
        let current_line = self.buffer().cursor_line();
        let target = if dir > 0 {
            functions.iter().find(|&&l| l > current_line).copied()
        } else {
            functions.iter().rev().find(|&&l| l < current_line).copied()
        };
        if let Some(t) = target {
            self.buffer_mut().set_cursor(t, 0);
            self.center_cursor();
        }
    }

    fn get_function_starts(&self) -> Vec<usize> {
        let lines = self.buffer().lines();
        let content: String = lines
            .iter()
            .map(|l| l.content())
            .collect::<Vec<_>>()
            .join("\n");
        let mut starts = Vec::new();
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
                    starts.push(node.start_position().row);
                }
            }
        }
        if starts.is_empty() {
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
                    || trimmed.starts_with("def ")
                    || trimmed.starts_with("function ")
                    || trimmed.starts_with("export function ")
                    || trimmed.starts_with("export async function ")
                    || trimmed.starts_with("interface ")
                {
                    starts.push(i);
                }
            }
        }
        starts
    }

    pub(super) fn do_dd(&mut self, amount: usize) -> anyhow::Result<()> {
        let buffer_name = self.buffer().name().to_string();
        if buffer_name == "SedChanges" {
            let cursor_line = self.buffer().cursor_line();
            let mut block_start = cursor_line;
            while block_start > 0 {
                let content = self
                    .buffer()
                    .lines()
                    .get(block_start)
                    .map(|l| l.content())
                    .unwrap_or_default();
                if content.starts_with("@@ ") {
                    break;
                }
                block_start -= 1;
            }
            let content = self
                .buffer()
                .lines()
                .get(block_start)
                .map(|l| l.content())
                .unwrap_or_default();
            if content.starts_with("@@ ") && cursor_line <= block_start + 3 {
                let lines_len = self.buffer().len();
                let end_line = (block_start + 4).min(lines_len);
                self.buffer_mut().remove_lines(block_start, end_line);

                let new_len = self.buffer().len();
                let new_cursor = if new_len == 0 {
                    0
                } else {
                    block_start.min(new_len - 1)
                };
                self.set_cursor(new_cursor, 0);
                self.ensure_cursor_visible();
                self.push_info("  🗑️  Discarded change", LineStyle::Dim);
            }
        } else {
            let cursor_line = self.buffer().cursor_line();
            let lines_len = self.buffer().len();
            if lines_len > 0 {
                let end_line = (cursor_line + amount).min(lines_len);
                let mut yanked_text = String::new();
                self.yank_register.clear();
                for i in cursor_line..end_line {
                    if let Some(line) = self.buffer().lines().get(i) {
                        yanked_text.push_str(&line.content());
                        yanked_text.push('\n');
                        self.yank_register.push(line.clone());
                    }
                }
                match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(yanked_text)) {
                    Ok(_) => {}
                    Err(e) => {
                        self.push_info(format!("  ❌ Clipboard error: {}", e), LineStyle::Error)
                    }
                }
                self.buffer_mut().remove_lines(cursor_line, end_line);
                let new_len = self.buffer().len();
                let new_cursor = if new_len == 0 {
                    0
                } else {
                    cursor_line.min(new_len - 1)
                };
                self.set_cursor(new_cursor, 0);
                self.ensure_cursor_visible();
                self.push_info(
                    format!("  🗑️  Deleted {} line(s)", end_line - cursor_line),
                    LineStyle::Dim,
                );
                let buf_name = self.buffer().name().to_string();
                if !buf_name.is_empty() && buf_name != "Chat" && buf_name != "Console" {
                    self.modified_buffers.insert(buf_name);
                }
            }
        }
        Ok(())
    }
    fn clear_pending(&mut self) {
        if self.popup_mode == PopupMode::WhichKey && self.popup.active {
            self.popup.hide();
        }
        self.pending = None;
    }
}