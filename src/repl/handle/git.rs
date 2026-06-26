// src/repl/handle/git.rs
//! Git gutter, git-status buffer, hunk popup.

use super::super::*;
use crate::repl::buffer::{BufferLine, LineStyle, ResponseBuffer};
use crate::repl::helper::{PopupItem, PopupPosition};
use crate::repl::Mode;
use std::io;

impl Repl {
    fn get_git_root(&self) -> Option<std::path::PathBuf> {
        let repo_path = std::path::Path::new(&self.config.tools.project_root);
        let output = std::process::Command::new("git")
            .arg("rev-parse")
            .arg("--show-toplevel")
            .current_dir(repo_path)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if path_str.is_empty() {
            return None;
        }
        Some(std::path::PathBuf::from(path_str))
    }

    pub(crate) fn get_git_gutter(&self) -> Option<Vec<char>> {
        let buffer_name = self.buffer().name();
        if buffer_name == "Chat"
            || buffer_name == "Console"
            || buffer_name == "rg"
            || buffer_name == "fd"
            || buffer_name == "GitStatus"
            || buffer_name.is_empty()
        {
            return None;
        }
        let abs_git_root = self.get_git_root()?;
        let repo = match git2::Repository::open(&abs_git_root) {
            Ok(r) => r,
            Err(_) => return None,
        };
        let project_root = std::path::PathBuf::from(&self.config.tools.project_root);
        let raw_path = std::path::Path::new(buffer_name);
        let abs_file_path = if raw_path.is_absolute() {
            raw_path.to_path_buf()
        } else {
            project_root.join(buffer_name)
        };
        let abs_file_path = match abs_file_path.canonicalize() {
            Ok(p) => p,
            Err(_) => return None,
        };
        let rel_path = match abs_file_path.strip_prefix(&abs_git_root) {
            Ok(p) => p,
            Err(_) => return None,
        };

        let mut content: String = self
            .buffer()
            .lines()
            .iter()
            .map(|l| l.content().clone())
            .collect::<Vec<_>>()
            .join("\n");
        let line_count = self.buffer().lines().len();

        let head_tree = match repo.head() {
            Ok(h) => match h.peel_to_tree() {
                Ok(t) => t,
                Err(_) => return Some(vec!['+'; line_count]),
            },
            Err(_) => return Some(vec!['+'; line_count]),
        };

        let entry = match head_tree.get_path(rel_path) {
            Ok(e) => e,
            Err(_) => {
                return Some(vec!['+'; line_count]);
            }
        };

        let old_blob = match entry.to_object(&repo).and_then(|o| o.peel_to_blob()) {
            Ok(b) => b,
            Err(_) => return None,
        };

        let file_path = std::path::Path::new(buffer_name);

        if !content.is_empty() {
            if let Ok(bytes) = std::fs::read(&abs_file_path) {
                if bytes.last() == Some(&b'\n') && !content.ends_with('\n') {
                    content.push('\n');
                }
            }
        }

        let patch = match git2::Patch::from_blob_and_buffer(
            &old_blob,
            Some(file_path),
            content.as_bytes(),
            Some(file_path),
            None,
        ) {
            Ok(p) => p,
            Err(_) => return None,
        };

        let mut gutter = vec![' '; line_count];
        for h in 0..patch.num_hunks() {
            let mut i = 0;

            let mut lines = Vec::new();
            while let Ok(line) = patch.line_in_hunk(h, i) {
                lines.push(line);
                i += 1;
            }

            let has_addition = lines.iter().any(|l| l.origin() == '+');
            if has_addition {
                for line in &lines {
                    if line.origin() == '+' {
                        if let Some(nl) = line.new_lineno() {
                            let idx = (nl - 1) as usize;
                            if idx < gutter.len() {
                                gutter[idx] = '+';
                            }
                        }
                    }
                }
            } else {
                let mut found_deletion = false;
                let mut marked = false;
                for line in &lines {
                    if line.origin() == '-' {
                        found_deletion = true;
                    } else if line.origin() == ' ' && found_deletion {
                        if let Some(nl) = line.new_lineno() {
                            let idx = (nl - 1) as usize;
                            if idx < gutter.len() {
                                gutter[idx] = '+';
                                marked = true;
                            }
                        }
                        break;
                    }
                }
                if found_deletion && !marked && !gutter.is_empty() {
                    let last_idx = gutter.len() - 1;
                    gutter[last_idx] = '+';
                }
            }
        }
        Some(gutter)
    }

    pub(super) fn get_git_gutter_lines(&self) -> Option<Vec<usize>> {
        let gutter = self.get_git_gutter()?;
        let lines: Vec<usize> = gutter
            .iter()
            .enumerate()
            .filter_map(|(i, &c)| if c == '+' { Some(i + 1) } else { None })
            .collect();
        Some(lines)
    }

    pub(super) fn show_git_status(
        &mut self,
        stdout: &mut io::Stdout,
        target_file: Option<&str>,
    ) -> anyhow::Result<()> {
        let output = std::process::Command::new("git")
            .arg("status")
            .arg("--porcelain=v1")
            .output();

        let new_buf_idx = if self.buffer().name() == "GitStatus" {
            self.active_buffer
        } else {
            let idx = self.buffers.len();
            self.buffers.push(ResponseBuffer::with_name("GitStatus"));
            idx
        };
        self.active_buffer = new_buf_idx;
        self.buffers[new_buf_idx].clear();
        let c_idx = self.console_buffer_idx();
        self.buffers[c_idx].push(BufferLine::new("  📊 GitStatus", LineStyle::Info));

        let mut staged = Vec::new();
        let mut unstaged = Vec::new();
        let mut untracked = Vec::new();

        if let Ok(out) = output {
            let s = String::from_utf8_lossy(&out.stdout);
            for line in s.lines() {
                if line.len() < 3 {
                    continue;
                }
                let status = &line[..2];
                let file = line[3..].trim().to_string();
                if status == "??" {
                    untracked.push(file);
                } else {
                    let x = status.chars().next().unwrap_or(' ');
                    let y = status.chars().nth(1).unwrap_or(' ');
                    if x != ' ' && x != '?' {
                        staged.push(file.clone());
                    }
                    if y != ' ' && y != '?' {
                        unstaged.push(file);
                    }
                }
            }
        }

        self.push_line(
            format!("  Stage Changes ({})", staged.len()),
            LineStyle::Info,
        );
        self.push_line("  ────────────────────────────────────────", LineStyle::Dim);
        if staged.is_empty() {
            self.push_line("    (none)", LineStyle::Dim);
        } else {
            for f in &staged {
                let segments = vec![
                    ("    + ".to_string(), LineStyle::ToolResult),
                    (f.clone(), LineStyle::Plain),
                ];
                self.buffer_mut().push(BufferLine::from_segments(segments));
            }
        }
        self.push_line("", LineStyle::Plain);

        self.push_line(
            format!("  Unstage Changes ({})", unstaged.len()),
            LineStyle::Info,
        );
        self.push_line("  ────────────────────────────────────────", LineStyle::Dim);
        if unstaged.is_empty() {
            self.push_line("    (none)", LineStyle::Dim);
        } else {
            for f in &unstaged {
                self.push_line(format!("    {}", f), LineStyle::Error);
            }
        }
        self.push_line("", LineStyle::Plain);

        self.push_line(
            format!("  Untracked Files ({})", untracked.len()),
            LineStyle::Info,
        );
        self.push_line("  ────────────────────────────────────────", LineStyle::Dim);
        if untracked.is_empty() {
            self.push_line("    (none)", LineStyle::Dim);
        } else {
            for f in &untracked {
                self.push_line(format!("    {}", f), LineStyle::Dim);
            }
        }
        self.push_line("", LineStyle::Plain);

        self.push_line("  ------ Branch ------", LineStyle::Info);
        self.push_line("  ────────────────────────────────────────", LineStyle::Dim);
        let branch = std::process::Command::new("git")
            .arg("branch")
            .arg("--show-current")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let time = std::process::Command::new("git")
            .arg("log")
            .arg("-1")
            .arg("--format=%cr")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if branch.is_empty() {
            self.push_line("    (detached HEAD)", LineStyle::Dim);
        } else {
            self.push_line(format!("    * {} {}", branch, time), LineStyle::ToolResult);
        }
        self.push_line("", LineStyle::Plain);

        self.push_line("  ------ Stash ------", LineStyle::Info);
        self.push_line("  ────────────────────────────────────────", LineStyle::Dim);
        let stash = std::process::Command::new("git")
            .arg("stash")
            .arg("list")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        if stash.lines().next().is_none() {
            self.push_line("    (none)", LineStyle::Dim);
        } else {
            for line in stash.lines() {
                self.push_line(format!("    {}", line), LineStyle::Plain);
            }
        }
        self.push_line("", LineStyle::Plain);

        self.push_line("  ────────────────────────────────────────", LineStyle::Dim);
        self.push_line(
            "  [s] Toggle staged  [r] Reset file  [Enter] Open file  [z] stash [q] Close",
            LineStyle::Dim,
        );

        let target_line = if let Some(file) = target_file {
            self.buffer().lines().iter().position(|l| {
                let c = l.content();
                if !c.starts_with("    ") {
                    return false;
                }
                let cleaned = if c.starts_with("    + ") {
                    c.trim_start_matches("    + ").trim()
                } else {
                    c.trim_start_matches("    ").trim()
                };
                cleaned == file
            })
        } else {
            self.buffer()
                .lines()
                .iter()
                .position(|l| l.content().starts_with("    ") && l.content().trim() != "(none)")
        };
        if let Some(idx) = target_line {
            self.buffer_mut().set_cursor(idx, 0);
        } else {
            self.scroll_to_bottom();
        }
        self.ensure_cursor_visible();
        self.mode = Mode::Normal;
        self.render(stdout)?;
        Ok(())
    }

    pub(super) fn show_git_hunk_popup(&mut self) {
        if let Some(diff_lines) = self.get_current_hunk_diff() {
            if diff_lines.is_empty() {
                self.push_info("  No git hunks found in this buffer.", LineStyle::Dim);
                self.scroll_to_bottom();
                return;
            }
            let current_line_num = self.buffer().cursor_line() + 1;
            let active_idx = diff_lines
                .iter()
                .position(|&(line_num, _)| line_num == current_line_num)
                .unwrap_or(0);
            let items: Vec<PopupItem> = diff_lines
                .iter()
                .map(|&(line_num, ref display)| {
                    let is_active = line_num == current_line_num;
                    PopupItem {
                        text: display.clone(),
                        is_active,
                        id: Some(line_num),
                    }
                })
                .collect();
            self.popup_mode = crate::repl::PopupMode::GitHunks;
            self.popup
                .show("Git Hunk Diff", items, active_idx, PopupPosition::Bottom);
            self.popup.show_filter = false;
        } else {
            self.push_info(
                "  Cursor is not inside a git hunk or not a git file.",
                LineStyle::Dim,
            );
            self.scroll_to_bottom();
        }
    }

    fn get_current_hunk_diff(&self) -> Option<Vec<(usize, String)>> {
        let buffer_name = self.buffer().name();
        if buffer_name == "Chat"
            || buffer_name == "Console"
            || buffer_name == "rg"
            || buffer_name == "fd"
            || buffer_name == "GitStatus"
            || buffer_name.is_empty()
        {
            return None;
        }
        let abs_git_root = self.get_git_root()?;
        let repo = match git2::Repository::open(&abs_git_root) {
            Ok(r) => r,
            Err(_) => return None,
        };
        let project_root = std::path::PathBuf::from(&self.config.tools.project_root);
        let raw_path = std::path::Path::new(buffer_name);
        let abs_file_path = if raw_path.is_absolute() {
            raw_path.to_path_buf()
        } else {
            project_root.join(buffer_name)
        };
        let abs_file_path = match abs_file_path.canonicalize() {
            Ok(p) => p,
            Err(_) => return None,
        };
        let rel_path = match abs_file_path.strip_prefix(&abs_git_root) {
            Ok(p) => p,
            Err(_) => return None,
        };

        let mut content: String = self
            .buffer()
            .lines()
            .iter()
            .map(|l| l.content().clone())
            .collect::<Vec<_>>()
            .join("\n");
        let head_tree = match repo.head() {
            Ok(h) => match h.peel_to_tree() {
                Ok(t) => t,
                Err(_) => return None,
            },
            Err(_) => return None,
        };
        let entry = match head_tree.get_path(rel_path) {
            Ok(e) => e,
            Err(_) => return None,
        };
        let old_blob = match entry.to_object(&repo).and_then(|o| o.peel_to_blob()) {
            Ok(b) => b,
            Err(_) => return None,
        };
        let file_path = std::path::Path::new(buffer_name);
        if !content.is_empty() {
            if let Ok(bytes) = std::fs::read(&abs_file_path) {
                if bytes.last() == Some(&b'\n') && !content.ends_with('\n') {
                    content.push('\n');
                }
            }
        }
        let patch = match git2::Patch::from_blob_and_buffer(
            &old_blob,
            Some(file_path),
            content.as_bytes(),
            Some(file_path),
            None,
        ) {
            Ok(p) => p,
            Err(_) => return None,
        };

        let mut diff_lines = Vec::new();
        let current_line_num = self.buffer().cursor_line() + 1;

        for h in 0..patch.num_hunks() {
            let (hunk, _) = match patch.hunk(h) {
                Ok(hk) => hk,
                Err(_) => continue,
            };
            let new_start = hunk.new_start() as usize;
            let new_lines = hunk.new_lines() as usize;
            let end = new_start + new_lines.saturating_sub(1);

            if current_line_num >= new_start && current_line_num <= end {
                let mut i = 0;
                while let Ok(line) = patch.line_in_hunk(h, i) {
                    let origin = line.origin();
                    let prefix = match origin {
                        '+' => "+ ",
                        '-' => "- ",
                        _ => "  ",
                    };
                    if let Ok(s) = std::str::from_utf8(line.content()) {
                        let line_num = line.new_lineno().map(|n| n as usize).unwrap_or(0);
                        let display = format!("{}{}", prefix, s.trim_end());
                        diff_lines.push((line_num, display));
                    }
                    i += 1;
                }
                return Some(diff_lines);
            }
        }

        None
    }
}
