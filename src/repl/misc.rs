// src/repl/misc.rs
//! Free-standing helpers and constants extracted from the REPL.

use crate::repl::buffer::{BufferLine, LineStyle};
use unicode_segmentation::UnicodeSegmentation;

pub const COMMAND_LIST: &[&str] = &[
    "quit", "q", "exit", "help", "h", "?", "save", "load", "sessions", "delete", "rm", "reset",
    "config", "tools", "debug", "status", "cls", "clear", "skills", "rg", "grep", "fd", "find",
    "ls", "cancel", "bn", "bp", "bd", "open", "e", "saveas", "write", "workflow", "gs", "sed",
];

/// Current wall-clock time as `HH:MM:SS` (local-ish, derived from UNIX_EPOCH).
pub fn get_timestamp() -> String {
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

/// Recursively list project files (depth-limited, skipping hidden / build dirs).
pub fn list_project_files(root: &std::path::Path) -> Vec<String> {
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

/// List `.impl` task files relative to *root*.
pub fn list_impl_files(root: &std::path::Path, impl_dir: &std::path::Path) -> Vec<String> {
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

/// Wrap ANSI escape codes around every match of *query* in *content*.
/// (Currently unused — retained for potential search highlighting.)
pub fn highlight_search(content: &str, query: &str) -> String {
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

/// Build styled segments for a sed-style diff line, highlighting the
/// matched *pattern* with *pattern_style*.
pub fn highlight_segments(
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

/// Strip matching surrounding `"…"` or `'…'` quotes from *s*.
pub fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].to_string()
    } else if s.len() >= 2 && s.starts_with('\'') && s.ends_with('\'') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Return a new `BufferLine` with characters in `[start_char, end_char)` removed,
/// preserving per-segment styling.
pub fn splice_line(line: &BufferLine, start_char: usize, end_char: usize) -> BufferLine {
    let mut new_segments = Vec::new();
    let mut current_char = 0;
    for (text, style) in &line.segments {
        let mut current_text = String::new();
        for g in text.graphemes(true) {
            if current_char < start_char || current_char >= end_char {
                current_text.push_str(g);
            }
            current_char += 1;
        }
        if !current_text.is_empty() {
            new_segments.push((current_text, *style));
        }
    }
    BufferLine::from_segments(new_segments)
}
