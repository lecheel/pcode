use super::*;
use std::io;

#[derive(Clone, Copy, PartialEq)]
pub enum PopupPosition {
    Center,
    Bottom,
    BottomRight,
}

fn disp_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

fn pad_to_width(s: &str, target_width: usize) -> String {
    let current_w = disp_width(s);
    if current_w >= target_width {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(target_width - current_w))
    }
}

#[derive(Clone)]
pub struct PopupItem {
    pub text: String,
    pub is_active: bool,
    pub id: Option<usize>,
}
pub struct Popup {
    pub active: bool,
    pub cursor: usize,
    pub title: String,
    pub items: Vec<PopupItem>,
    pub all_items: Vec<PopupItem>,
    pub filter: String,
    pub max_height: usize,
    pub position: PopupPosition,
    pub show_filter: bool,
    pub compact: bool,
}
impl Popup {
    pub fn new() -> Self {
        Self {
            active: false,
            cursor: 0,
            title: String::new(),
            items: Vec::new(),
            all_items: Vec::new(),
            filter: String::new(),
            max_height: 15,
            position: PopupPosition::Center,
            show_filter: true,
            compact: false,
        }
    }

    pub fn show(
        &mut self,
        title: &str,
        items: Vec<PopupItem>,
        initial_cursor: usize,
        position: PopupPosition,
    ) {
        let len = items.len();
        self.active = true;
        self.title = title.to_string();
        self.all_items = items.clone();
        self.items = items;
        self.cursor = initial_cursor.min(len.saturating_sub(1));
        self.filter = String::new();
        self.max_height = len.max(1).min(15);
        self.position = position;
        self.show_filter = true;
        self.compact = false;
    }
    pub fn hide(&mut self) {
        self.active = false;
        self.filter.clear();
    }
    pub fn move_up(&mut self) {
        if self.items.is_empty() {
            return;
        }
        if self.cursor > 0 {
            self.cursor -= 1;
        } else {
            self.cursor = self.items.len() - 1;
        }
    }
    pub fn move_down(&mut self) {
        if self.items.is_empty() {
            return;
        }
        if self.cursor < self.items.len() - 1 {
            self.cursor += 1;
        } else {
            self.cursor = 0;
        }
    }
    pub fn update_filter(&mut self, query: &str) {
        self.filter = query.to_string();
        if self.filter.is_empty() {
            self.items = self.all_items.clone();
        } else {
            let q = self.filter.to_lowercase();
            self.items = self
                .all_items
                .iter()
                .filter(|item| item.text.to_lowercase().contains(&q))
                .cloned()
                .collect();
        }
        if self.cursor >= self.items.len() {
            self.cursor = 0;
        }
    }
    pub fn render(&self, stdout: &mut io::Stdout, width: u16, height: u16) -> anyhow::Result<()> {
        if !self.active {
            return Ok(());
        }
        let num_items = self.items.len();
        let visible_items = self.max_height.max(1);
        let scroll_offset = if num_items <= visible_items {
            0
        } else if self.cursor < visible_items {
            0
        } else {
            self.cursor - visible_items + 1
        };
        let rendered_lines: Vec<String> = if num_items == 0 {
            vec![format!("   No matches for '{}'", self.filter)]
        } else {
            self.items
                .iter()
                .map(|item| {
                    let marker = " ";
                    let active_marker = if item.is_active { "*" } else { " " };
                    format!("{} {} {}", marker, active_marker, item.text)
                })
                .collect()
        };
        let term_width = width as usize;
        let preferred_width = (term_width * 90) / 100;
        let max_content_width = rendered_lines
            .iter()
            .map(|l| disp_width(l))
            .max()
            .unwrap_or(20)
            .max(20);
        // CHANGED: support compact mode (no expansion to preferred_width)
        let inner_width = if self.compact {
            // max_content_width.min(term_width)
            max_content_width.max(40).min(term_width)
        } else if self.show_filter {
            let filter_line = format!(" Filter: {} ", self.filter);
            let max_filter_width = disp_width(&filter_line);
            preferred_width
                .max(max_content_width)
                .max(max_filter_width)
                .min(term_width)
        } else {
            preferred_width.max(max_content_width).min(term_width)
        };
        let box_width = inner_width + 2;
        let box_height = (visible_items + if self.show_filter { 3 } else { 2 }) as u16;
        // CHANGED: position BottomRight
        let col = match self.position {
            PopupPosition::Center | PopupPosition::Bottom => {
                (width.saturating_sub(box_width as u16)) / 2
            }
            PopupPosition::BottomRight => width.saturating_sub(box_width as u16).saturating_sub(1),
        };
        let row = match self.position {
            PopupPosition::Center => (height.saturating_sub(box_height)) / 2,
            PopupPosition::Bottom | PopupPosition::BottomRight => {
                height.saturating_sub(box_height).saturating_sub(2)
            }
        };

        let title_disp = disp_width(&self.title);
        let total_pad = inner_width.saturating_sub(title_disp).saturating_sub(2);
        let left_pad = total_pad / 2;
        let right_pad = total_pad - left_pad;
        let title_line = format!(
            " {}{}{} ",
            " ".repeat(left_pad),
            self.title,
            " ".repeat(right_pad)
        );
        let top_border = format!("╭{}╮", title_line);
        queue!(
            stdout,
            cursor::MoveTo(col, row),
            SetForegroundColor(Color::Cyan),
            SetAttribute(Attribute::Bold),
            Print(&top_border),
            style::ResetColor,
            SetAttribute(Attribute::Reset)
        )?;

        // Render filter line
        queue!(stdout, cursor::MoveTo(col, row + 1))?;
        let start_y = if self.show_filter { row + 2 } else { row + 1 };

        if self.show_filter {
            let filter_line = format!(" Filter: {} ", self.filter);
            let padded_filter = pad_to_width(&filter_line, inner_width);
            queue!(
                stdout,
                cursor::MoveTo(col, row + 1),
                SetForegroundColor(Color::Cyan),
                SetAttribute(Attribute::Bold),
                Print("│"),
                SetAttribute(Attribute::Reset),
                SetForegroundColor(Color::Yellow),
                Print(&padded_filter),
                style::ResetColor,
                SetForegroundColor(Color::Cyan),
                SetAttribute(Attribute::Bold),
                Print("│"),
                style::ResetColor,
                SetAttribute(Attribute::Reset)
            )?;
        }
        // Render items
        for i in 0..visible_items {
            let y = start_y + i as u16;
            queue!(stdout, cursor::MoveTo(col, y))?;

            let actual_idx = scroll_offset + i;

            let (fg, bg, padded) = if let Some(item) = self.items.get(actual_idx) {
                let is_selected = actual_idx == self.cursor;
                let marker = if is_selected { ">" } else { " " };
                let active_marker = if item.is_active { "*" } else { " " };
                let text = format!("{} {} {}", marker, active_marker, item.text);
                let padded = pad_to_width(&text, inner_width);
                let (fg, bg) = if is_selected {
                    (Color::Black, Color::Cyan)
                } else {
                    let color = if item.text.starts_with("- ") {
                        Color::Red
                    } else if item.text.starts_with("+ ") {
                        Color::Green
                    } else {
                        Color::White
                    };
                    (color, Color::DarkGrey)
                };
                (fg, bg, padded)
            } else if num_items == 0 && i == 0 {
                let text = format!("   No matches for '{}'", self.filter);
                let padded = pad_to_width(&text, inner_width);
                (Color::White, Color::DarkGrey, padded)
            } else {
                let padded = pad_to_width("", inner_width);
                (Color::White, Color::DarkGrey, padded)
            };

            queue!(
                stdout,
                SetForegroundColor(Color::Cyan),
                SetAttribute(Attribute::Bold),
                Print("│"),
                SetAttribute(Attribute::Reset),
                SetForegroundColor(fg),
                SetBackgroundColor(bg),
                Print(&padded),
                style::ResetColor,
                SetForegroundColor(Color::Cyan),
                SetAttribute(Attribute::Bold),
                Print("│"),
                style::ResetColor,
                SetAttribute(Attribute::Reset)
            )?;
        }

        let bottom_border = format!("╰{}╯", "─".repeat(inner_width));
        let y = start_y + visible_items as u16;
        queue!(
            stdout,
            cursor::MoveTo(col, y),
            SetForegroundColor(Color::Cyan),
            SetAttribute(Attribute::Bold),
            Print(&bottom_border),
            style::ResetColor,
            SetAttribute(Attribute::Reset)
        )?;

        // Position cursor at the end of the filter text
        if self.show_filter {
            let cursor_x = col + 1 + " Filter: ".len() as u16 + disp_width(&self.filter) as u16;
            queue!(stdout, cursor::Show, cursor::MoveTo(cursor_x, row + 1))?;
        } else {
            queue!(stdout, cursor::Hide)?;
        }
        Ok(())
    }
}

impl super::Repl {
    pub(super) fn set_skill_group(
        &mut self,
        idx: usize,
        stdout: &mut io::Stdout,
    ) -> anyhow::Result<()> {
        let groups_len = self.skill_groups().len();
        if idx >= groups_len {
            return Ok(());
        }

        let (emoji, name, description) = {
            let group = &self.skill_groups()[idx];
            (
                group.emoji.clone(),
                group.name.clone(),
                group.description.clone(),
            )
        };

        self.agent_mut().set_skill_group(idx);
        self.cached_skill_group = idx;

        self.push_line(
            format!("  {} {} — {}", emoji, name, description),
            LineStyle::ToolResult,
        );
        self.scroll_to_bottom();
        self.render(stdout)
    }

    pub(super) fn cycle_skill_group(&mut self, stdout: &mut io::Stdout) -> anyhow::Result<()> {
        self.agent_mut().cycle_skill_group();
        self.cached_skill_group = self.agent_ref().active_skill_group;

        let (emoji, name, description) = {
            let group = &self.skill_groups()[self.cached_skill_group];
            (
                group.emoji.clone(),
                group.name.clone(),
                group.description.clone(),
            )
        };

        self.push_line(
            format!("  {} {} — {}", emoji, name, description),
            LineStyle::ToolResult,
        );
        self.scroll_to_bottom();
        self.render(stdout)
    }

    pub(super) fn set_skill_group_by_name(
        &mut self,
        name: &str,
        stdout: &mut io::Stdout,
    ) -> anyhow::Result<()> {
        if let Some(idx) = self.agent_mut().set_skill_group_by_name(name) {
            self.cached_skill_group = idx;

            let (emoji, name, description) = {
                let group = &self.skill_groups()[idx];
                (
                    group.emoji.clone(),
                    group.name.clone(),
                    group.description.clone(),
                )
            };

            self.push_line(
                format!("  {} {} — {}", emoji, name, description),
                LineStyle::ToolResult,
            );
            self.scroll_to_bottom();
            self.render(stdout)?;
        } else {
            self.push_line(
                format!("  ❌ Unknown skill group: {}", name),
                LineStyle::Error,
            );
            self.scroll_to_bottom();
            self.render(stdout)?;
        }
        Ok(())
    }

    pub(super) fn toggle_tools(&mut self, stdout: &mut io::Stdout) -> anyhow::Result<()> {
        let tools_on = self.agent_mut().toggle_skills();
        self.cached_skill_group = self.agent_ref().active_skill_group;
        self.push_line(
            format!("  Tools: {}", if tools_on { "ON" } else { "OFF" }),
            LineStyle::ToolResult,
        );
        self.scroll_to_bottom();
        self.render(stdout)
    }

    pub(super) fn show_which_key_popup(&mut self, pending: char) {
        let items = match pending {
            ']' => vec![PopupItem {
                text: "h → next hunk".to_string(),
                is_active: false,
                id: None,
            }],
            '[' => vec![PopupItem {
                text: "h → prev hunk".to_string(),
                is_active: false,
                id: None,
            }],
            'd' => vec![PopupItem {
                text: "d → delete line".to_string(),
                is_active: false,
                id: None,
            }],
            'g' => vec![PopupItem {
                text: "g → go to top".to_string(),
                is_active: false,
                id: None,
            }],
            'y' => vec![PopupItem {
                text: "y → yank line".to_string(),
                is_active: false,
                id: None,
            }],
            'z' => vec![PopupItem {
                text: "z → center cursor".to_string(),
                is_active: false,
                id: None,
            }],
            _ => return,
        };
        let title = format!("{}", pending);
        self.popup_mode = super::PopupMode::WhichKey;
        self.popup
            .show(&title, items, 0, PopupPosition::BottomRight);
        self.popup.show_filter = false;
        self.popup.compact = true;
    }

    pub(super) fn show_skill_group_popup(&mut self) {
        self.popup_mode = super::PopupMode::SkillGroups;
        let active_skill_group = self.active_skill_group();
        let groups = self.skill_groups();

        let max_name_width = groups
            .iter()
            .map(|g| disp_width(&g.name))
            .max()
            .unwrap_or(5);

        let items: Vec<PopupItem> = groups
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let alias_str = if g.aliases.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", g.aliases.join(", "))
                };
                let padded_name = pad_to_width(&g.name, max_name_width);
                let text = format!(
                    "{} {} — {}{}",
                    g.emoji, padded_name, g.description, alias_str
                );
                PopupItem {
                    text,
                    is_active: i == active_skill_group,
                    id: Some(i),
                }
            })
            .collect();

        self.popup.show(
            "Skill Groups",
            items,
            active_skill_group,
            PopupPosition::Center,
        );
    }
}
