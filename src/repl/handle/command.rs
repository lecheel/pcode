// src/repl/handle/command.rs
//! Command-mode key handling, `:command` execution, sed apply, help.

use super::super::*;
use crate::agent::SKILL_GROUPS;
use crate::repl::buffer::{BufferLine, LineStyle, ResponseBuffer};
use crate::repl::misc::{highlight_segments, unquote};
use crate::repl::{CommandResult, Mode};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::io;

impl Repl {
    pub(super) fn handle_command_key(
        &mut self,
        key: KeyEvent,
        stdout: &mut io::Stdout,
    ) -> anyhow::Result<()> {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('a') | KeyCode::Char('A') => self.cmd_editor.move_home(),
                KeyCode::Char('e') | KeyCode::Char('E') => self.cmd_editor.move_end(),
                KeyCode::Char('u') | KeyCode::Char('U') => self.cmd_editor.kill_to_start(),
                KeyCode::Char('k') | KeyCode::Char('K') => self.cmd_editor.kill_to_end(),
                KeyCode::Char('w') | KeyCode::Char('W') => self.cmd_editor.kill_word_back(),
                _ => {}
            }
            self.render_spinner_only(stdout)?;
            return Ok(());
        }
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.cmd_editor.clear();
            }
            KeyCode::Tab => {
                self.cmd_editor.tab_complete(COMMAND_LIST);
            }
            KeyCode::Enter => {
                let cmd = self.cmd_editor.submit();
                self.mode = Mode::Normal;
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
                self.render(stdout)?;
                return Ok(());
            }
            KeyCode::Backspace => {
                self.cmd_editor.backspace();
                if self.cmd_editor.is_empty() {
                    self.mode = Mode::Normal;
                }
            }
            KeyCode::Char(c) => {
                self.cmd_editor.insert_char(c);
            }
            KeyCode::Left => self.cmd_editor.move_left(),
            KeyCode::Right => self.cmd_editor.move_right(),
            KeyCode::Home => self.cmd_editor.move_home(),
            KeyCode::End => self.cmd_editor.move_end(),
            KeyCode::Up => self.cmd_editor.history_up(),
            KeyCode::Down => self.cmd_editor.history_down(),
            _ => {}
        }
        if self.popup.active {
            self.render(stdout)
        } else {
            self.render_spinner_only(stdout)
        }
    }

    pub(super) fn execute_command(
        &mut self,
        cmd: &str,
        stdout: &mut io::Stdout,
    ) -> anyhow::Result<CommandResult> {
        let parts: Vec<&str> = cmd.trim().splitn(2, ' ').collect();
        let command = parts[0];
        let arg = parts.get(1).copied().unwrap_or("");
        match command {
            "q" | "quit" | "exit" => {
                self.push_command_info("Bye!", LineStyle::Dim);
                return Ok(CommandResult::Quit);
            }
            "cancel" => {
                if self.waiting {
                    if let Some(tx) = self.cancel_tx.take() {
                        let _ = tx.send(());
                    }
                    self.push_command_info("  ⛔ Cancelling agent task...", LineStyle::Error);
                    self.scroll_to_bottom();
                } else {
                    self.push_command_info("  Nothing to cancel.", LineStyle::Dim);
                }
            }
            "llm" => {
                self.grab_selection_to_chat();
            }
            "h" | "help" | "?" => {
                self.push_help();
                self.scroll_to_bottom();
            }
            "open" | "e" => {
                if self.waiting {
                    self.push_command_info("  ⏳ Agent is busy", LineStyle::Dim);
                } else if arg.is_empty() {
                    self.show_file_picker();
                    return Ok(CommandResult::Continue);
                } else {
                    self.load_file_to_buffer(arg, stdout)?;
                    return Ok(CommandResult::Continue);
                }
            }
            "sed" => {
                let arg = parts.get(1).copied().unwrap_or("");
                if arg.is_empty() {
                    self.push_command_info("  Usage: :sed <find> <replace> [path]", LineStyle::Dim);
                } else {
                    let (find, replace, path) = if arg.starts_with('/') {
                        let parts: Vec<&str> = arg[1..].splitn(2, '/').collect();
                        if parts.len() == 2 {
                            let find = parts[0].to_string();
                            let rest = parts[1].trim_start_matches('/');
                            let parts2: Vec<&str> = rest.splitn(2, ' ').collect();
                            let replace = parts2[0].to_string();
                            let path = if parts2.len() == 2 {
                                parts2[1].trim().to_string()
                            } else {
                                ".".to_string()
                            };
                            (find, replace, path)
                        } else {
                            (String::new(), String::new(), ".".to_string())
                        }
                    } else if arg.contains('|') {
                        let parts: Vec<&str> = arg.splitn(2, '|').collect();
                        let find = parts[0].to_string();
                        if parts.len() == 2 {
                            let parts2: Vec<&str> = parts[1].splitn(2, ' ').collect();
                            let replace = parts2[0].to_string();
                            let path = if parts2.len() == 2 {
                                parts2[1].trim().to_string()
                            } else {
                                ".".to_string()
                            };
                            (find, replace, path)
                        } else {
                            (find, String::new(), ".".to_string())
                        }
                    } else {
                        let parts: Vec<&str> = arg.splitn(3, ' ').collect();
                        if parts.len() >= 2 {
                            (
                                unquote(parts[0]),
                                unquote(parts[1]),
                                if parts.len() == 3 {
                                    unquote(parts[2])
                                } else {
                                    ".".to_string()
                                },
                            )
                        } else {
                            (String::new(), String::new(), ".".to_string())
                        }
                    };

                    if find.is_empty() {
                        self.push_command_info(
                            "  ❌ Find pattern cannot be empty",
                            LineStyle::Error,
                        );
                        self.scroll_to_bottom();
                        return Ok(CommandResult::Continue);
                    }

                    let output = std::process::Command::new("rg")
                        .arg("--color")
                        .arg("never")
                        .arg("-n")
                        .arg("-H")
                        .arg("--no-heading")
                        .arg("-F")
                        .arg(&find)
                        .arg(&path)
                        .output();

                    match output {
                        Ok(out) => {
                            let stdout_str = String::from_utf8_lossy(&out.stdout);
                            let lines: Vec<&str> = stdout_str.lines().collect();

                            let new_buf_idx = if self.buffer().name() == "SedChanges" {
                                self.active_buffer
                            } else {
                                let idx = self.buffers.len();
                                self.buffers.push(ResponseBuffer::with_name("SedChanges"));
                                idx
                            };
                            self.active_buffer = new_buf_idx;
                            self.buffers[new_buf_idx].clear();

                            let c_idx = self.console_buffer_idx();
                            self.buffers[c_idx].push(BufferLine::new(
                                format!("  🔍 sed: {} -> {}", find, replace),
                                LineStyle::Info,
                            ));

                            let mut count = 0;
                            for line in lines.iter() {
                                if line.is_empty() {
                                    continue;
                                }
                                let cols: Vec<&str> = line.splitn(3, ':').collect();
                                if cols.len() == 3 {
                                    let file = cols[0].trim_start_matches("./").to_string();
                                    let line_num = cols[1].parse::<usize>().unwrap_or(0);
                                    let old_line = cols[2].to_string();

                                    let new_line = old_line.replace(&find, &replace);

                                    self.buffers[new_buf_idx].push(BufferLine::from_segments(
                                        vec![
                                            ("@@ ".to_string(), LineStyle::Info),
                                            (
                                                format!("{}:{}", file, line_num),
                                                LineStyle::ToolResult,
                                            ),
                                        ],
                                    ));

                                    let old_segments = highlight_segments(
                                        &old_line,
                                        &find,
                                        LineStyle::Plain,
                                        LineStyle::User,
                                        "- ",
                                        LineStyle::Error,
                                    );
                                    self.buffers[new_buf_idx]
                                        .push(BufferLine::from_segments(old_segments));

                                    let new_segments = highlight_segments(
                                        &new_line,
                                        &replace,
                                        LineStyle::Plain,
                                        LineStyle::ToolResult,
                                        "+ ",
                                        LineStyle::ToolResult,
                                    );
                                    self.buffers[new_buf_idx]
                                        .push(BufferLine::from_segments(new_segments));

                                    self.buffers[new_buf_idx].push_blank();
                                    count += 1;
                                }
                            }

                            if count == 0 {
                                self.buffers[new_buf_idx].push(BufferLine::new(
                                    "  No matches found".to_string(),
                                    LineStyle::Dim,
                                ));
                            } else {
                                self.buffers[new_buf_idx].push(BufferLine::new(format!("  {} proposed changes. Edit buffer, [dd] to delete, [w] to apply", count), LineStyle::Info));
                            }
                            self.scroll_to_bottom();
                        }
                        Err(e) => self
                            .push_command_info(format!("  ❌ sed failed: {}", e), LineStyle::Error),
                    }
                }
            }
            "write" | "w" => {
                if self.waiting {
                    self.push_command_info(
                        "  ⏳ Cannot save while agent is running",
                        LineStyle::Error,
                    );
                } else {
                    let path_str = if arg.is_empty() {
                        let name = self.buffer().name().to_string();
                        let name = if name == "rg" {
                            "rg_results.txt".to_string()
                        } else if name.is_empty() || name == "Chat" {
                            "chat.md".to_string()
                        } else {
                            name
                        };
                        name.replace('*', "")
                    } else {
                        arg.to_string()
                    };

                    if path_str.is_empty() {
                        self.push_command_info(
                            "  ❌ Specify a file path: :write <path>",
                            LineStyle::Error,
                        );
                    } else {
                        let root = std::path::PathBuf::from(&self.config.tools.project_root);
                        let raw_path = std::path::Path::new(&path_str);
                        let resolved = if raw_path.is_absolute() {
                            raw_path.to_path_buf()
                        } else {
                            root.join(&path_str)
                        };
                        let canonical_root = match root.canonicalize() {
                            Ok(p) => p,
                            Err(e) => {
                                self.push_command_info(
                                    format!("  ❌ Invalid project root: {}", e),
                                    LineStyle::Error,
                                );
                                return Ok(CommandResult::Continue);
                            }
                        };
                        let parent = resolved.parent().unwrap_or(&root);
                        if !parent.exists() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let canonical_target = if resolved.exists() {
                            match resolved.canonicalize() {
                                Ok(p) => p,
                                Err(e) => {
                                    self.push_command_info(
                                        format!("  ❌ Failed to resolve {}: {}", path_str, e),
                                        LineStyle::Error,
                                    );
                                    return Ok(CommandResult::Continue);
                                }
                            }
                        } else if let Some(parent_canon) = parent.canonicalize().ok() {
                            parent_canon.join(resolved.file_name().unwrap_or_default())
                        } else {
                            resolved.clone()
                        };
                        let is_allowed = canonical_target.starts_with(&canonical_root)
                            || self.config.tools.allow_paths.iter().any(|p| {
                                std::path::PathBuf::from(p)
                                    .canonicalize()
                                    .map(|c| canonical_target.starts_with(c))
                                    .unwrap_or(false)
                            });
                        if !is_allowed {
                            self.push_command_info(
                                format!(
                                    "  ❌ Access denied: '{}' is outside the project root",
                                    path_str
                                ),
                                LineStyle::Error,
                            );
                        } else {
                            let content: String = self
                                .buffer()
                                .lines()
                                .iter()
                                .map(|l| l.content().clone())
                                .collect::<Vec<String>>()
                                .join("\n");
                            match std::fs::write(&canonical_target, content) {
                                Ok(()) => {
                                    self.push_command_info(
                                        format!("  💾 Wrote buffer to: {}", path_str),
                                        LineStyle::ToolResult,
                                    );
                                    self.buffer_mut().set_name(&path_str);
                                }
                                Err(e) => self.push_command_info(
                                    format!("  ❌ Write failed: {}", e),
                                    LineStyle::Error,
                                ),
                            }
                        }
                    }
                }
            }
            "saveas" => {
                if arg.is_empty() {
                    self.push_command_info("  Usage: :saveas <path>", LineStyle::Error);
                } else {
                    let root = std::path::PathBuf::from(&self.config.tools.project_root);
                    let raw_path = std::path::Path::new(arg);
                    let resolved = if raw_path.is_absolute() {
                        raw_path.to_path_buf()
                    } else {
                        root.join(arg)
                    };
                    if let Some(parent) = resolved.parent() {
                        if let Err(e) = std::fs::create_dir_all(parent) {
                            self.push_command_info(
                                format!("  ❌ Cannot create dir: {}", e),
                                LineStyle::Error,
                            );
                            self.scroll_to_bottom();
                            return Ok(CommandResult::Continue);
                        }
                    }
                    let content = self
                        .buffer()
                        .lines()
                        .iter()
                        .map(|l| l.content().clone())
                        .collect::<Vec<String>>()
                        .join("\n");
                    match std::fs::write(&resolved, format!("{}\n", content)) {
                        Ok(()) => self.push_command_info(
                            format!("  💾 Saved: {}", resolved.display()),
                            LineStyle::ToolResult,
                        ),
                        Err(e) => self.push_command_info(
                            format!("  ❌ Save failed: {}", e),
                            LineStyle::Error,
                        ),
                    }
                }
            }
            "ssave" => {
                if self.waiting {
                    self.push_command_info(
                        "  ⏳ Cannot save while agent is running",
                        LineStyle::Error,
                    );
                } else {
                    let name = if arg.is_empty() { "default" } else { arg };
                    match self
                        .agent_ref()
                        .session
                        .save(&self.config.repl.sessions_dir, name)
                    {
                        Ok(()) => self.push_command_info(
                            format!("  💾 Session saved: {}", name),
                            LineStyle::ToolResult,
                        ),
                        Err(e) => self.push_command_info(
                            format!("  ❌ Save failed: {}", e),
                            LineStyle::Error,
                        ),
                    }
                }
            }
            "sload" => {
                if self.waiting {
                    self.push_command_info(
                        "  ⏳ Cannot load while agent is running",
                        LineStyle::Error,
                    );
                } else {
                    let name = if arg.is_empty() { "default" } else { arg };
                    match crate::session::Session::load(&self.config.repl.sessions_dir, name) {
                        Ok(session) => {
                            self.agent_mut().session = session;
                            self.push_command_info(
                                format!("  📂 Session loaded: {}", name),
                                LineStyle::ToolResult,
                            );
                        }
                        Err(e) => self.push_command_info(
                            format!("  ❌ Load failed: {}", e),
                            LineStyle::Error,
                        ),
                    }
                }
            }
            "sessions" => {
                self.push_command_info("  📂 Saved sessions:", LineStyle::Info);
                match std::fs::read_dir(&self.config.repl.sessions_dir) {
                    Ok(entries) => {
                        for entry in entries.flatten() {
                            if let Some(name) = entry.path().file_stem() {
                                self.push_command_info(
                                    format!("    • {}", name.to_string_lossy()),
                                    LineStyle::Plain,
                                );
                            }
                        }
                    }
                    Err(_) => self.push_command_info("    (none found)", LineStyle::Dim),
                }
            }
            "delete" | "rm" => {
                if arg.is_empty() {
                    self.push_command_info("  Usage: :delete <name>", LineStyle::Error);
                } else {
                    let path = std::path::Path::new(&self.config.repl.sessions_dir)
                        .join(format!("{}.json", arg));
                    match std::fs::remove_file(&path) {
                        Ok(()) => self.push_command_info(
                            format!("  🗑️  Session deleted: {}", arg),
                            LineStyle::ToolResult,
                        ),
                        Err(e) => self.push_command_info(
                            format!("  ❌ Delete failed: {}", e),
                            LineStyle::Error,
                        ),
                    }
                }
            }
            "reset" => {
                if self.waiting {
                    self.push_command_info(
                        "  ⏳ Cannot reset while agent is running",
                        LineStyle::Error,
                    );
                } else {
                    self.agent_mut().session.reset();
                    let idx = self.llm_buffer_idx();
                    self.buffers[idx].clear();
                    self.push_command_info("  🔄 Session reset.", LineStyle::ToolResult);
                }
            }
            "config" => {
                self.push_command_info("  ⚙️  Config:", LineStyle::Info);
                self.push_command_info(
                    format!("    base_url: {}", self.config.server.base_url),
                    LineStyle::Plain,
                );
                self.push_command_info(
                    format!("    model: {}", self.config.server.model),
                    LineStyle::Plain,
                );
                self.push_command_info(
                    format!("    timeout: {}s", self.config.server.timeout),
                    LineStyle::Plain,
                );
                self.push_command_info(
                    format!("    auto_verify: {}", self.config.tools.auto_verify),
                    LineStyle::Plain,
                );
                self.push_command_info(
                    format!("    max_rounds: {}", self.config.repl.max_rounds),
                    LineStyle::Plain,
                );
            }
            "tools" => {
                if self.waiting {
                    self.push_command_info("  ⏳ Agent is busy", LineStyle::Dim);
                } else {
                    let tools = self.agent_ref().active_tools();
                    self.push_command_info(
                        format!("  🔧 Active tools ({}):", tools.len()),
                        LineStyle::Info,
                    );
                    for t in &tools {
                        let name = t["function"]["name"].as_str().unwrap_or("?");
                        let desc: String = t["function"]["description"]
                            .as_str()
                            .unwrap_or("")
                            .chars()
                            .take(60)
                            .collect();
                        self.push_command_info(
                            format!("    • {} — {}", name, desc),
                            LineStyle::Plain,
                        );
                    }
                }
            }
            "debug" => {
                crate::debug::set_debug(!crate::debug::is_debug());
                let state = if crate::debug::is_debug() {
                    "ON"
                } else {
                    "OFF"
                };
                self.push_command_info(format!("  🐛 Debug: {}", state), LineStyle::ToolResult);
            }
            "skills" => {
                if self.waiting {
                    self.push_command_info(
                        "  ⏳ Cannot change skills while agent is running",
                        LineStyle::Error,
                    );
                } else if arg.is_empty() {
                    self.show_skill_group_popup();
                    return Ok(CommandResult::Continue);
                } else if arg == "next" {
                    self.cycle_skill_group(stdout)?;
                } else if arg == "toggle" {
                    self.toggle_tools(stdout)?;
                } else if let Ok(idx) = arg.parse::<usize>() {
                    self.set_skill_group(idx, stdout)?;
                } else {
                    self.set_skill_group_by_name(arg, stdout)?;
                }
            }
            "status" => {
                let skill_idx = self.active_skill_group();
                let group = &SKILL_GROUPS[skill_idx];
                self.push_command_info("  📊 Status:", LineStyle::Info);
                self.push_command_info(
                    format!("    Model: {}", self.config.server.model),
                    LineStyle::Plain,
                );
                self.push_command_info(
                    format!("    Skill: {} {} [{}]", group.emoji, group.name, skill_idx),
                    LineStyle::Plain,
                );
                if !self.waiting {
                    self.push_command_info(
                        format!("    Messages: {}", self.agent_ref().session.messages.len()),
                        LineStyle::Plain,
                    );
                    self.push_command_info(
                        format!(
                            "    Tools: {} available",
                            self.agent_ref().active_tools().len()
                        ),
                        LineStyle::Plain,
                    );
                } else {
                    self.push_command_info("    Agent: ⏳ busy", LineStyle::Dim);
                }
                self.push_command_info("    Task:", LineStyle::Info);
                match crate::task::Task::load_active() {
                    Ok(Some(task)) => {
                        self.push_command_info(format!("      ID: {}", task.id), LineStyle::Plain);
                        self.push_command_info(
                            format!("      Title: {}", task.title),
                            LineStyle::Plain,
                        );
                        self.push_command_info(
                            format!("      Status: {}", task.status),
                            LineStyle::Plain,
                        );
                        self.push_command_info(
                            format!("      Steps: {}", task.steps.len()),
                            LineStyle::Plain,
                        );
                    }
                    Ok(None) => {
                        self.push_command_info("      No active task", LineStyle::Dim);
                    }
                    Err(e) => {
                        self.push_command_info(
                            format!("      Error loading task: {}", e),
                            LineStyle::Error,
                        );
                    }
                }
            }
            "cls" | "clear" => {
                return Ok(CommandResult::ClearScreen);
            }
            "rg" | "grep" => {
                if arg.is_empty() {
                    self.push_command_info("  Usage: :rg <pattern>", LineStyle::Dim);
                } else {
                    let output = std::process::Command::new("rg")
                        .arg("--color")
                        .arg("never")
                        .arg("--heading")
                        .arg("-n")
                        .arg(arg)
                        .arg(".")
                        .output()
                        .or_else(|_| {
                            std::process::Command::new("grep")
                                .arg("--color=never")
                                .arg("-rnH")
                                .arg(arg)
                                .arg(".")
                                .output()
                        });
                    match output {
                        Ok(out) => {
                            let stdout_str = String::from_utf8_lossy(&out.stdout);
                            let lines: Vec<&str> = stdout_str.lines().collect();

                            let new_buf_idx = if self.buffer().name() == "rg" {
                                self.active_buffer
                            } else {
                                let idx = self.buffers.len();
                                self.buffers.push(ResponseBuffer::with_name("rg"));
                                idx
                            };
                            self.active_buffer = new_buf_idx;
                            self.buffers[new_buf_idx].clear();

                            let c_idx = self.console_buffer_idx();
                            self.buffers[c_idx].push(BufferLine::new(
                                format!("  🔍 rg: {}", arg),
                                LineStyle::Info,
                            ));

                            let mut is_grep = false;
                            for line in lines.iter() {
                                if line.is_empty() {
                                    self.buffer_mut().push_blank();
                                    continue;
                                }
                                if let Some(colon1) = line.find(':') {
                                    if let Some(colon2) = line[colon1 + 1..].find(':') {
                                        let file = &line[..colon1];
                                        let line_num_str = &line[colon1 + 1..colon1 + 1 + colon2];
                                        if let Ok(line_num) = line_num_str.parse::<usize>() {
                                            is_grep = true;
                                            let content = &line[colon1 + 1 + colon2 + 1..];
                                            let segments = vec![
                                                (file.to_string(), LineStyle::ToolResult),
                                                (":".to_string(), LineStyle::Dim),
                                                (line_num.to_string(), LineStyle::Info),
                                                (":".to_string(), LineStyle::Dim),
                                                (content.to_string(), LineStyle::Plain),
                                            ];
                                            self.buffer_mut()
                                                .push(BufferLine::from_segments(segments));
                                            continue;
                                        }
                                    }
                                }

                                if !is_grep {
                                    if let Some(colon) = line.find(':') {
                                        let line_num_str = &line[..colon];
                                        if let Ok(line_num) = line_num_str.parse::<usize>() {
                                            let content = &line[colon + 1..];
                                            let segments = vec![
                                                (line_num.to_string(), LineStyle::Info),
                                                (":".to_string(), LineStyle::Dim),
                                                (content.to_string(), LineStyle::Plain),
                                            ];
                                            self.buffer_mut()
                                                .push(BufferLine::from_segments(segments));
                                            continue;
                                        }
                                    }
                                    self.buffer_mut().push(BufferLine::new(
                                        line.to_string(),
                                        LineStyle::ToolResult,
                                    ));
                                }
                            }

                            if lines.is_empty() {
                                self.push_line("  No matches found", LineStyle::Dim);
                            }
                            self.push_line(format!("  [{} lines]", lines.len()), LineStyle::Dim);
                            self.scroll_to_bottom();
                        }
                        Err(e) => self.push_command_info(
                            format!("  ❌ Search failed: {}", e),
                            LineStyle::Error,
                        ),
                    }
                }
            }
            "fd" | "find" => {
                if arg.is_empty() {
                    self.push_command_info("  Usage: :fd <pattern>", LineStyle::Dim);
                } else {
                    let output = std::process::Command::new("fd")
                        .arg(arg)
                        .output()
                        .or_else(|_| {
                            std::process::Command::new("find")
                                .arg(".")
                                .arg("-name")
                                .arg(arg)
                                .output()
                        });
                    match output {
                        Ok(out) => {
                            let stdout_str = String::from_utf8_lossy(&out.stdout);
                            let lines: Vec<&str> = stdout_str.lines().collect();

                            let new_buf_idx = if self.buffer().name() == "fd" {
                                self.active_buffer
                            } else {
                                let idx = self.buffers.len();
                                self.buffers.push(ResponseBuffer::with_name("fd"));
                                idx
                            };
                            self.active_buffer = new_buf_idx;
                            self.buffers[new_buf_idx].clear();

                            let c_idx = self.console_buffer_idx();
                            self.buffers[c_idx].push(BufferLine::new(
                                format!("  🔍 fd: {}", arg),
                                LineStyle::Info,
                            ));

                            for line in lines.iter().take(1000) {
                                if line.is_empty() {
                                    continue;
                                }
                                let path = line.trim_start_matches("./");
                                self.buffers[new_buf_idx]
                                    .push(BufferLine::new(path.to_string(), LineStyle::Plain));
                            }

                            if lines.is_empty() {
                                self.buffers[new_buf_idx].push(BufferLine::new(
                                    "  No matches found".to_string(),
                                    LineStyle::Dim,
                                ));
                            }
                            self.buffers[new_buf_idx].push(BufferLine::new(
                                format!("  [{} files]", lines.len()),
                                LineStyle::Dim,
                            ));
                            self.scroll_to_bottom();
                        }
                        Err(e) => self.push_command_info(
                            format!("  ❌ Find failed: {}", e),
                            LineStyle::Error,
                        ),
                    }
                }
            }
            "workflow" => {
                if self.waiting {
                    self.push_command_info("  ⏳ Agent is busy", LineStyle::Dim);
                } else {
                    let prompt = self
                        .agent_ref()
                        .session
                        .messages
                        .get(0)
                        .and_then(|m| m["content"].as_str())
                        .map(|s| s.to_string());
                    if let Some(prompt) = prompt {
                        self.push_command_info("  📜 Active System Prompt:", LineStyle::Info);
                        self.buffer_mut().push_blank();
                        for line in prompt.lines() {
                            self.push_command_info(format!("    {}", line), LineStyle::Plain);
                        }
                        self.buffer_mut().push_blank();
                        if self.agent_ref().system_prompt_override.is_some() {
                            self.push_command_info(
                                "  ✅ Local CODER.md is overriding system prompt.",
                                LineStyle::ToolResult,
                            );
                        } else {
                            self.push_command_info(
                                "  ❌ Using built-in system prompt.",
                                LineStyle::Dim,
                            );
                        }
                    } else {
                        self.push_command_info("  ⚠️ No system prompt found.", LineStyle::Error);
                    }
                }
            }
            "gs" => {
                self.show_git_status(stdout, None)?;
            }
            "ls" => {
                let path = if arg.is_empty() { "." } else { arg };
                let output = std::process::Command::new("ls")
                    .arg("-la")
                    .arg(path)
                    .output();
                match output {
                    Ok(out) => {
                        let stdout_str = String::from_utf8_lossy(&out.stdout);
                        for line in stdout_str.lines() {
                            self.push_command_info(format!("    {}", line), LineStyle::Plain);
                        }
                    }
                    Err(e) => {
                        self.push_command_info(format!("  ❌ ls failed: {}", e), LineStyle::Error)
                    }
                }
            }
            "bn" => self.switch_buffer(1),
            "bp" => self.switch_buffer(-1),
            "bd" => self.close_buffer(),
            _ => {
                self.push_command_info(
                    format!("  ❌ Unknown command: :{}", command),
                    LineStyle::Error,
                );
                self.push_command_info("  Type :help for available commands", LineStyle::Dim);
            }
        }
        self.scroll_to_bottom();
        Ok(CommandResult::Continue)
    }

    pub(super) fn apply_sed_changes(&mut self, stdout: &mut io::Stdout) -> anyhow::Result<()> {
        let lines = self.buffer().lines();
        let mut changes_to_apply: std::collections::HashMap<String, Vec<(usize, String)>> =
            std::collections::HashMap::new();

        let mut i = 0;
        while i < lines.len() {
            let content = lines[i].content();
            if content.starts_with("@@ ") {
                let parts: Vec<&str> = content[3..].splitn(2, ':').collect();
                if parts.len() == 2 {
                    let file = parts[0].to_string();
                    let line_num = parts[1].parse::<usize>().unwrap_or(0);

                    if i + 2 < lines.len() {
                        let old_line_content = lines[i + 1].content();
                        let new_line_content = lines[i + 2].content();

                        if old_line_content.starts_with("- ") && new_line_content.starts_with("+ ")
                        {
                            let new_text = new_line_content[2..].to_string();
                            changes_to_apply
                                .entry(file)
                                .or_default()
                                .push((line_num, new_text));
                            i += 3;
                            if i < lines.len() && lines[i].content().is_empty() {
                                i += 1;
                            }
                            continue;
                        }
                    }
                }
            }
            i += 1;
        }

        let mut applied_count: usize = 0;
        let mut failed_count: usize = 0;

        for (file, mut changes) in changes_to_apply {
            changes.sort_by(|a, b| b.0.cmp(&a.0));

            let root = std::path::PathBuf::from(&self.config.tools.project_root);
            let resolved = if std::path::Path::new(&file).is_absolute() {
                std::path::PathBuf::from(&file)
            } else {
                root.join(&file)
            };

            if let Ok(content) = std::fs::read_to_string(&resolved) {
                let has_trailing_newline = content.ends_with('\n');
                let mut lines_vec: Vec<String> = content.lines().map(|l| l.to_string()).collect();
                let mut modified = false;

                for (line_num, new_text) in &changes {
                    let idx = line_num.saturating_sub(1);
                    if idx < lines_vec.len() {
                        if lines_vec[idx] != *new_text {
                            lines_vec[idx] = new_text.clone();
                            modified = true;
                        }
                        applied_count += 1;
                    } else {
                        failed_count += 1;
                    }
                }

                if modified {
                    let mut new_content = lines_vec.join("\n");
                    if has_trailing_newline {
                        new_content.push('\n');
                    }
                    if std::fs::write(&resolved, new_content).is_err() {
                        failed_count += changes.len();
                        applied_count = applied_count.saturating_sub(changes.len());
                    }
                }
            } else {
                failed_count += changes.len();
            }
        }

        self.push_info(
            format!(
                "  ✅ Applied {} changes. {} failed.",
                applied_count, failed_count
            ),
            if failed_count > 0 {
                LineStyle::Error
            } else {
                LineStyle::ToolResult
            },
        );
        self.close_buffer();
        self.scroll_to_bottom();
        self.render(stdout)?;
        Ok(())
    }

    fn push_help(&mut self) {
        self.push_command_info("  pcode — Vim-Modal Commands", LineStyle::Info);
        self.buffer_mut().push_blank();
        let cmds: &[(&str, &str)] = &[
            (":q / :quit", "Exit the REPL"),
            (":cancel", "Cancel running agent task"),
            (":help", "Show this help"),
            (":ssave <name>", "Save current session"),
            (":sload <name>", "Load a saved session"),
            (":sessions", "List saved sessions"),
            (":delete <name>", "Delete a saved session"),
            (":reset", "Reset conversation"),
            (":config", "Show current config"),
            (":tools", "Show active tools"),
            (":debug", "Toggle debug mode"),
            (":status", "Show session status"),
            (":cls", "Clear response buffer"),
            (":skills", "Show skill group popup"),
            (":skills <n|name>", "Switch skill group"),
            (":skills next", "Cycle to next group"),
            (":skills toggle", "Toggle tools on/off"),
            (":rg <pattern>", "Search code (rg/grep) in new buffer"),
            (":fd <pattern>", "Find files (fd/find)"),
            (":ls [path]", "List directory"),
            (":workflow", "Show active system prompt & CODER.md status"),
            (
                ":write [path]",
                "Save buffer to file (defaults to buffer name)",
            ),
            (":bn / :bp", "Next / Previous buffer"),
            (":bd", "Close buffer"),
        ];
        for (cmd, desc) in cmds {
            self.push_command_info(format!("  {:<22} {}", cmd, desc), LineStyle::Plain);
        }
        self.buffer_mut().push_blank();
        self.push_command_info("  Normal mode keys:", LineStyle::Info);
        let keys: &[(&str, &str)] = &[
            ("i / a / o", "Enter Insert mode"),
            (": / >", "Enter Command mode"),
            ("F12", "Cancel running agent task"),
            ("j / k", "Scroll down / up"),
            ("G", "Go to bottom (5G → line 5)"),
            ("K", "Grep word under cursor"),
            ("gg", "Go to top"),
            ("yy", "Yank line to clipboard"),
            ("dd (5dd)", "Delete line (5 lines)"),
            ("u", "Undo line deletion"),
            ("o", "Open in $EDITOR"),
            ("l / L", "Next / Previous git hunk"),
            ("C-d / C-u", "Half page down / up"),
            ("C-f / C-b", "Page down / up"),
            ("Space / Enter", "Enter Insert mode"),
            ("Alt-x", "Close buffer"),
            ("Alt-- / Alt-=", "Previous / Next buffer"),
            ("Esc", "Cancel / back to Normal"),
        ];
        for (key, desc) in keys {
            self.push_command_info(format!("  {:<22} {}", key, desc), LineStyle::Plain);
        }
        self.buffer_mut().push_blank();
        self.push_command_info("  Insert mode keys:", LineStyle::Info);
        let insert_keys: &[(&str, &str)] = &[
            ("Enter", "Send message to LLM"),
            ("Alt+Enter", "Insert literal newline"),
            ("C-a / C-e", "Home / End"),
            ("C-u / C-k", "Kill to start / end"),
            ("C-w", "Delete word backward"),
            ("Up / Down", "History navigation"),
            ("Esc", "Back to Normal mode"),
        ];
        for (key, desc) in insert_keys {
            self.push_command_info(format!("  {:<22} {}", key, desc), LineStyle::Plain);
        }
    }
}
