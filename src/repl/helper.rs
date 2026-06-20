use super::*;
use std::io;

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

pub struct PopupItem {
    pub text: String,
    pub is_active: bool,
}

pub struct Popup {
    pub active: bool,
    pub cursor: usize,
    pub title: String,
    pub items: Vec<PopupItem>,
}

impl Popup {
    pub fn new() -> Self {
        Self {
            active: false,
            cursor: 0,
            title: String::new(),
            items: Vec::new(),
        }
    }

    pub fn show(&mut self, title: &str, items: Vec<PopupItem>, initial_cursor: usize) {
        let len = items.len();
        self.active = true;
        self.title = title.to_string();
        self.items = items;
        self.cursor = initial_cursor.min(len.saturating_sub(1));
    }

    pub fn hide(&mut self) {
        self.active = false;
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

    pub fn render(&self, stdout: &mut io::Stdout, width: u16, height: u16) -> anyhow::Result<()> {
        if !self.active {
            return Ok(());
        }

        let num_items = self.items.len();
        if num_items == 0 {
            return Ok(());
        }

        let rendered_lines: Vec<String> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let is_selected = i == self.cursor;
                let marker = if is_selected { ">" } else { " " };
                let active_marker = if item.is_active { "*" } else { " " };
                format!("{} {} {}", marker, active_marker, item.text)
            })
            .collect();

        let term_width = width as usize;
        let preferred_width = (term_width * 90) / 100;

        let max_content_width = rendered_lines
            .iter()
            .map(|l| disp_width(l))
            .max()
            .unwrap_or(20)
            .max(20);

        let inner_width = preferred_width.max(max_content_width).min(term_width);

        let box_width = inner_width + 2;
        let box_height = (num_items + 2) as u16;
        let col = (width.saturating_sub(box_width as u16)) / 2;
        let row = (height.saturating_sub(box_height)) / 2;

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

        for (i, line) in rendered_lines.iter().enumerate() {
            let y = row + 1 + i as u16;
            queue!(stdout, cursor::MoveTo(col, y))?;
            let is_selected = i == self.cursor;

            let padded = pad_to_width(line, inner_width);

            let fg = if is_selected {
                Color::Black
            } else {
                Color::White
            };
            let bg = if is_selected {
                Color::Cyan
            } else {
                Color::DarkGrey
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
        let y = row + 1 + num_items as u16;
        queue!(
            stdout,
            cursor::MoveTo(col, y),
            SetForegroundColor(Color::Cyan),
            SetAttribute(Attribute::Bold),
            Print(&bottom_border),
            style::ResetColor,
            SetAttribute(Attribute::Reset)
        )?;

        Ok(())
    }
}

impl super::Repl {
    pub(super) fn set_skill_group(
        &mut self,
        idx: usize,
        stdout: &mut io::Stdout,
    ) -> anyhow::Result<()> {
        if idx >= SKILL_GROUPS.len() {
            return Ok(());
        }
        self.agent_mut().set_skill_group(idx);
        self.cached_skill_group = idx;
        let group = &SKILL_GROUPS[idx];
        self.push_line(
            format!("  {} {} — {}", group.emoji, group.name, group.description),
            LineStyle::ToolResult,
        );
        self.scroll_to_bottom();
        self.render(stdout)
    }

    pub(super) fn cycle_skill_group(&mut self, stdout: &mut io::Stdout) -> anyhow::Result<()> {
        self.agent_mut().cycle_skill_group();
        self.cached_skill_group = self.agent_ref().active_skill_group;
        let group = &SKILL_GROUPS[self.cached_skill_group];
        self.push_line(
            format!("  {} {} — {}", group.emoji, group.name, group.description),
            LineStyle::ToolResult,
        );
        self.scroll_to_bottom();
        self.render(stdout)
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

    pub(super) fn set_skill_group_by_name(
        &mut self,
        name: &str,
        stdout: &mut io::Stdout,
    ) -> anyhow::Result<()> {
        if let Some(idx) = self.agent_mut().set_skill_group_by_name(name) {
            self.cached_skill_group = idx;
            let group = &SKILL_GROUPS[idx];
            self.push_line(
                format!("  {} {} — {}", group.emoji, group.name, group.description),
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

    pub(super) fn show_skill_group_popup(&mut self) {
        self.popup_mode = super::PopupMode::SkillGroups;
        let active_skill_group = self.active_skill_group();
        let max_name_width = SKILL_GROUPS
            .iter()
            .map(|g| disp_width(&g.name))
            .max()
            .unwrap_or(5);
        let items: Vec<PopupItem> = SKILL_GROUPS
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
                }
            })
            .collect();

        self.popup.show("Skill Groups", items, active_skill_group);
    }
}
