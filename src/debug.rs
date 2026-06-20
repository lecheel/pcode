use std::fs::OpenOptions;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

static DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn set_debug(enabled: bool) {
    DEBUG_ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn is_debug() -> bool {
    DEBUG_ENABLED.load(Ordering::Relaxed)
}

fn write_log(line: &str) {
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open("debug.log")
    {
        let _ = writeln!(file, "{}", line);
    }
}

pub fn dbg_msg(msg: impl std::fmt::Display) {
    if is_debug() {
        write_log(&format!("{}", msg));
    }
}

pub fn separator(title: &str) {
    if !is_debug() {
        return;
    }
    let width = 72;
    if title.is_empty() {
        write_log(&"─".repeat(width));
    } else {
        let pad = width - title.len() - 2;
        let left = pad / 2;
        let right = pad - left;
        write_log(&format!(
            "\n{} {} {}",
            "─".repeat(left),
            title,
            "─".repeat(right)
        ));
    }
}

pub fn dbg_json(label: &str, data: &serde_json::Value, max_len: usize) {
    if !is_debug() {
        return;
    }
    let text = serde_json::to_string_pretty(data).unwrap_or_default();
    let display = if text.len() > max_len {
        format!(
            "{}\n... [truncated, {} total chars]",
            &text[..max_len],
            text.len()
        )
    } else {
        text
    };
    write_log(&format!("  {}:", label));
    for line in display.lines() {
        write_log(&format!("    {}", line));
    }
}
