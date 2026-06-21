// src/repl/editor.rs

use serde_json;

pub struct LineEditor {
    buffer: String,
    cursor_pos: usize,
    history: Vec<String>,
    history_index: Option<usize>,
    saved_buffer: String,
}

impl LineEditor {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            cursor_pos: 0,
            history: Vec::new(),
            history_index: None,
            saved_buffer: String::new(),
        }
    }

    pub fn content(&self) -> &str {
        &self.buffer
    }

    pub fn cursor_display_col(&self) -> usize {
        self.buffer[..self.cursor_pos].chars().count()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn insert_char(&mut self, c: char) {
        self.buffer.insert(self.cursor_pos, c);
        self.cursor_pos += c.len_utf8();
    }

    pub fn backspace(&mut self) {
        if self.cursor_pos > 0 {
            let prev = self.buffer[..self.cursor_pos]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.buffer.drain(prev..self.cursor_pos);
            self.cursor_pos = prev;
        }
    }

    pub fn delete(&mut self) {
        if self.cursor_pos < self.buffer.len() {
            let next = self.buffer[self.cursor_pos..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.cursor_pos + i)
                .unwrap_or(self.buffer.len());
            self.buffer.drain(self.cursor_pos..next);
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos = self.buffer[..self.cursor_pos]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor_pos < self.buffer.len() {
            self.cursor_pos = self.buffer[self.cursor_pos..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.cursor_pos + i)
                .unwrap_or(self.buffer.len());
        }
    }

    pub fn move_home(&mut self) {
        self.cursor_pos = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor_pos = self.buffer.len();
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.cursor_pos = 0;
    }

    pub fn submit(&mut self) -> String {
        let content = std::mem::take(&mut self.buffer);
        self.cursor_pos = 0;

        // Only add to history if it's not empty/whitespace and not a duplicate of the last entry
        if !content.trim().is_empty() {
            if self.history.last().map_or(true, |last| last != &content) {
                self.history.push(content.clone());
            }
        }

        self.history_index = None;
        self.saved_buffer.clear();
        content
    }

    pub fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        if self.history_index.is_none() {
            self.saved_buffer = self.buffer.clone();
        }
        let start_idx = self.history_index.unwrap_or(self.history.len());
        for i in (0..start_idx).rev() {
            if self.history[i].starts_with(&self.saved_buffer) {
                self.history_index = Some(i);
                self.buffer = self.history[i].clone();
                self.cursor_pos = self.buffer.len();
                return;
            }
        }
    }

    pub fn history_down(&mut self) {
        if let Some(idx) = self.history_index {
            let start_idx = idx + 1;
            for i in start_idx..self.history.len() {
                if self.history[i].starts_with(&self.saved_buffer) {
                    self.history_index = Some(i);
                    self.buffer = self.history[i].clone();
                    self.cursor_pos = self.buffer.len();
                    return;
                }
            }
            self.history_index = None;
            self.buffer = self.saved_buffer.clone();
            self.cursor_pos = self.buffer.len();
        }
    }

    pub fn tab_complete(&mut self, candidates: &[&str]) {
        let input = &self.buffer[..self.cursor_pos];
        let matches: Vec<&&str> = candidates.iter().filter(|c| c.starts_with(input)).collect();
        if matches.is_empty() {
            return;
        }
        if matches.len() == 1 {
            let completion = *matches[0];
            self.buffer = completion.to_string();
            self.cursor_pos = self.buffer.len();
            return;
        }
        let first = *matches[0];
        let mut longest_prefix_char_len = first.chars().count();
        for m in &matches[1..] {
            let m_str = **m;
            let common_char_len = first
                .chars()
                .zip(m_str.chars())
                .take_while(|(a, b)| a == b)
                .count();
            longest_prefix_char_len = longest_prefix_char_len.min(common_char_len);
        }
        let input_char_len = input.chars().count();
        if longest_prefix_char_len > input_char_len {
            let byte_idx = first
                .char_indices()
                .nth(longest_prefix_char_len)
                .map(|(i, _)| i)
                .unwrap_or(first.len());
            let new_buffer = first[..byte_idx].to_string();
            self.buffer = new_buffer;
            self.cursor_pos = self.buffer.len();
        }
    }

    pub fn kill_to_end(&mut self) {
        self.buffer.drain(self.cursor_pos..);
    }

    pub fn kill_to_start(&mut self) {
        self.buffer.drain(..self.cursor_pos);
        self.cursor_pos = 0;
    }

    pub fn kill_word_back(&mut self) {
        if self.cursor_pos == 0 {
            return;
        }
        let text = &self.buffer[..self.cursor_pos];
        let trimmed = text.trim_end();
        if trimmed.is_empty() {
            self.buffer.drain(..self.cursor_pos);
            self.cursor_pos = 0;
            return;
        }
        let pos = trimmed
            .rfind(|c: char| c.is_whitespace())
            .map(|p| p + 1)
            .unwrap_or(0);
        self.buffer.drain(pos..self.cursor_pos);
        self.cursor_pos = pos;
    }

    pub fn load_history(&mut self, path: &str) {
        if let Ok(content) = std::fs::read_to_string(path) {
            // Try parsing as JSON array first (new format)
            if let Ok(history) = serde_json::from_str::<Vec<String>>(&content) {
                self.history = history;
            } else {
                // Fallback to old line-based format for backward compatibility
                for line in content.lines() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        self.history.push(trimmed.to_string());
                    }
                }
            }
        }
    }

    pub fn save_history(&self, path: &str) {
        let max = 500;
        let start = self.history.len().saturating_sub(max);

        // Save as JSON array to preserve multi-line inputs
        if let Ok(json) = serde_json::to_string(&self.history[start..]) {
            let _ = std::fs::write(path, json);
        }
    }
}
