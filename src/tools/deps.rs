use crate::config::AppConfig;
use crate::tools::common::check_binary;
use std::path::Path;
use std::process::Command;

pub fn find_codex_eyes(config: &AppConfig) -> Option<String> {
    if let Some(ref bin) = config.tools.codex_eyes_binary {
        if Path::new(bin).exists() {
            return Some(bin.clone());
        }
    }
    if let Ok(output) = Command::new("which").arg("codex-eyes").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() && Path::new(&path).exists() {
                return Some(path);
            }
        }
    }
    None
}

pub fn check_tool_dependencies() -> Vec<(&'static str, bool)> {
    vec![
        ("rg", check_binary("rg")),
        ("fd", check_binary("fd")),
        ("sg", check_binary("sg") || check_binary("ast-grep")),
        ("cargo", check_binary("cargo")),
        ("git", check_binary("git")),
    ]
}
