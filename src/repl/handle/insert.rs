// src/repl/handle/insert.rs
//! Insert-mode key handling.

use super::super::*;
use crate::repl::buffer::LineStyle;
use crate::repl::misc::COMMAND_LIST;
use crate::repl::Mode;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::io;

#[cfg(target_os = "macos")]
fn read_clipboard() -> Result<String, String> {
    use std::process::Command;
    let output = Command::new("pbpaste")
        .output()
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(target_os = "linux")]
fn read_clipboard() -> Result<String, String> {
    use std::process::Command;
    let output = Command::new("xclip")
        .args(["-selection", "clipboard", "-o"])
        .output()
        .or_else(|_| {
            Command::new("xsel")
                .args(["--clipboard", "--output"])
                .output()
        })
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn read_clipboard() -> Result<String, String> {
    Err("Clipboard reading is only supported on macOS and Linux".to_string())
}

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

        if key.modifiers.contains(KeyModifiers::ALT) {
            if let KeyCode::Char('v')
            | KeyCode::Char('V')
            | KeyCode::Char('p')
            | KeyCode::Char('P') = key.code
            {
                let content = read_clipboard().unwrap_or_default();
                let hunks = crate::patch::parse_patches(&content);
                if !hunks.is_empty() {
                    self.start_merge(hunks);
                    self.render(stdout)?;
                    return Ok(());
                } else {
                    self.push_info("  No patches found in clipboard.", LineStyle::Error);
                }
                self.render_spinner_only(stdout)?;
                return Ok(());
            }
        }

        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
            }
            KeyCode::Insert => {
                let content = read_clipboard().unwrap_or_default();
                let hunks = crate::patch::parse_patches(&content);
                if !hunks.is_empty() {
                    self.start_merge(hunks);
                    self.render(stdout)?;
                    return Ok(());
                } else {
                    self.push_info("  No patches found in clipboard.", LineStyle::Error);
                }
            }
            KeyCode::Tab => {
                let content = self.editor.content().to_string();
                if content.starts_with('/') && !content.contains(' ') {
                    let current_skill = content.trim_start_matches('/');
                    if let Some(config_dir) = dirs::config_dir() {
                        let skills_dir = config_dir.join("pcode").join("skills");
                        let mut candidates: Vec<String> = Vec::new();
                        if let Ok(entries) = std::fs::read_dir(&skills_dir) {
                            for entry in entries.flatten() {
                                if entry.path().is_dir() {
                                    if let Some(name) = entry.file_name().to_str() {
                                        if name.starts_with(current_skill) {
                                            candidates.push(format!("/{}", name));
                                        }
                                    }
                                }
                            }
                        }
                        if !candidates.is_empty() {
                            let candidates_ref: Vec<&str> =
                                candidates.iter().map(|s| s.as_str()).collect();
                            self.editor.tab_complete(&candidates_ref);
                            let new_content = self.editor.content().to_string();
                            if candidates.len() == 1 && !new_content.ends_with(' ') {
                                self.editor.insert_char(' ');
                            }
                        }
                    }
                }
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
