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

/// Scan `~/.config/pcode/skills/` for subdirs containing `SKILL.md`
/// whose name starts with `prefix` (case-sensitive prefix match).
fn collect_skill_candidates(prefix: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(config_dir) = dirs::config_dir() {
        let skills_dir = config_dir.join("pcode").join("skills");
        if let Ok(entries) = std::fs::read_dir(&skills_dir) {
            let mut names: Vec<String> = entries
                .flatten()
                .filter_map(|e| {
                    let p = e.path();
                    if !p.is_dir() {
                        return None;
                    }
                    let name = e.file_name().to_string_lossy().to_string();
                    if !name.starts_with(prefix) {
                        return None;
                    }
                    if !p.join("SKILL.md").exists() {
                        return None;
                    }
                    Some(name)
                })
                .collect();
            names.sort();
            for n in names {
                out.push(format!("/{}", n));
            }
        }
    }
    out
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
                self.skill_completion_active = false;
                self.skill_completion_candidates.clear();
                self.mode = Mode::Normal;
            }
            KeyCode::Tab => {
                let content = self.editor.content().to_string();
                if content.starts_with('/') && !content.contains(' ') {
                    let current_skill = content.trim_start_matches('/').to_string();

                    if !self.skill_completion_active
                        || self.skill_completion_prefix != current_skill
                    {
                        let cands = collect_skill_candidates(&current_skill);
                        self.skill_completion_candidates = cands;
                        self.skill_completion_prefix = current_skill;
                        self.skill_completion_idx = 0;
                        self.skill_completion_active = !self.skill_completion_candidates.is_empty();
                    } else if !self.skill_completion_candidates.is_empty() {
                        self.skill_completion_idx =
                            (self.skill_completion_idx + 1) % self.skill_completion_candidates.len();
                    }

                    if self.skill_completion_active {
                        let cand = self.skill_completion_candidates[self.skill_completion_idx].clone();
                        self.editor.kill_to_start();
                        for ch in cand.chars() {
                            self.editor.insert_char(ch);
                        }
                    } else {
                        self.push_info("  ⚠️ No matching skill found.", LineStyle::Dim);
                    }
                }
            }
            KeyCode::Right => {
                if self.skill_completion_active
                    && !self.skill_completion_candidates.is_empty()
                {
                    let cur = self.editor.content().to_string();
                    if !cur.ends_with(' ') && !cur.contains(' ') {
                        self.editor.insert_char(' ');
                    }
                    self.show_skill_complete_popup();
                    self.render(stdout)?;
                    return Ok(());
                } else {
                    self.editor.move_right();
                }
            }
            KeyCode::Enter => {
                let was_active = self.skill_completion_active;
                self.skill_completion_active = false;
                self.skill_completion_candidates.clear();

                if key.modifiers.contains(KeyModifiers::SHIFT)
                    || key.modifiers.contains(KeyModifiers::ALT)
                {
                    self.editor.insert_char('\n');
                } else {
                    let input = self.editor.submit();
                    self.submit_input(stdout, input)?;
                    return Ok(());
                }
                let _ = was_active;
            }
            KeyCode::Char(c) => {
                self.skill_completion_active = false;
                self.skill_completion_candidates.clear();
                self.editor.insert_char(c);
            }
            KeyCode::Backspace => {
                self.skill_completion_active = false;
                self.skill_completion_candidates.clear();
                self.editor.backspace();
            }
            KeyCode::Delete => {
                self.skill_completion_active = false;
                self.editor.delete();
            }
            KeyCode::Left => {
                self.editor.move_left();
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
