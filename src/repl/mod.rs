pub mod buffer;
pub mod editor;
pub mod helper;
pub mod mode;
use crate::agent::{PatchAgent, SKILL_GROUPS};
use crate::config::AppConfig;
use arboard::Clipboard;
use buffer::{BufferLine, LineStyle, ResponseBuffer};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{self, Attribute, Color, Print, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal::{self, ClearType},
};
use editor::LineEditor;
use helper::{Popup, PopupItem};
use mode::Mode;
use std::io::{self, Write};
use unicode_width::UnicodeWidthStr;
const COMMAND_LIST: &[&str] = &[
    "quit", "q", "exit", "help", "h", "?", "save", "load", "sessions", "delete", "rm", "reset",
    "config", "tools", "debug", "status", "cls", "clear", "skills", "rg", "grep", "fd", "find",
    "ls", "cancel", "bn", "bp", "bd", "open", "e", "saveas", "write", "workflow", "gs",
];
struct TerminalGuard;
impl TerminalGuard {
    fn init(stdout: &mut io::Stdout) -> anyhow::Result<Self> {
        terminal::enable_raw_mode()?;
        execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide)?;
        Ok(Self)
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        let _ = execute!(stdout, cursor::Show, terminal::LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}
#[derive(PartialEq, Clone, Copy)]
pub enum PopupMode {
    SkillGroups,
    FilePicker,
    TaskFilePicker,
}
enum CommandResult {
    Continue,
    Quit,
    ClearScreen,
}
pub struct Repl {
    mode: Mode,
    buffers: Vec<ResponseBuffer>,
    active_buffer: usize,
    llm_buffer_idx: Option<usize>,
    console_buffer_idx: Option<usize>,
    editor: LineEditor,
    cmd_editor: LineEditor,
    agent: Option<PatchAgent>,
    config: AppConfig,
    width: u16,
    height: u16,
    waiting: bool,
    pending: Option<char>,
    count: Option<usize>,
    popup: Popup,
    popup_mode: PopupMode,
    agent_rx: Option<tokio::sync::oneshot::Receiver<(PatchAgent, String)>>,
    agent_handle: Option<tokio::task::JoinHandle<()>>,
    cached_skill_group: usize,
    thinking_start: Option<std::time::Instant>,
    event_rx: Option<tokio::sync::mpsc::UnboundedReceiver<crate::agent::AgentEvent>>,
    last_event: Option<crate::agent::AgentEvent>,
    cancel_tx: Option<tokio::sync::oneshot::Sender<()>>,
    spinner_char: String,
    search_query: Option<String>,
    search_matches: Vec<usize>,
    search_match_idx: Option<usize>,
    stash_pop_target: Option<String>,
    cached_git_info: String,
}
const INPUT_AREA_ROWS: usize = 2;
impl Repl {
    pub fn new(agent: PatchAgent, config: AppConfig) -> Self {
        let (width, height) = terminal::size().unwrap_or((80, 24));
        let mut editor = LineEditor::new();
        editor.load_history(&config.repl.history_file);
        let cached_skill_group = agent.active_skill_group;
        Self {
            mode: Mode::Insert,
            buffers: vec![ResponseBuffer::with_name("Chat")],
            active_buffer: 0,
            llm_buffer_idx: Some(0),
            console_buffer_idx: None,
            editor,
            cmd_editor: LineEditor::new(),
            agent: Some(agent),
            config,
            width,
            height,
            waiting: false,
            pending: None,
            count: None,
            popup: Popup::new(),
            popup_mode: PopupMode::SkillGroups,
            agent_rx: None,
            agent_handle: None,
            cached_skill_group,
            thinking_start: None,
            event_rx: None,
            last_event: None,
            cancel_tx: None,
            spinner_char: String::new(),
            search_query: None,
            search_matches: Vec::new(),
            search_match_idx: None,
            stash_pop_target: None,
            cached_git_info: String::new(),
        }
    }
    fn buffer(&self) -> &ResponseBuffer {
        &self.buffers[self.active_buffer]
    }
    fn buffer_mut(&mut self) -> &mut ResponseBuffer {
        &mut self.buffers[self.active_buffer]
    }
    fn llm_buffer_idx(&mut self) -> usize {
        if let Some(idx) = self.llm_buffer_idx {
            if idx < self.buffers.len() {
                return idx;
            }
        }
        let idx = self.buffers.len();
        self.buffers.push(ResponseBuffer::with_name("Chat"));
        self.llm_buffer_idx = Some(idx);
        idx
    }
    fn console_buffer_idx(&mut self) -> usize {
        if let Some(idx) = self.console_buffer_idx {
            if idx < self.buffers.len() {
                return idx;
            }
        }
        let idx = self.buffers.len();
        self.buffers.push(ResponseBuffer::with_name("Console"));
        self.console_buffer_idx = Some(idx);
        idx
    }
    fn push_line(&mut self, content: impl Into<String>, style: LineStyle) {
        self.buffer_mut().push(BufferLine::new(content, style));
    }
    fn push_command_info(&mut self, content: impl Into<String>, style: LineStyle) {
        let idx = self.console_buffer_idx();
        self.buffers[idx].push(BufferLine::new(content, style));
        self.active_buffer = idx;
    }
    fn push_llm_line(&mut self, content: impl Into<String>, style: LineStyle) {
        let idx = self.llm_buffer_idx();
        self.buffers[idx].push(BufferLine::new(content, style));
    }
    fn scroll_llm_to_bottom(&mut self) {
        let idx = self.llm_buffer_idx();
        let h = self.response_area_height();
        let w = self.content_width();
        self.buffers[idx].scroll_to_bottom(h, w);
    }
    fn scroll_to_bottom(&mut self) {
        let h = self.response_area_height();
        let w = self.content_width();
        self.buffer_mut().scroll_to_bottom(h, w);
    }
    fn ensure_cursor_visible(&mut self) {
        let h = self.response_area_height();
        let w = self.content_width();
        self.buffer_mut().ensure_cursor_visible(h, w);
    }
    fn move_bottom(&mut self) {
        let h = self.response_area_height();
        let w = self.content_width();
        self.buffer_mut().move_bottom(h, w);
    }
    fn half_page_down(&mut self) {
        let h = self.response_area_height();
        let w = self.content_width();
        self.buffer_mut().half_page_down(h, w);
    }
    fn half_page_up(&mut self) {
        let h = self.response_area_height();
        let w = self.content_width();
        self.buffer_mut().half_page_up(h, w);
    }
    fn scroll_to_bottom_view(&mut self) {
        let h = self.response_area_height();
        let w = self.content_width();
        self.buffer_mut().scroll_to_bottom_view(h, w);
    }
    fn set_cursor(&mut self, line: usize, col: usize) {
        self.buffer_mut().set_cursor(line, col);
    }
    fn switch_buffer(&mut self, direction: i32) {
        if self.buffers.len() > 1 {
            if direction > 0 {
                self.active_buffer = (self.active_buffer + 1) % self.buffers.len();
            } else if self.active_buffer == 0 {
                self.active_buffer = self.buffers.len() - 1;
            } else {
                self.active_buffer -= 1;
            }
            self.scroll_to_bottom();
        } else {
            self.push_command_info("  Only 1 buffer", LineStyle::Dim);
            self.scroll_to_bottom();
        }
    }
    fn close_buffer(&mut self) {
        if self.buffers.len() <= 1 {
            self.buffer_mut().clear();
            self.push_command_info(
                "  Cannot close last buffer, cleared instead.",
                LineStyle::Dim,
            );
            self.scroll_to_bottom();
            return;
        }

        let closed_idx = self.active_buffer;

        // Prevent closing primary buffers (Chat/Console), just clear them instead
        if self.llm_buffer_idx == Some(closed_idx) || self.console_buffer_idx == Some(closed_idx) {
            self.buffers[closed_idx].clear();
            self.push_command_info(
                "  Cannot close primary buffers (Chat/Console). Cleared instead.",
                LineStyle::Dim,
            );
            self.scroll_to_bottom();
            return;
        }

        self.buffers.remove(closed_idx);

        if let Some(idx) = self.llm_buffer_idx.as_mut() {
            if *idx > closed_idx {
                *idx -= 1;
            }
        }

        if let Some(idx) = self.console_buffer_idx.as_mut() {
            if *idx > closed_idx {
                *idx -= 1;
            }
        }

        if self.active_buffer >= self.buffers.len() {
            self.active_buffer = self.buffers.len() - 1;
        }
        self.scroll_to_bottom();
    }
    fn response_area_height(&self) -> usize {
        self.height as usize - INPUT_AREA_ROWS
    }
    fn agent_ref(&self) -> &PatchAgent {
        self.agent.as_ref().expect("agent missing")
    }
    fn agent_mut(&mut self) -> &mut PatchAgent {
        self.agent.as_mut().expect("agent missing")
    }
    fn active_skill_group(&self) -> usize {
        if self.agent.is_some() {
            self.agent_ref().active_skill_group
        } else {
            self.cached_skill_group
        }
    }
    pub async fn run(&mut self, initial_prompt: Option<String>) -> anyhow::Result<()> {
        let mut stdout = io::stdout();
        let _guard = TerminalGuard::init(&mut stdout)?;
        self.push_welcome();
        self.scroll_to_bottom();
        self.render(&mut stdout)?;
        if let Some(prompt) = initial_prompt {
            self.submit_input(&mut stdout, prompt)?;
        }
        self.event_loop(&mut stdout).await
    }
    fn push_welcome(&mut self) {
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
        self.push_llm_line("  F12              → Cancel running task", LineStyle::Dim);
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
        self.push_llm_line("  u                → Undo line deletion", LineStyle::Dim);
        self.push_llm_line("  o                → Open via $EDITOR", LineStyle::Dim);
        self.push_llm_line("  l / L            → hunkNext/hunkPrev", LineStyle::Dim);
        self.push_llm_line("  Alt-w            → Write buffer", LineStyle::Dim);
        self.push_llm_line("  Alt-x            → Close buffer", LineStyle::Dim);
        self.push_llm_line(
            "  Alt-- / Alt-=    → Previous / Next buffer",
            LineStyle::Dim,
        );
        self.buffers[idx].push_blank();
    }
    async fn event_loop(&mut self, stdout: &mut io::Stdout) -> anyhow::Result<()> {
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
                    crate::agent::AgentEvent::Thinking { .. } => {}
                    crate::agent::AgentEvent::RunningTool { name } => {
                        self.push_llm_line(format!("  ⏳ Running {}...", name), LineStyle::Dim);
                        self.scroll_llm_to_bottom();
                        need_render = true;
                    }
                    crate::agent::AgentEvent::Verifying => {
                        self.push_llm_line("  🔍 Verifying changes...", LineStyle::Dim);
                        self.scroll_llm_to_bottom();
                        need_render = true;
                    }
                    crate::agent::AgentEvent::ToolCall { summary, .. } => {
                        for line in summary.lines() {
                            self.push_llm_line(format!("  {}", line), LineStyle::Tool);
                        }
                        self.scroll_llm_to_bottom();
                        need_render = true;
                    }
                    crate::agent::AgentEvent::ToolResult {
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
                    crate::agent::AgentEvent::DiffLine { line } => {
                        self.push_llm_line(format!("     {}", line), LineStyle::Plain);
                        need_render = true;
                    }
                    crate::agent::AgentEvent::Reasoning { preview } => {
                        self.push_llm_line(format!("  {}", preview), LineStyle::Dim);
                        self.scroll_llm_to_bottom();
                        need_render = true;
                    }
                    crate::agent::AgentEvent::Done => {}
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
    fn handle_key(&mut self, key: KeyEvent, stdout: &mut io::Stdout) -> anyhow::Result<()> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.waiting {
                self.push_command_info(
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
        if key.modifiers.contains(KeyModifiers::ALT) {
            match key.code {
                KeyCode::Char('q') => {
                    self.editor.save_history(&self.config.repl.history_file);
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
                    self.execute_command("write", stdout)?;
                    self.render(stdout)?;
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
            if self.waiting {
                if let Some(tx) = self.cancel_tx.take() {
                    let _ = tx.send(());
                }
                self.push_command_info("  ⛔ Cancelling agent task...", LineStyle::Error);
                self.scroll_to_bottom();
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
        if key.code == KeyCode::F(9) {
            self.show_git_status(stdout, None)?;
            return Ok(());
        }
        let skill_idx = match key.code {
            KeyCode::F(1) => Some(0),
            KeyCode::F(2) => Some(4),
            KeyCode::F(3) => Some(7),
            _ => None,
        };
        if let Some(idx) = skill_idx {
            if idx < SKILL_GROUPS.len() && !self.waiting {
                self.agent_mut().set_skill_group(idx);
                self.cached_skill_group = idx;
                self.popup.hide();
                let group = &SKILL_GROUPS[idx];
                self.push_command_info(
                    format!("  {} {} — {}", group.emoji, group.name, group.description),
                    LineStyle::ToolResult,
                );
                self.scroll_to_bottom();
                self.render(stdout)?;
            }
            return Ok(());
        }
        if self.popup.active {
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
        }
        Ok(())
    }
    fn handle_normal_repeat(
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
    fn handle_normal_key(&mut self, key: KeyEvent, stdout: &mut io::Stdout) -> anyhow::Result<()> {
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

        // Enter handler for rg buffer (supports both grouped and single-line formats)
        if key.code == KeyCode::Enter && self.buffer().name() == "rg" {
            let cursor_line = self.buffer().cursor_line();
            if let Some(line) = self.buffer().lines().get(cursor_line) {
                let content = line.content();

                // Try to parse single-line format (file:line:content)
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

                // Try to parse grouped format (line:content)
                if let Some(colon1) = content.find(':') {
                    let line_num_str = &content[..colon1];
                    if let Ok(line_num) = line_num_str.parse::<usize>() {
                        // Look upwards for the file name
                        let mut file_to_open = None;
                        for i in (0..cursor_line).rev() {
                            if let Some(prev_line) = self.buffer().lines().get(i) {
                                let prev_content = prev_line.content();
                                if prev_content.is_empty() {
                                    continue;
                                }
                                // If it starts with a digit and has a colon, it's another match line
                                if prev_content.starts_with(|c: char| c.is_ascii_digit())
                                    && prev_content.contains(':')
                                {
                                    continue;
                                }
                                // Otherwise, it's the file header
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
                            let file = content.trim().to_string();

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
                            file_to_open = Some(content.trim().to_string());
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
            self.pending = None;
            self.render(stdout)?;
            return Ok(());
        }
        match key.code {
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
                            self.push_command_info(
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
            KeyCode::Char('O') => {
                self.mode = Mode::Insert;
                self.editor.clear();
                self.count = None;
            }
            KeyCode::Char(':') | KeyCode::Char('>') => {
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
                    let len = line.content().chars().count();
                    let col = if len > 0 { len - 1 } else { 0 };
                    self.set_cursor(line_idx, col);
                    self.ensure_cursor_visible();
                }
                self.count = None;
            }
            KeyCode::End => {
                let line_idx = self.buffer().cursor_line();
                if let Some(line) = self.buffer().lines().get(line_idx) {
                    let len = line.content().chars().count();
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
                self.pending = None;
                self.render_spinner_only(stdout)?;
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
            KeyCode::Char('l') => {
                if let Some(lines) = self.get_git_gutter_lines() {
                    if !lines.is_empty() {
                        let current_line = self.buffer().cursor_line() + 1;
                        let next = lines.iter().find(|&&l| l > current_line).or(lines.first());
                        if let Some(&target) = next {
                            self.buffer_mut().set_cursor(target - 1, 0);
                            self.ensure_cursor_visible();
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
                            self.ensure_cursor_visible();
                        }
                    }
                }
                self.count = None;
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
                    let len = line.content().chars().count();
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
                    self.pending = None;
                    self.count = None;
                } else {
                    self.pending = Some('g');
                    self.render(stdout)?;
                    return Ok(());
                }
            }
            KeyCode::Char('y') => {
                if self.pending == Some('y') {
                    let cursor_line = self.buffer().cursor_line();
                    let cursor_col = self.buffer().cursor_col();
                    if let Some(line) = self.buffer().lines().get(cursor_line) {
                        match arboard::Clipboard::new()
                            .and_then(|mut cb| cb.set_text(line.content().clone()))
                        {
                            Ok(_) => {
                                self.push_command_info(
                                    "  📋 Yanked line to clipboard".to_string(),
                                    LineStyle::Dim,
                                );
                            }
                            Err(e) => {
                                self.push_command_info(
                                    format!("  ❌ Clipboard error: {}", e),
                                    LineStyle::Error,
                                );
                            }
                        }
                        self.set_cursor(cursor_line, cursor_col);
                        self.scroll_to_bottom_view();
                    }
                    self.pending = None;
                    self.count = None;
                } else {
                    self.pending = Some('y');
                    self.render(stdout)?;
                    return Ok(());
                }
            }

            KeyCode::Char('d') => {
                if self.pending == Some('d') {
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
                            if content.starts_with("📄 ") {
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
                        if content.starts_with("📄 ") && cursor_line <= block_start + 3 {
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
                            self.push_command_info("  🗑️  Discarded change", LineStyle::Dim);
                            self.scroll_to_bottom_view();
                        }
                        self.pending = None;
                        self.count = None;
                        self.render(stdout)?;
                        return Ok(());
                    }

                    let cursor_line = self.buffer().cursor_line();
                    let lines_len = self.buffer().len();
                    if lines_len > 0 {
                        let end_line = (cursor_line + amount).min(lines_len);
                        let mut yanked_text = String::new();

                        for i in cursor_line..end_line {
                            if let Some(line) = self.buffer().lines().get(i) {
                                yanked_text.push_str(&line.content());
                                yanked_text.push('\n');
                            }
                        }
                        match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(yanked_text))
                        {
                            Ok(_) => {}
                            Err(e) => self.push_command_info(
                                format!("  ❌ Clipboard error: {}", e),
                                LineStyle::Error,
                            ),
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
                        self.push_command_info(
                            format!("  🗑️  Deleted {} line(s)", end_line - cursor_line),
                            LineStyle::Dim,
                        );
                        self.scroll_to_bottom_view();
                    }
                    self.pending = None;
                    self.count = None;
                } else {
                    self.pending = Some('d');
                    self.render(stdout)?;
                    return Ok(());
                }
            }
            KeyCode::Char('u') => {
                if self.buffer_mut().undo() {
                    self.push_command_info("  ↩️  Undone line deletion", LineStyle::Dim);
                } else {
                    self.push_command_info("  Nothing to undo", LineStyle::Dim);
                }
                self.scroll_to_bottom_view();
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
                    self.pending = None;
                    self.render(stdout)?;
                    return Ok(());
                }
            }
            _ => {
                self.count = None;
            }
        }
        self.pending = None;
        self.render(stdout)
    }
    fn handle_insert_key(&mut self, key: KeyEvent, stdout: &mut io::Stdout) -> anyhow::Result<()> {
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
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
            }
            KeyCode::Tab => {
                self.cmd_editor.tab_complete(COMMAND_LIST);
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
            KeyCode::Tab => {}
            _ => {}
        }
        self.render_spinner_only(stdout)
    }
    fn handle_search_key(&mut self, key: KeyEvent, stdout: &mut io::Stdout) -> anyhow::Result<()> {
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
            KeyCode::Enter => {
                self.cmd_editor.submit();
                self.mode = Mode::Normal;
            }
            KeyCode::Backspace => {
                self.cmd_editor.backspace();
                if self.cmd_editor.is_empty() {
                    self.mode = Mode::Normal;
                }
                self.update_search();
            }
            KeyCode::Char(c) => {
                self.cmd_editor.insert_char(c);
                self.update_search();
            }
            KeyCode::Left => self.cmd_editor.move_left(),
            KeyCode::Right => self.cmd_editor.move_right(),
            KeyCode::Home => self.cmd_editor.move_home(),
            KeyCode::End => self.cmd_editor.move_end(),
            _ => {}
        }
        if self.popup.active {
            self.render(stdout)
        } else {
            self.render(stdout)
        }
    }
    fn update_search(&mut self) {
        let query = self.cmd_editor.content().to_string();
        if query.is_empty() {
            self.search_query = None;
            self.search_matches.clear();
            self.search_match_idx = None;
            return;
        }
        let q_lower = query.to_lowercase();
        let matches: Vec<usize> = self
            .buffer()
            .lines()
            .iter()
            .enumerate()
            .filter_map(|(i, l)| {
                if l.content().to_lowercase().contains(&q_lower) {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        let changed = matches != self.search_matches;
        self.search_query = Some(query);
        self.search_matches = matches;
        if changed {
            self.search_match_idx = if self.search_matches.is_empty() {
                None
            } else {
                Some(0)
            };
            if let Some(idx) = self.search_match_idx {
                let target_line = self.search_matches[idx];
                self.buffer_mut().set_cursor(target_line, 0);
                self.ensure_cursor_visible();
            }
        }
    }
    fn submit_input(&mut self, stdout: &mut io::Stdout, input: String) -> anyhow::Result<()> {
        if input.trim().is_empty() {
            self.mode = Mode::Normal;
            self.render(stdout)?;
            return Ok(());
        }
        let input_lower = input.to_lowercase();
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
        if is_code_request && self.active_skill_group() == 0 {
            if self.config.repl.auto_enable_tools_on_code_request {
                self.agent_mut().set_skill_group(5);
                self.cached_skill_group = 5;
                let group = &SKILL_GROUPS[5];
                self.push_llm_line(
                    format!(
                        "  ✨ Auto-switched to '{}' mode for code request.",
                        group.name
                    ),
                    LineStyle::ToolResult,
                );
            } else {
                let ts = Self::get_timestamp();
                self.push_llm_line(format!("[{}] > {}", ts, input), LineStyle::User);
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
        let ts = Self::get_timestamp();
        let llm_idx = self.llm_buffer_idx();
        self.buffers[llm_idx].push(BufferLine::new(
            format!("[{}] > {}", ts, input),
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
        let mut agent = self.agent.take().expect("agent missing");
        self.cached_skill_group = agent.active_skill_group;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        agent.set_event_channel(tx);
        self.event_rx = Some(rx);
        let (result_tx, result_rx) = tokio::sync::oneshot::channel::<(PatchAgent, String)>();
        let handle = tokio::task::spawn(async move {
            let response = agent.run_cycle(&input, cancel_rx).await;
            let _ = result_tx.send((agent, response));
        });
        self.agent_rx = Some(result_rx);
        self.agent_handle = Some(handle);
        Ok(())
    }
    fn execute_command(
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
                if arg.is_empty() {
                    self.push_command_info(
                        "  Usage: :sed <find>|<replace> [path] or :sed /find/replace/ [path]",
                        LineStyle::Dim,
                    );
                } else {
                    let (find, replace, path) = if arg.starts_with('/') {
                        let parts: Vec<&str> = arg[1..].splitn(3, '/').collect();
                        if parts.len() >= 2 {
                            (
                                parts[0].to_string(),
                                parts[1].to_string(),
                                if parts.len() == 3 {
                                    parts[2].to_string()
                                } else {
                                    ".".to_string()
                                },
                            )
                        } else {
                            (String::new(), String::new(), ".".to_string())
                        }
                    } else {
                        let parts: Vec<&str> = arg.splitn(3, '|').collect();
                        if parts.len() >= 2 {
                            (
                                parts[0].to_string(),
                                parts[1].to_string(),
                                if parts.len() == 3 {
                                    parts[2].to_string()
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
                        .arg("-F") // Fixed string match (like sed)
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
                                            ("📄 ".to_string(), LineStyle::Info),
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
                                        "  - ",
                                        LineStyle::Error,
                                    );
                                    self.buffers[new_buf_idx]
                                        .push(BufferLine::from_segments(old_segments));

                                    let new_segments = highlight_segments(
                                        &new_line,
                                        &replace,
                                        LineStyle::Plain,
                                        LineStyle::ToolResult,
                                        "  + ",
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
                                self.buffers[new_buf_idx].push(BufferLine::new(
                                    format!(
                                        "  {} proposed changes. [dd] to discard, [w] to apply",
                                        count
                                    ),
                                    LineStyle::Info,
                                ));
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
                        self.buffer().name().to_string()
                    } else {
                        arg.to_string()
                    };
                    let path_str = if arg.is_empty() {
                        let name = self.buffer().name().to_string();
                        let name = if name == "rg" {
                            "rg_results.txt".to_string()
                        } else if name.is_empty() || name == "Chat" {
                            "chat.md".to_string()
                        } else {
                            name
                        };
                        let name = name.replace('*', "");
                        name
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
                    // FIX: clone each String instead of borrowing a temporary
                    let content = self
                        .buffer()
                        .lines()
                        .iter()
                        .map(|l| l.content().clone()) // <-- changed from .as_str()
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
                                // Check if it's a grep fallback format (file:line:content)
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

                                // rg --heading format
                                if !is_grep {
                                    // Check if it's a line number match (e.g. \"12:content\")
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
                                    // Otherwise, it's a file header
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
    fn handle_command_key(&mut self, key: KeyEvent, stdout: &mut io::Stdout) -> anyhow::Result<()> {
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
    fn handle_popup_key(&mut self, key: KeyEvent, stdout: &mut io::Stdout) -> anyhow::Result<()> {
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
                            let idx = self.popup.cursor;
                            self.set_skill_group(idx, stdout)?;
                        }
                        PopupMode::FilePicker | PopupMode::TaskFilePicker => {
                            if let Some(item) = self.popup.items.get(self.popup.cursor) {
                                let path = item.text.clone();
                                self.load_file_to_buffer(&path, stdout)?;
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
                self.popup.filter.push(c);
                let f = self.popup.filter.clone();
                self.popup.update_filter(&f);
            }
            KeyCode::Backspace => {
                self.popup.filter.pop();
                let f = self.popup.filter.clone();
                self.popup.update_filter(&f);
            }
            _ => {}
        }
        self.render(stdout)
    }
    fn render_spinner_only(&self, stdout: &mut io::Stdout) -> anyhow::Result<()> {
        let status_y = self.height - 2;
        queue!(stdout, cursor::MoveTo(0, status_y))?;
        let mode_str = self.mode.as_str();
        let mode_color = self.mode.status_color();
        let skill_idx = self.active_skill_group();
        let skill = &SKILL_GROUPS[skill_idx];
        let buffer_name = self.buffer().name();
        let max_name_len = 20;
        let truncated_name = if buffer_name.chars().count() > max_name_len {
            let mut s: String = buffer_name.chars().take(max_name_len - 3).collect();
            s.push_str("...");
            s
        } else {
            buffer_name.to_string()
        };
        let buffer_info = format!(
            "[{}/{}] {}",
            self.active_buffer + 1,
            self.buffers.len(),
            truncated_name
        );
        let git_info = if self.cached_git_info.is_empty() {
            String::new()
        } else {
            format!(" {} ", self.cached_git_info)
        };
        let status_text = if self.waiting {
            let elapsed = self
                .thinking_start
                .map(|t| t.elapsed().as_secs_f32())
                .unwrap_or(0.0);
            let detail = match &self.last_event {
                Some(crate::agent::AgentEvent::Thinking { round, max_rounds }) => {
                    format!("Think {}/{}", round, max_rounds)
                }
                Some(crate::agent::AgentEvent::RunningTool { name }) => format!("Run {}", name),
                Some(crate::agent::AgentEvent::Verifying) => "Verify".to_string(),
                Some(crate::agent::AgentEvent::ToolCall { summary, .. }) => summary
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(30)
                    .collect(),
                _ => "Wait".to_string(),
            };
            format!(
                " {} {} {} ⏳ {:.1}s │ {} │ {} {} {}",
                self.spinner_char,
                mode_str,
                buffer_info,
                elapsed,
                detail,
                skill.emoji,
                skill.name,
                git_info
            )
        } else {
            format!(
                " {} {} │ {} {} │ {} {}",
                mode_str, buffer_info, skill.emoji, skill.name, self.config.server.model, git_info
            )
        };
        queue!(
            stdout,
            terminal::Clear(ClearType::CurrentLine),
            SetBackgroundColor(Color::DarkGrey),
            SetForegroundColor(mode_color),
            SetAttribute(Attribute::Bold),
            Print(&status_text),
        )?;
        let width = self.term_width().saturating_sub(1);
        let text_width = UnicodeWidthStr::width(status_text.as_str());
        let remaining = width.saturating_sub(text_width);
        if remaining > 0 {
            queue!(stdout, Print(" ".repeat(remaining)))?;
        }
        queue!(stdout, style::ResetColor, SetAttribute(Attribute::Reset))?;
        let input_y = self.height - 1;
        let (prompt, editor, cursor_col) = match self.mode {
            Mode::Insert => (">", &self.editor, self.editor.cursor_display_col()),
            Mode::Command => (":", &self.cmd_editor, self.cmd_editor.cursor_display_col()),
            Mode::Search => ("/", &self.cmd_editor, self.cmd_editor.cursor_display_col()),
            Mode::Normal => (" ", &self.editor, 0),
        };
        let input_text = format!("{}{}", prompt, editor.content());
        queue!(
            stdout,
            cursor::MoveTo(0, input_y),
            terminal::Clear(ClearType::CurrentLine),
            SetForegroundColor(Color::White),
            Print(&input_text),
            style::ResetColor
        )?;
        match self.mode {
            Mode::Normal => {
                queue!(stdout, cursor::Hide)?;
            }
            Mode::Insert | Mode::Command | Mode::Search => {
                let col = prompt.chars().count() + cursor_col;
                queue!(stdout, cursor::Show, cursor::MoveTo(col as u16, input_y))?;
            }
        }
        stdout.flush()?;
        Ok(())
    }
    fn render(&mut self, stdout: &mut io::Stdout) -> anyhow::Result<()> {
        let ra_height = self.response_area_height();
        let gutter_w = self.gutter_width();
        let width = self.content_width();
        let vscroll = self.buffer().scroll_offset();
        let visual_rows = self.buffer().visual_rows(width);

        let gutter_statuses = self.get_git_gutter();

        let git_info = if let Some(gutter) = &gutter_statuses {
            let lines_count = gutter.iter().filter(|&&c| c == '+').count();
            let mut hunks = 0;
            let mut in_hunk = false;
            for &c in gutter.iter() {
                if c == '+' && !in_hunk {
                    hunks += 1;
                    in_hunk = true;
                } else if c != '+' {
                    in_hunk = false;
                }
            }
            if lines_count > 0 {
                format!("│ +{} ({}h)", lines_count, hunks)
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        self.cached_git_info = git_info;

        let lines = self.buffer().lines();
        for i in 0..ra_height {
            let vrow_idx = vscroll + i;
            queue!(
                stdout,
                cursor::MoveTo(0, i as u16),
                terminal::Clear(ClearType::CurrentLine)
            )?;
            if vrow_idx < visual_rows.len() {
                let vrow = &visual_rows[vrow_idx];
                let line_num = if vrow.start_col == 0 {
                    format!(
                        "{:>width$} ",
                        vrow.logical_line + 1,
                        width = gutter_w.saturating_sub(2)
                    )
                } else {
                    " ".repeat(gutter_w - 1)
                };
                queue!(
                    stdout,
                    SetForegroundColor(Color::DarkGrey),
                    Print(&line_num),
                    style::ResetColor,
                )?;

                let git_char = if vrow.start_col == 0 {
                    gutter_statuses
                        .as_ref()
                        .and_then(|g| g.get(vrow.logical_line).copied())
                        .unwrap_or(' ')
                } else {
                    ' '
                };
                let git_color = if git_char == '+' {
                    Color::Green
                } else {
                    Color::DarkGrey
                };

                queue!(
                    stdout,
                    SetForegroundColor(git_color),
                    Print(git_char),
                    style::ResetColor,
                )?;

                // Print each segment with its own color
                for (text, style) in &vrow.segments {
                    queue!(
                        stdout,
                        SetForegroundColor(style.fg_color()),
                        if style.is_bold() {
                            SetAttribute(Attribute::Bold)
                        } else {
                            SetAttribute(Attribute::Reset)
                        },
                        Print(text),
                        style::ResetColor,
                        SetAttribute(Attribute::Reset)
                    )?;
                }
            }
        }
        let cursor_line_idx = self.buffer().cursor_line();
        let cursor_col_idx = self.buffer().cursor_col();
        if !self.waiting && matches!(self.mode, Mode::Normal) && !self.popup.active {
            for (i, vrow) in visual_rows.iter().enumerate() {
                if i < vscroll || i >= vscroll + ra_height {
                    continue;
                }
                if vrow.logical_line != cursor_line_idx {
                    continue;
                }
                let in_range = if vrow.start_col == vrow.end_col {
                    cursor_col_idx == 0
                } else {
                    cursor_col_idx >= vrow.start_col && cursor_col_idx < vrow.end_col
                };
                let at_end = cursor_col_idx == vrow.end_col
                    && (i + 1 >= visual_rows.len()
                        || visual_rows[i + 1].logical_line != cursor_line_idx);
                if in_range || at_end {
                    let y_pos = (i - vscroll) as u16;
                    let x_pos = (self.gutter_width() + cursor_col_idx - vrow.start_col) as u16;
                    if in_range && vrow.start_col != vrow.end_col {
                        if let Some(ch) =
                            lines[cursor_line_idx].content().chars().nth(cursor_col_idx)
                        {
                            queue!(
                                stdout,
                                cursor::MoveTo(x_pos, y_pos),
                                SetBackgroundColor(Color::Red),
                                SetForegroundColor(Color::White),
                                SetAttribute(Attribute::Bold),
                                Print(ch.to_string()),
                                style::ResetColor,
                                SetAttribute(Attribute::Reset)
                            )?;
                        }
                    } else {
                        queue!(
                            stdout,
                            cursor::MoveTo(x_pos, y_pos),
                            SetBackgroundColor(Color::Red),
                            Print(" "),
                            style::ResetColor
                        )?;
                    }
                    break;
                }
            }
        }
        self.render_spinner_only(stdout)?;
        if self.popup.active {
            self.popup.render(stdout, self.width, self.height)?;
        }
        stdout.flush()?;
        Ok(())
    }
    fn term_width(&self) -> usize {
        if self.width > 0 {
            self.width as usize
        } else {
            120
        }
    }
    fn get_git_gutter(&self) -> Option<Vec<char>> {
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

        let repo_path = std::path::Path::new(&self.config.tools.project_root);
        let repo = match git2::Repository::discover(repo_path) {
            Ok(r) => r,
            Err(_) => return None,
        };

        let abs_repo_path = match repo_path.canonicalize() {
            Ok(p) => p,
            Err(_) => return None,
        };

        let path = std::path::Path::new(buffer_name);
        let abs_file_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            abs_repo_path.join(path)
        };

        let rel_path = match abs_file_path.strip_prefix(&abs_repo_path) {
            Ok(p) => p,
            Err(_) => return None,
        };

        let content: String = self
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
            while let Ok(line) = patch.line_in_hunk(h, i) {
                if line.origin() == '+' {
                    if let Some(nl) = line.new_lineno() {
                        let idx = (nl - 1) as usize;
                        if idx < gutter.len() {
                            gutter[idx] = '+';
                        }
                    }
                }
                i += 1;
            }
        }
        Some(gutter)
    }
    fn get_git_gutter_lines(&self) -> Option<Vec<usize>> {
        let gutter = self.get_git_gutter()?;
        let lines: Vec<usize> = gutter
            .iter()
            .enumerate()
            .filter_map(|(i, &c)| if c == '+' { Some(i + 1) } else { None })
            .collect();
        Some(lines)
    }
    fn gutter_width(&self) -> usize {
        let lines = self.buffer().len();
        let base = if lines < 10 {
            2
        } else if lines < 100 {
            3
        } else if lines < 1000 {
            4
        } else if lines < 10000 {
            5
        } else {
            lines.to_string().len() + 1
        };
        base + 1 // +1 for git gutter
    }
    fn content_width(&self) -> usize {
        self.term_width().saturating_sub(self.gutter_width() + 1)
    }
    fn get_timestamp() -> String {
        let now = std::time::SystemTime::now();
        let duration = now
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = duration.as_secs();
        let hours = (secs / 3600) % 24;
        let mins = (secs / 60) % 60;
        let secs = secs % 60;
        format!("{:02}:{:02}:{:02}", hours, mins, secs)
    }
    fn show_git_status(
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
        // self.push_line("  [c] Stage tracked and commit with LLM", LineStyle::Info);
        self.push_line(
            "  [s] Toggle staged  [Enter] Open file  [z] stash [q] Close",
            LineStyle::Dim,
        );

        let target_line = if let Some(file) = target_file {
            self.buffer()
                .lines()
                .iter()
                .position(|l| l.content().trim() == file && l.content().starts_with("    "))
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
    fn show_file_picker(&mut self) {
        let root = std::path::PathBuf::from(&self.config.tools.project_root);
        let files = list_project_files(&root);
        let items: Vec<PopupItem> = files
            .iter()
            .map(|f| PopupItem {
                text: f.clone(),
                is_active: false,
            })
            .collect();
        self.popup_mode = PopupMode::FilePicker;
        self.popup.show("Open File", items, 0);
    }
    fn show_task_file_picker(&mut self) {
        let root = std::path::PathBuf::from(&self.config.tools.project_root);
        let impl_dir = root.join(".impl");
        let files = list_impl_files(&root, &impl_dir);
        let items: Vec<PopupItem> = files
            .iter()
            .map(|f| PopupItem {
                text: f.clone(),
                is_active: false,
            })
            .collect();
        self.popup_mode = PopupMode::TaskFilePicker;
        self.popup.show("Task Files (.impl)", items, 0);
    }
    fn load_file_to_buffer(&mut self, path: &str, stdout: &mut io::Stdout) -> anyhow::Result<()> {
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

                // Push "Opened" message to Console, but DON'T switch view away from the file
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
fn list_project_files(root: &std::path::Path) -> Vec<String> {
    let mut files = Vec::new();
    fn walk(dir: &std::path::Path, base: &std::path::Path, files: &mut Vec<String>, depth: usize) {
        if depth > 4 || files.len() > 1000 {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') || name == "target" || name == "node_modules" {
                    continue;
                }
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, base, files, depth + 1);
                } else if let Ok(rel) = path.strip_prefix(base) {
                    files.push(rel.to_string_lossy().to_string());
                }
            }
        }
    }
    walk(root, root, &mut files, 0);
    files.sort();
    files
}
fn highlight_segments(
    line: &str,
    pattern: &str,
    line_style: LineStyle,
    pattern_style: LineStyle,
    prefix: &str,
    prefix_style: LineStyle,
) -> Vec<(String, LineStyle)> {
    let mut segments = vec![(prefix.to_string(), prefix_style)];
    if pattern.is_empty() {
        segments.push((line.to_string(), line_style));
        return segments;
    }
    let mut last_end = 0;
    let mut start = 0;
    while let Some(rel_start) = line[start..].find(pattern) {
        let abs_start = start + rel_start;
        if abs_start > last_end {
            segments.push((line[last_end..abs_start].to_string(), line_style));
        }
        segments.push((
            line[abs_start..abs_start + pattern.len()].to_string(),
            pattern_style,
        ));
        last_end = abs_start + pattern.len();
        start = last_end;
    }
    if last_end < line.len() {
        segments.push((line[last_end..].to_string(), line_style));
    }
    segments
}
fn highlight_search(content: &str, query: &str) -> String {
    let mut result = String::new();
    let mut last_end = 0;
    let q_lower = query.to_lowercase();
    let c_lower = content.to_lowercase();
    while let Some(pos) = c_lower[last_end..].find(&q_lower) {
        let abs_pos = last_end + pos;
        result.push_str(&content[last_end..abs_pos]);
        result.push_str("\x1b[43;30;1m");
        result.push_str(&content[abs_pos..abs_pos + query.len()]);
        result.push_str("\x1b[0m");
        last_end = abs_pos + query.len();
    }
    result.push_str(&content[last_end..]);
    result
}
fn list_impl_files(root: &std::path::Path, impl_dir: &std::path::Path) -> Vec<String> {
    let mut files = Vec::new();
    fn walk(dir: &std::path::Path, root: &std::path::Path, files: &mut Vec<String>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, root, files);
                } else if let Ok(rel) = path.strip_prefix(root) {
                    files.push(rel.to_string_lossy().to_string());
                }
            }
        }
    }
    walk(impl_dir, root, &mut files);
    files.sort();
    files
}
