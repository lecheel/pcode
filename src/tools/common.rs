use crate::config::AppConfig;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

pub type ToolResult = HashMap<String, Value>;

pub const BLOCKED_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "__pycache__",
    ".next",
    ".venv",
    "venv",
    "env",
    ".env",
    "dist",
    "build",
    "out",
    ".cargo",
    "vendor",
    ".idea",
    ".vscode",
    ".cache",
];

pub fn is_blocked_component(name: &str) -> bool {
    name.starts_with('.') || BLOCKED_DIRS.contains(&name)
}

pub fn path_contains_blocked_dir(path: &Path) -> bool {
    path.components().any(|c| {
        if let std::path::Component::Normal(os_str) = c {
            if let Some(s) = os_str.to_str() {
                return is_blocked_component(s);
            }
        }
        false
    })
}

pub fn resolve_path(path: &str, config: &AppConfig) -> Result<PathBuf, String> {
    let project_root = PathBuf::from(&config.tools.project_root);
    let canonical_root = project_root
        .canonicalize()
        .map_err(|e| format!("Invalid project root '{}': {}", project_root.display(), e))?;
    let raw = Path::new(path);
    let target = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        canonical_root.join(path)
    };
    if path_contains_blocked_dir(&target) {
        return Err(format!(
            "🚫 Access denied: '{}' is inside a blocked directory (target, .git, node_modules, etc.)",
            path
        ));
    }
    let canonical_target = if target.exists() {
        target
            .canonicalize()
            .map_err(|e| format!("Cannot resolve path '{}': {}", path, e))?
    } else if let Some(parent) = target.parent() {
        if parent.as_os_str().is_empty() {
            canonical_root.join(target.file_name().unwrap_or_default())
        } else if parent.exists() {
            let canonical_parent = parent
                .canonicalize()
                .map_err(|e| format!("Parent directory does not exist for '{}': {}", path, e))?;
            canonical_parent.join(target.file_name().unwrap_or_default())
        } else {
            return Err(format!("Parent directory does not exist for '{}'", path));
        }
    } else {
        return Err(format!("Invalid path: '{}'", path));
    };
    if canonical_target.starts_with(&canonical_root) {
        if path_contains_blocked_dir(&canonical_target) {
            return Err(format!(
                "🚫 Access denied: '{}' is inside a blocked directory",
                path
            ));
        }
        return Ok(canonical_target);
    }
    for allowed in &config.tools.allow_paths {
        let canonical_allowed = match PathBuf::from(allowed).canonicalize() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if canonical_target.starts_with(&canonical_allowed) {
            if path_contains_blocked_dir(&canonical_target) {
                return Err(format!(
                    "🚫 Access denied: '{}' is inside a blocked directory",
                    path
                ));
            }
            return Ok(canonical_target);
        }
    }
    Err(format!(
        "🚫 Access denied: '{}' is outside the project root ({}) and not in allow_paths",
        path,
        canonical_root.display()
    ))
}

pub fn is_disabled(config: &AppConfig, name: &str) -> bool {
    config.tools.disabled.get(name).copied().unwrap_or(false)
}

pub fn check_binary(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn escape_regex(s: &str) -> String {
    let mut escaped = String::new();
    for c in s.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

pub fn ok_result(msg: &str) -> ToolResult {
    let mut m = HashMap::new();
    m.insert("success".into(), json!(true));
    m.insert("output".into(), json!(msg));
    m
}

pub fn err_result(msg: &str) -> ToolResult {
    let mut m = HashMap::new();
    m.insert("success".into(), json!(false));
    m.insert("error".into(), json!(msg));
    m
}
