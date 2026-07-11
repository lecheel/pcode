// src/repl/handle/event.rs
//! Main event loop + LLM submission.

use super::super::*;
use crate::agent::{AgentEvent, PatchAgent};
use crate::repl::buffer::{BufferLine, LineStyle};
use crate::repl::misc;
use crate::repl::CommandResult;
use crate::repl::Mode;
use crossterm::event::{self, Event, KeyEventKind};
use std::io;

impl Repl {
    pub(crate) fn push_welcome(&mut self) {
        self.push_llm_line("pcode — vim-modal patch REPL", LineStyle::Info);
        let idx = self.llm_buffer_idx();
        self.buffers[idx].push_blank();
        self.push_llm_line(
            "  i                → enter Insert mode (type message to LLM)",
            LineStyle::Dim,
        );
        self.push_llm_line(
            "  :                → enter Command mode (:help, :quit, …)",
            LineStyle::Dim,
        );
        self.push_llm_line("  F12              → Cancel running task", LineStyle::Tool);
        self.push_llm_line("  Esc              → back to Normal mode", LineStyle::Dim);
        self.push_llm_line(
            "  j/k G gg C-d C-u → scroll response buffer",
            LineStyle::Dim,
        );
        self.push_llm_line(
            "  yy               → Yank line to clipboard",
            LineStyle::Dim,
        );
        self.push_llm_line("  dd (5dd)         → Delete line (5 lines)", LineStyle::Dim);
        self.push_llm_line("  p                → paste", LineStyle::Dim);
        self.push_llm_line("  u                → Undo line deletion", LineStyle::Dim);
        self.push_llm_line("  o                → Open via $EDITOR", LineStyle::Tool);
        self.push_llm_line(
            "  >                → Visual Selection to >>llm",
            LineStyle::Tool,
        );
        self.push_llm_line("  l / L            → hunkNext/hunkPrev", LineStyle::Dim);
        self.push_llm_line("  Alt-d            → Delete line", LineStyle::Dim);
        self.push_llm_line("  Alt-w            → Write buffer", LineStyle::Dim);
        self.push_llm_line("  Alt-x            → Close buffer", LineStyle::Dim);
        self.push_llm_line(
            "  :sed /search/replace :fd main.rs :rg fn main  (replace, find, grep)",
            LineStyle::Tool,
        );
        self.push_llm_line(
            "  Alt-- / Alt-=    → Previous / Next buffer",
            LineStyle::Dim,
        );
        self.buffers[idx].push_blank();
    }

    pub(crate) async fn event_loop(&mut self, stdout: &mut io::Stdout) -> anyhow::Result<()> {
        let spinner_chars = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let mut spinner_idx: usize = 0;
        let mut last_spinner_update = std::time::Instant::now();
        loop {
            let mut pending_events = Vec::new();
            if let Some(rx) = self.event_rx.as_mut() {
                while let Ok(event) = rx.try_recv() {
                    pending_events.push(event);
                }
            }
            let mut need_render = false;
            for event in pending_events {
                match &event {
                    AgentEvent::Thinking { .. } => {}
                    AgentEvent::RunningTool { name } => {
                        let ts = misc::get_timestamp();
                        self.push_info(format!("[{}] ⏳ Running {}...", ts, name), LineStyle::Dim);
                        need_render = true;
                    }
                    AgentEvent::Verifying => {
                        self.push_llm_line("  🔍 Verifying changes...", LineStyle::Dim);
                        self.scroll_llm_to_bottom();
                        need_render = true;
                    }
                    AgentEvent::ToolCall { name, summary } => {
                        let ts = misc::get_timestamp();
                        self.push_info(format!("[{}] ⚙️  {}", ts, name), LineStyle::Tool);
                        for line in summary.lines() {
                            self.push_info(format!("       {}", line), LineStyle::Tool);
                        }
                        need_render = true;
                    }
                    AgentEvent::ToolResult {
                        success, summary, ..
                    } => {
                        if !summary.is_empty() {
                            let style = if *success {
                                LineStyle::ToolResult
                            } else {
                                LineStyle::Error
                            };
                            let icon = if *success { "✅" } else { "❌" };
                            self.push_llm_line(format!("     {} {}", icon, summary), style);
                            self.scroll_llm_to_bottom();
                        }
                        need_render = true;
                    }
                    AgentEvent::DiffLine { line } => {
                        self.push_llm_line(format!("     {}", line), LineStyle::Plain);
                        need_render = true;
                    }
                    AgentEvent::Reasoning { preview } => {
                        self.push_llm_line(format!("  {}", preview), LineStyle::Dim);
                        self.scroll_llm_to_bottom();
                        need_render = true;
                    }
                    AgentEvent::Done => {}
                }
                self.last_event = Some(event);
            }
            if self.waiting {
                let mut agent_result = None;
                if let Some(rx) = self.agent_rx.as_mut() {
                    match rx.try_recv() {
                        Ok(pair) => agent_result = Some(Ok(pair)),
                        Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                            agent_result = Some(Err(()));
                        }
                        Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
                    }
                }
                if let Some(result) = agent_result {
                    self.agent_rx = None;
                    self.agent_handle = None;
                    self.cancel_tx = None;
                    self.waiting = false;
                    self.thinking_start = None;
                    self.event_rx = None;
                    self.last_event = None;
                    match result {
                        Ok((agent, response)) => {
                            self.agent = Some(agent);
                            self.cached_skill_group = self.agent_ref().active_skill_group;
                            let llm_idx = self.llm_buffer_idx();
                            self.buffers[llm_idx].push_blank();
                            self.buffers[llm_idx].push_str(&response, LineStyle::Assistant);
                            self.active_buffer = llm_idx;
                            self.scroll_to_bottom();
                            need_render = true;
                        }
                        Err(()) => {
                            self.push_llm_line(
                                "  ❌ Agent task failed unexpectedly.",
                                LineStyle::Error,
                            );
                        }
                    }
                    self.scroll_llm_to_bottom();
                    need_render = true;
                }
            }
            if self.waiting {
                let now = std::time::Instant::now();
                if now.duration_since(last_spinner_update) >= std::time::Duration::from_millis(80)
                    || need_render
                {
                    spinner_idx = (spinner_idx + 1) % spinner_chars.len();
                    last_spinner_update = now;
                    self.spinner_char = spinner_chars[spinner_idx].to_string();
                    if need_render {
                        let _ = self.render(stdout);
                        need_render = false;
                    } else {
                        let _ = self.render_spinner_only(stdout);
                    }
                }
            }
            if need_render {
                self.render(stdout)?;
                need_render = false;
            }
            let timeout = std::time::Duration::from_millis(50);
            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        self.handle_key(key, stdout)?;
                    }
                    Event::Key(key) if key.kind == KeyEventKind::Repeat => {
                        if matches!(self.mode, Mode::Normal) && !self.waiting {
                            self.handle_normal_repeat(key, stdout)?;
                        }
                    }
                    Event::Resize(w, h) => {
                        self.width = w;
                        self.height = h;
                        self.ensure_cursor_visible();
                        self.render(stdout)?;
                    }
                    _ => {}
                }
            }
        }
    }

    pub(crate) fn submit_input(
        &mut self,
        stdout: &mut io::Stdout,
        input: String,
    ) -> anyhow::Result<()> {
        if input.trim().is_empty() {
            self.mode = Mode::Normal;
            if self.pending_snippet.take().is_some() {
                self.push_llm_line("  [cancelled addition]", LineStyle::Dim);
            }
            self.render(stdout)?;
            return Ok(());
        }

        let snippet = self.pending_snippet.take();
        let is_addition = snippet.is_some();

        if !is_addition && input.starts_with(':') {
            self.editor.save_history(&self.config.repl.history_file);
            let cmd = input.trim_start_matches(':').trim().to_string();
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

        let final_input = if let Some(s) = snippet {
            format!("{}\n\n{}", s, input)
        } else {
            input
        };

        let input_lower = final_input.to_lowercase();

        let code_keywords = [
            "impl ",
            "create ",
            "write ",
            "fix ",
            "build ",
            "refactor ",
            "implement ",
            "add ",
            "debug ",
        ];
        let is_code_request = code_keywords.iter().any(|kw| input_lower.starts_with(kw));

        // Inside submit_input, replace the is_code_request block with:
        if is_code_request && self.active_skill_group() == 0 {
            if self.config.repl.auto_enable_tools_on_code_request {
                let (code_idx, group_name) = {
                    let groups = self.skill_groups();
                    let code_idx = groups.iter().position(|g| g.name == "Code").unwrap_or(5);
                    let code_idx = code_idx.min(groups.len().saturating_sub(1));
                    let group_name = groups
                        .get(code_idx)
                        .map(|g| g.name.clone())
                        .unwrap_or_default();
                    (code_idx, group_name)
                };

                self.agent_mut().set_skill_group(code_idx);
                self.cached_skill_group = code_idx;
                self.push_llm_line(
                    format!(
                        "  ✨ Auto-switched to '{}' mode for code request.",
                        group_name
                    ),
                    LineStyle::ToolResult,
                );
            } else {
                let ts = misc::get_timestamp();
                self.push_llm_line(format!("[{}] > {}", ts, final_input), LineStyle::User);
                self.push_llm_line(
                    "  ⛔ Blocked: You are in 'Chat' mode (no tools available).",
                    LineStyle::Error,
                );
                self.push_llm_line(
                    "  To write or edit code, switch to 'Code' or 'Full' mode first.",
                    LineStyle::Dim,
                );
                self.push_llm_line(
                    "  Press F5 (Code) or F6 (Full), or type :skills Code",
                    LineStyle::Dim,
                );
                let llm_idx = self.llm_buffer_idx();
                self.buffers[llm_idx].push_blank();
                self.active_buffer = llm_idx;
                self.mode = Mode::Normal;
                self.scroll_to_bottom();
                self.render(stdout)?;
                return Ok(());
            }
        }
        self.editor.save_history(&self.config.repl.history_file);
        let ts = misc::get_timestamp();
        let llm_idx = self.llm_buffer_idx();
        self.buffers[llm_idx].push(BufferLine::new(
            format!("[{}] > {}", ts, final_input),
            LineStyle::User,
        ));
        self.active_buffer = llm_idx;
        self.waiting = true;
        self.thinking_start = Some(std::time::Instant::now());
        self.last_event = None;
        self.mode = Mode::Normal;
        self.scroll_to_bottom();
        self.render(stdout)?;
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
        self.cancel_tx = Some(cancel_tx);

        let mut agent = match self.agent.take() {
            Some(a) => a,
            None => {
                self.push_llm_line(
                    "  ❌ Agent is missing. Cannot submit input. Please restart the application.",
                    LineStyle::Error,
                );
                self.mode = Mode::Normal;
                self.waiting = false;
                self.render(stdout)?;
                return Ok(());
            }
        };

        self.cached_skill_group = agent.active_skill_group;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        agent.set_event_channel(tx);
        self.event_rx = Some(rx);
        let (result_tx, result_rx) = tokio::sync::oneshot::channel::<(PatchAgent, String)>();
        let handle = tokio::task::spawn(async move {
            let response = agent.run_cycle(&final_input, cancel_rx).await;
            let _ = result_tx.send((agent, response));
        });
        self.agent_rx = Some(result_rx);
        self.agent_handle = Some(handle);
        Ok(())
    }
}
