// src/repl/mod.rs

pub mod buffer;
pub mod editor;
pub mod handle;
pub mod helper;
pub mod misc;
pub mod mode;

use crate::agent::PatchAgent;
use crate::config::AppConfig;
use buffer::{BufferLine, LineStyle, ResponseBuffer};
use crossterm::{
    cursor, event, execute, queue,
    style::{self, Attribute, Color, Print, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal::{self, ClearType},
};
use editor::LineEditor;
use helper::Popup;
use mode::Mode;
use std::io::{self, Write};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub use misc::COMMAND_LIST;

struct TerminalGuard;
impl TerminalGuard {
    fn init(stdout: &mut io::Stdout) -> anyhow::Result<Self> {
        let _ = terminal::enable_raw_mode();
        let _ = execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide);
        Ok(TerminalGuard)
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
    Buffers,
    GitHunks,
    FunctionList,
    WhichKey,
}

pub(crate) enum CommandResult {
    Continue,
    Quit,
    ClearScreen,
}

#[derive(Clone, Copy)]
pub(crate) enum RepeatAction {
    NextHunk,
    PrevHunk,
    NextFunc,
    PrevFunc,
}

pub struct Repl {
    pub(crate) mode: Mode,
    pub(crate) buffers: Vec<ResponseBuffer>,
    pub(crate) active_buffer: usize,
    pub(crate) llm_buffer_idx: Option<usize>,
    pub(crate) console_buffer_idx: Option<usize>,
    pub(crate) editor: LineEditor,
    pub(crate) cmd_editor: LineEditor,
    pub(crate) agent: Option<PatchAgent>,
    pub(crate) config: AppConfig,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) waiting: bool,
    pub(crate) pending: Option<char>,
    pub(crate) count: Option<usize>,
    pub(crate) popup: Popup,
    pub(crate) popup_mode: PopupMode,
    pub(crate) agent_rx: Option<tokio::sync::oneshot::Receiver<(PatchAgent, String)>>,
    pub(crate) agent_handle: Option<tokio::task::JoinHandle<()>>,
    pub(crate) cached_skill_group: usize,
    pub(crate) thinking_start: Option<std::time::Instant>,
    pub(crate) event_rx: Option<tokio::sync::mpsc::UnboundedReceiver<crate::agent::AgentEvent>>,
    pub(crate) last_event: Option<crate::agent::AgentEvent>,
    pub(crate) cancel_tx: Option<tokio::sync::oneshot::Sender<()>>,
    pub(crate) spinner_char: String,
    pub(crate) search_query: Option<String>,
    pub(crate) search_matches: Vec<usize>,
    pub(crate) search_match_idx: Option<usize>,
    pub(crate) stash_pop_target: Option<String>,
    pub(crate) cached_git_info: String,
    pub(crate) pending_reset_target: Option<String>,
    pub(crate) fkey_help: bool,
    pub(crate) selection_start: Option<(usize, usize)>,
    pub(crate) last_visual_mode: Option<Mode>,
    pub(crate) pending_snippet: Option<String>,
    pub(crate) last_action: Option<RepeatAction>,
    pub(crate) pending_merge: Option<Vec<crate::patch::PatchHunk>>,
    pub(crate) merge_index: usize,
    pub(crate) merge_scroll: usize,
    pub(crate) merge_file_scroll: usize,
    pub(crate) merge_match_idx: usize,
    pub(crate) merge_match_end: usize,
    pub(crate) merge_cursor: usize,
    pub(crate) merge_left_active: bool,
    pub(crate) merge_right_cursor: usize,
    pub(crate) merge_search_query: Option<String>,
    pub(crate) modified_buffers: std::collections::HashSet<String>,
    pub(crate) merge_buffer_apply: bool,
    pub(crate) merge_last_modified: Option<(String, bool)>,
}

const INPUT_AREA_ROWS: usize = 2;

impl Repl {
    pub fn new(agent: PatchAgent, config: AppConfig) -> Self {
        let (width, height) = terminal::size().unwrap_or((80, 24));

        let mut editor = LineEditor::new();
        editor.load_history(&config.repl.history_file);

        let mut cmd_editor = LineEditor::new();
        cmd_editor.load_history(&config.repl.command_history_file);

        let cached_skill_group = agent.active_skill_group;
        Self {
            mode: Mode::Insert,
            buffers: vec![ResponseBuffer::with_name("Chat")],
            active_buffer: 0,
            llm_buffer_idx: Some(0),
            console_buffer_idx: None,
            editor,
            cmd_editor,
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
            pending_reset_target: None,
            cached_git_info: String::new(),
            fkey_help: false,
            selection_start: None,
            last_visual_mode: None,
            pending_snippet: None,
            last_action: None,
            pending_merge: None,
            merge_index: 0,
            merge_scroll: 0,
            merge_file_scroll: 0,
            merge_match_idx: 0,
            merge_match_end: 0,
            merge_cursor: 0,
            merge_left_active: true,
            merge_right_cursor: 0,
            merge_search_query: None,
            modified_buffers: std::collections::HashSet::new(),
            merge_buffer_apply: false,
            merge_last_modified: None,
        }
    }

    pub(crate) fn skill_groups(&self) -> &[crate::agent::SkillGroup] {
        &self.agent_ref().skill_groups
    }

    // ── buffer accessors ──────────────────────────────────────────

    pub(crate) fn buffer(&self) -> &ResponseBuffer {
        &self.buffers[self.active_buffer]
    }

    pub(crate) fn buffer_mut(&mut self) -> &mut ResponseBuffer {
        &mut self.buffers[self.active_buffer]
    }

    pub(crate) fn llm_buffer_idx(&mut self) -> usize {
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

    pub(crate) fn console_buffer_idx(&mut self) -> usize {
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

    // ── push helpers ──────────────────────────────────────────────

    pub(crate) fn push_line(&mut self, content: impl Into<String>, style: LineStyle) {
        self.buffer_mut().push(BufferLine::new(content, style));
    }

    pub(crate) fn push_command_info(&mut self, content: impl Into<String>, style: LineStyle) {
        let idx = self.console_buffer_idx();
        self.buffers[idx].push(BufferLine::new(content, style));
        self.active_buffer = idx;
        let h = self.response_area_height();
        let w = self.content_width();
        self.buffers[idx].scroll_to_bottom(h, w);
    }

    pub(crate) fn push_info(&mut self, content: impl Into<String>, style: LineStyle) {
        let idx = self.console_buffer_idx();
        self.buffers[idx].push(BufferLine::new(content, style));
        if self.active_buffer == idx {
            let h = self.response_area_height();
            let w = self.content_width();
            self.buffers[idx].scroll_to_bottom(h, w);
        }
    }

    pub(crate) fn push_llm_line(&mut self, content: impl Into<String>, style: LineStyle) {
        let idx = self.llm_buffer_idx();
        self.buffers[idx].push(BufferLine::new(content, style));
    }

    // ── scroll helpers ────────────────────────────────────────────

    pub(crate) fn scroll_llm_to_bottom(&mut self) {
        let idx = self.llm_buffer_idx();
        let h = self.response_area_height();
        let w = self.content_width();
        self.buffers[idx].scroll_to_bottom(h, w);
    }

    pub(crate) fn scroll_to_bottom(&mut self) {
        let h = self.response_area_height();
        let w = self.content_width();
        self.buffer_mut().scroll_to_bottom(h, w);
    }

    pub(crate) fn ensure_cursor_visible(&mut self) {
        let h = self.response_area_height();
        let w = self.content_width();
        self.buffer_mut().ensure_cursor_visible(h, w);
    }

    pub(crate) fn center_cursor(&mut self) {
        let h = self.response_area_height();
        let w = self.content_width();
        self.buffer_mut().center_cursor(h, w);
    }

    pub(crate) fn move_bottom(&mut self) {
        let h = self.response_area_height();
        let w = self.content_width();
        self.buffer_mut().move_bottom(h, w);
    }

    pub(crate) fn half_page_down(&mut self) {
        let h = self.response_area_height();
        let w = self.content_width();
        self.buffer_mut().half_page_down(h, w);
    }

    pub(crate) fn half_page_up(&mut self) {
        let h = self.response_area_height();
        let w = self.content_width();
        self.buffer_mut().half_page_up(h, w);
    }

    pub(crate) fn scroll_to_bottom_view(&mut self) {
        let h = self.response_area_height();
        let w = self.content_width();
        self.buffer_mut().scroll_to_bottom_view(h, w);
    }

    pub(crate) fn set_cursor(&mut self, line: usize, col: usize) {
        self.buffer_mut().set_cursor(line, col);
    }

    pub(crate) fn switch_buffer(&mut self, direction: i32) {
        if self.buffers.len() > 1 {
            if direction > 0 {
                self.active_buffer = (self.active_buffer + 1) % self.buffers.len();
            } else if self.active_buffer == 0 {
                self.active_buffer = self.buffers.len() - 1;
            } else {
                self.active_buffer -= 1;
            }
            self.ensure_cursor_visible();
        } else {
            self.push_info("  Only 1 buffer", LineStyle::Dim);
            self.scroll_to_bottom();
        }
    }
    pub(crate) fn close_buffer(&mut self) {
        if self.buffers.len() <= 1 {
            self.buffer_mut().clear();
            self.push_info(
                "  Cannot close last buffer, cleared instead.",
                LineStyle::Dim,
            );
            self.scroll_to_bottom();
            return;
        }
        let closed_idx = self.active_buffer;
        if self.llm_buffer_idx == Some(closed_idx) || self.console_buffer_idx == Some(closed_idx) {
            self.buffers[closed_idx].clear();
            self.push_info(
                "  Cannot close primary buffers (Chat/Console). Cleared instead.",
                LineStyle::Dim,
            );
            self.scroll_to_bottom();
            return;
        }
        let closed_name = self.buffers[closed_idx].name().to_string();
        self.modified_buffers.remove(&closed_name);
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
        if self.console_buffer_idx == Some(self.active_buffer) && self.buffers.len() > 1 {
            self.active_buffer = (self.active_buffer - 1) % self.buffers.len();
        }
        self.ensure_cursor_visible();
    }

    // ── geometry / agent helpers ──────────────────────────────────

    pub(crate) fn response_area_height(&self) -> usize {
        self.height as usize - INPUT_AREA_ROWS
    }

    pub(crate) fn agent_ref(&self) -> &PatchAgent {
        self.agent.as_ref().expect("agent missing")
    }

    pub(crate) fn agent_mut(&mut self) -> &mut PatchAgent {
        self.agent.as_mut().expect("agent missing")
    }

    pub(crate) fn active_skill_group(&self) -> usize {
        if self.agent.is_some() {
            self.agent_ref().active_skill_group
        } else {
            self.cached_skill_group
        }
    }

    pub(crate) fn term_width(&self) -> usize {
        if self.width > 0 {
            self.width as usize
        } else {
            120
        }
    }

    pub(crate) fn gutter_width(&self) -> usize {
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

    pub(crate) fn content_width(&self) -> usize {
        self.term_width().saturating_sub(self.gutter_width() + 1)
    }

    // ── entry point ───────────────────────────────────────────────

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

    // ── rendering ─────────────────────────────────────────────────

    pub(crate) fn render(&mut self, stdout: &mut io::Stdout) -> anyhow::Result<()> {
        if self.mode == Mode::Merge {
            self.render_merge(stdout)?;
            self.render_spinner_only(stdout)?;
            stdout.flush()?;
            return Ok(());
        }
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

                for (text, style) in &vrow.segments {
                    queue!(
                        stdout,
                        if style.is_bold() {
                            SetAttribute(Attribute::Bold)
                        } else {
                            SetAttribute(Attribute::Reset)
                        },
                        SetForegroundColor(style.fg_color()),
                        Print(text),
                        style::ResetColor,
                        SetAttribute(Attribute::Reset)
                    )?;
                }
            }
        }

        let cursor_line_idx = self.buffer().cursor_line();
        let cursor_col_idx = self.buffer().cursor_col();

        if matches!(self.mode, Mode::Normal) && !self.popup.active {
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
                    let mut x_offset = 0;
                    if in_range && vrow.start_col != vrow.end_col {
                        let prefix_graphemes = cursor_col_idx - vrow.start_col;
                        for (g_idx, g) in vrow.content.graphemes(true).enumerate() {
                            if g_idx >= prefix_graphemes {
                                break;
                            }
                            x_offset += UnicodeWidthStr::width(g);
                        }
                    } else {
                        x_offset = UnicodeWidthStr::width(vrow.content.as_str());
                    }
                    let x_pos = (self.gutter_width() + x_offset) as u16;
                    if in_range && vrow.start_col != vrow.end_col {
                        if let Some(g) = vrow
                            .content
                            .graphemes(true)
                            .nth(cursor_col_idx - vrow.start_col)
                        {
                            queue!(
                                stdout,
                                cursor::MoveTo(x_pos, y_pos),
                                SetBackgroundColor(Color::Red),
                                SetForegroundColor(Color::White),
                                SetAttribute(Attribute::Bold),
                                Print(g),
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
        } else if matches!(self.mode, Mode::Visual | Mode::VisualLine) && !self.popup.active {
            if let Some(start) = self.selection_start {
                let end = (cursor_line_idx, cursor_col_idx);
                let (sl, sc) = if start <= end { start } else { end };
                let (el, ec) = if start <= end { end } else { start };
                let is_visual_line = matches!(self.mode, Mode::VisualLine);

                for (i, vrow) in visual_rows.iter().enumerate() {
                    if i < vscroll || i >= vscroll + ra_height {
                        continue;
                    }
                    if vrow.logical_line < sl || vrow.logical_line > el {
                        continue;
                    }

                    let y_pos = (i - vscroll) as u16;
                    let x_pos = self.gutter_width() as u16;
                    queue!(
                        stdout,
                        cursor::MoveTo(x_pos, y_pos),
                        terminal::Clear(ClearType::UntilNewLine)
                    )?;

                    if vrow.segments.is_empty() {
                        let in_hl = vrow.logical_line >= sl && vrow.logical_line <= el;
                        if in_hl {
                            queue!(
                                stdout,
                                SetBackgroundColor(Color::Cyan),
                                Print(" ".repeat(width)),
                                crossterm::style::ResetColor
                            )?;
                        }
                        continue;
                    }

                    let mut current_col = vrow.start_col;
                    for (text, style) in &vrow.segments {
                        for g in text.graphemes(true) {
                            let in_hl = if is_visual_line {
                                true
                            } else {
                                let start_cond = vrow.logical_line > sl
                                    || (vrow.logical_line == sl && current_col >= sc);
                                let end_cond = vrow.logical_line < el
                                    || (vrow.logical_line == el && current_col <= ec);
                                start_cond && end_cond
                            };

                            if in_hl {
                                queue!(
                                    stdout,
                                    if style.is_bold() {
                                        SetAttribute(Attribute::Bold)
                                    } else {
                                        SetAttribute(Attribute::Reset)
                                    },
                                    SetBackgroundColor(Color::Cyan),
                                    SetForegroundColor(Color::Black),
                                    Print(g),
                                    crossterm::style::ResetColor,
                                    SetAttribute(Attribute::Reset)
                                )?;
                            } else {
                                queue!(
                                    stdout,
                                    if style.is_bold() {
                                        SetAttribute(Attribute::Bold)
                                    } else {
                                        SetAttribute(Attribute::Reset)
                                    },
                                    SetForegroundColor(style.fg_color()),
                                    Print(g),
                                    crossterm::style::ResetColor,
                                    SetAttribute(Attribute::Reset)
                                )?;
                            }
                            current_col += 1;
                        }
                    }
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

    pub(crate) fn render_spinner_only(&self, stdout: &mut io::Stdout) -> anyhow::Result<()> {
        let status_y = self.height - 2;
        queue!(stdout, cursor::MoveTo(0, status_y))?;
        let mode_str = if let Some(c) = self.pending {
            format!("{}({})", self.mode.as_str(), c)
        } else {
            self.mode.as_str().to_string()
        };
        let mode_color = self.mode.status_color();
        let skill_str = if let Some(agent) = self.agent.as_ref() {
            let groups = &agent.skill_groups;
            let idx = self
                .active_skill_group()
                .min(groups.len().saturating_sub(1));
            format!("{} {}", groups[idx].emoji, groups[idx].name)
        } else {
            "⚠️ Working...".to_string()
        };
        let buffer_name = self.buffer().name();

        let max_name_len = 20;
        let truncated_name = if UnicodeWidthStr::width(buffer_name) > max_name_len {
            let mut s: String = String::new();
            let mut w = 0;
            for g in buffer_name.graphemes(true) {
                let gw = UnicodeWidthStr::width(g);
                if w + gw + 3 > max_name_len {
                    break;
                }
                s.push_str(g);
                w += gw;
            }
            s.push_str("...");
            s
        } else {
            buffer_name.to_string()
        };
        let modified_indicator = if self.modified_buffers.contains(buffer_name) {
            "[+] "
        } else {
            ""
        };
        let buffer_prefix = format!("[{}/{}] ", self.active_buffer + 1, self.buffers.len());
        let git_info = if self.cached_git_info.is_empty() {
            String::new()
        } else {
            format!(" {} ", self.cached_git_info)
        };

        let mut segments: Vec<(String, Color)> = Vec::new();
        if self.waiting {
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
            segments.push((format!(" {} ", self.spinner_char), Color::Yellow));
            segments.push((mode_str.to_string(), mode_color));
            segments.push((format!(" {} ", buffer_prefix), Color::Cyan));
            if !modified_indicator.is_empty() {
                segments.push((modified_indicator.to_string(), Color::Red));
            }
            segments.push((format!("{} ", truncated_name), Color::Cyan));
            segments.push((format!(" ⏳ {:.1}s", elapsed), Color::Yellow));
            segments.push((" │ ".to_string(), Color::Grey));
            segments.push((detail, Color::Yellow));
            segments.push((" │ ".to_string(), Color::Grey));

            segments.push((skill_str.clone(), Color::Green));
            segments.push((" │ ".to_string(), Color::Grey));
            segments.push((self.config.server.model.clone(), Color::White));
            segments.push((format!("[{}]", self.config.server.api_type), Color::Magenta));
            if !git_info.is_empty() {
                segments.push((git_info, Color::Green));
            }
        } else {
            segments.push((format!(" {} ", mode_str), mode_color));
            segments.push((buffer_prefix.clone(), Color::Cyan));
            if !modified_indicator.is_empty() {
                segments.push((modified_indicator.to_string(), Color::Red));
            }
            segments.push((truncated_name.to_string(), Color::Cyan));
            segments.push((" │ ".to_string(), Color::Grey));
            segments.push((skill_str.clone(), Color::Green));

            segments.push((" │ ".to_string(), Color::Grey));
            segments.push((self.config.server.model.clone(), Color::White));
            segments.push((format!("[{}]", self.config.server.api_type), Color::Magenta));
            segments.push((" │ ".to_string(), Color::Grey));
            segments.push(("[o] EDITOR".to_string(), Color::Cyan));
            if !git_info.is_empty() {
                segments.push((git_info, Color::Green));
            }
        }
        queue!(
            stdout,
            terminal::Clear(ClearType::CurrentLine),
            SetBackgroundColor(Color::DarkGrey),
            SetAttribute(Attribute::Bold),
        )?;
        let mut total_width = 0usize;
        for (text, color) in &segments {
            total_width += UnicodeWidthStr::width(text.as_str());
            queue!(stdout, SetForegroundColor(*color), Print(text.as_str()))?;
        }
        let width = self.term_width().saturating_sub(1);
        let remaining = width.saturating_sub(total_width);
        if remaining > 0 {
            queue!(stdout, Print(" ".repeat(remaining)))?;
        }
        queue!(stdout, style::ResetColor, SetAttribute(Attribute::Reset))?;

        let input_y = self.height - 1;
        let is_addition = self.pending_snippet.is_some();
        let (prompt, editor, cursor_col) = match self.mode {
            Mode::Insert => (
                if is_addition { ">>" } else { ">" },
                &self.editor,
                self.editor.cursor_display_col(),
            ),
            Mode::Command => (":", &self.cmd_editor, self.cmd_editor.cursor_display_col()),
            Mode::Search => ("/", &self.cmd_editor, self.cmd_editor.cursor_display_col()),
            Mode::Normal => (" ", &self.editor, 0),
            Mode::Visual | Mode::VisualLine | Mode::Merge => (" ", &self.editor, 0),
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
            Mode::Normal | Mode::Visual | Mode::VisualLine | Mode::Merge => {
                queue!(stdout, cursor::Hide)?;
            }
            Mode::Insert | Mode::Command | Mode::Search => {
                let col = UnicodeWidthStr::width(prompt) + cursor_col;
                queue!(stdout, cursor::Show, cursor::MoveTo(col as u16, input_y))?;
            }
        }

        if self.fkey_help {
            let term_w = self.term_width() as u16;
            let margin = 2;
            let box_w = term_w.saturating_sub(margin * 2);
            let x = margin;
            let y_bot = self.height.saturating_sub(3);
            let y_l2 = y_bot.saturating_sub(1);
            let y_l1 = y_l2.saturating_sub(1);
            let y_top = y_l1.saturating_sub(1);
            let inner_w = (box_w as usize).saturating_sub(2);

            let pad = |s: &str| -> String {
                let current_w = UnicodeWidthStr::width(s);
                if current_w < inner_w {
                    format!("{}{}", s, " ".repeat(inner_w - current_w))
                } else {
                    s.to_string()
                }
            };
            let pad_item = |s: &str| -> String {
                let target_w = 13;
                let w = UnicodeWidthStr::width(s);
                if w < target_w {
                    format!("{}{}", s, " ".repeat(target_w - w))
                } else {
                    s.to_string()
                }
            };
            let groups: &[crate::agent::SkillGroup] = if let Some(agent) = self.agent.as_ref() {
                &agent.skill_groups
            } else {
                &[]
            };
            let get_skill_str = |key: &str, default_name: &str| -> String {
                if let Some(idx) = groups.iter().position(|g| g.key.as_deref() == Some(key)) {
                    let g = &groups[idx];
                    pad_item(&format!("{}: {}{}", key, g.emoji, g.name))
                } else {
                    pad_item(&format!("{}: {}", key, default_name))
                }
            };

            let line1_str = format!(
                "{}{}{}{}{}{}{}",
                get_skill_str(" F1", "Git"),
                get_skill_str(" F2", "Chat"),
                get_skill_str(" F3", "Full"),
                get_skill_str(" F4", "Hunks"),
                get_skill_str(" F5", "--NA"),
                get_skill_str(" F6", "--NA"),
                pad_item("  *: --NA")
            );
            let line2_str = format!(
                "{}{}{}{}{}{}{}",
                get_skill_str(" F7", "--NA"),
                get_skill_str(" F8", "Func"),
                get_skill_str(" F9", "Merge"),
                get_skill_str("F10", "Skills"),
                get_skill_str("F11", "Prompt"),
                get_skill_str("F12", "Cancel"),
                pad_item("Ins: Paste todo")
            );

            let l1 = pad(&line1_str);
            let l2 = pad(&line2_str);

            queue!(
                stdout,
                SetForegroundColor(Color::Yellow),
                SetAttribute(Attribute::Bold),
                cursor::MoveTo(x, y_top),
                Print(format!("╭{}╮", "─".repeat(inner_w))),
                cursor::MoveTo(x, y_bot),
                Print(format!("╰{}╯", "─".repeat(inner_w))),
                cursor::MoveTo(x, y_l1),
                Print(format!("│{}│", l1)),
                cursor::MoveTo(x, y_l2),
                Print(format!("│{}│", l2)),
                style::ResetColor,
                SetAttribute(Attribute::Reset),
                cursor::Hide
            )?;
        }

        stdout.flush()?;
        Ok(())
    }
}
