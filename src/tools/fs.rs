use crate::config::AppConfig;
use crate::tools::common::{
    err_result, escape_regex, is_blocked_component, ok_result, path_contains_blocked_dir,
    resolve_path, ToolResult,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

pub async fn execute_ls(args: &Value, config: &AppConfig) -> ToolResult {
    let path = args["path"].as_str().unwrap_or(".");
    let recursive = args["recursive"].as_bool().unwrap_or(false);
    let resolved = match resolve_path(path, config) {
        Ok(p) => p,
        Err(e) => return err_result(&e),
    };
    if !resolved.exists() {
        return err_result(&format!("Path does not exist: {}", path));
    }
    if !resolved.is_dir() {
        return err_result(&format!("Path is not a directory: {}", path));
    }
    let mut entries = Vec::new();
    if recursive {
        if let Err(e) = walk_dir(&resolved, &resolved, &mut entries, config) {
            return err_result(&format!("Error walking directory: {}", e));
        }
    } else {
        match std::fs::read_dir(&resolved) {
            Ok(rd) => {
                for entry in rd.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if is_blocked_component(&name) {
                        continue;
                    }
                    let file_type = if entry.path().is_dir() { "dir" } else { "file" };
                    let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    entries.push(format!("{}/\t{}\t{}B", name, file_type, size));
                }
            }
            Err(e) => return err_result(&format!("Error reading directory: {}", e)),
        }
    }
    if entries.is_empty() {
        ok_result("(empty directory)")
    } else {
        entries.sort();
        ok_result(&entries.join("\n"))
    }
}

fn walk_dir(
    root: &Path,
    dir: &Path,
    entries: &mut Vec<String>,
    config: &AppConfig,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if is_blocked_component(&name) {
            continue;
        }
        let canonical_root = PathBuf::from(&config.tools.project_root)
            .canonicalize()
            .unwrap_or_default();
        if let Ok(canonical_entry) = entry.path().canonicalize() {
            if !canonical_entry.starts_with(&canonical_root) {
                continue;
            }
            if path_contains_blocked_dir(&canonical_entry) {
                continue;
            }
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(&entry.path())
            .to_string_lossy()
            .to_string();
        let file_type = if entry.path().is_dir() { "dir" } else { "file" };
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        entries.push(format!("{}/\t{}\t{}B", rel, file_type, size));
        if entry.path().is_dir() {
            walk_dir(root, &entry.path(), entries, config)?;
        }
    }
    Ok(())
}

pub async fn execute_sed(args: &Value, config: &AppConfig) -> ToolResult {
    let path = match args["path"].as_str() {
        Some(p) => p,
        None => return err_result("Missing required argument: path"),
    };
    let find = match args["find"].as_str() {
        Some(p) => p,
        None => return err_result("Missing required argument: find"),
    };
    let replace = match args["replace"].as_str() {
        Some(p) => p,
        None => return err_result("Missing required argument: replace"),
    };
    let replace_all = args["replace_all"].as_bool().unwrap_or(true);
    if find.is_empty() {
        return err_result("Find pattern cannot be empty");
    }
    let resolved = match resolve_path(path, config) {
        Ok(p) => p,
        Err(e) => return err_result(&e),
    };
    if !resolved.exists() {
        return err_result(&format!("File does not exist: {}", path));
    }
    if resolved.is_dir() {
        return err_result(&format!("Path is a directory, not a file: {}", path));
    }
    let content = match std::fs::read_to_string(&resolved) {
        Ok(c) => c,
        Err(e) => return err_result(&format!("Failed to read file: {}", e)),
    };
    let mut count = 0usize;
    let new_content = if replace_all {
        let mut result = String::with_capacity(content.len());
        let mut last_end = 0;
        while let Some(start) = content[last_end..].find(find) {
            let abs_start = last_end + start;
            result.push_str(&content[last_end..abs_start]);
            result.push_str(replace);
            last_end = abs_start + find.len();
            count += 1;
        }
        if count > 0 {
            result.push_str(&content[last_end..]);
        }
        result
    } else {
        if let Some(start) = content.find(find) {
            let mut result = String::with_capacity(content.len() - find.len() + replace.len());
            result.push_str(&content[..start]);
            result.push_str(replace);
            result.push_str(&content[start + find.len()..]);
            count = 1;
            result
        } else {
            content.clone()
        }
    };
    if count == 0 {
        return err_result(&format!("Find pattern '{}' not found in {}", find, path));
    }
    match std::fs::write(&resolved, &new_content) {
        Ok(()) => {
            let mut map = HashMap::new();
            map.insert("success".into(), json!(true));
            map.insert("replacements".into(), json!(count));
            map.insert(
                "output".into(),
                json!(format!(
                    "Replaced {} occurrence(s) of '{}' in {}",
                    count, find, path
                )),
            );
            map
        }
        Err(e) => err_result(&format!("Failed to write file: {}", e)),
    }
}

pub async fn execute_grep(args: &Value, config: &AppConfig) -> ToolResult {
    let path = args["path"].as_str().unwrap_or(".");
    let pattern = match args["pattern"].as_str() {
        Some(p) => p,
        None => return err_result("Missing required argument: pattern"),
    };
    let case_insensitive = args["case_insensitive"].as_bool().unwrap_or(false);
    let file_pattern = args["file_pattern"].as_str().unwrap_or("");
    let max_results = args["max_results"].as_u64().unwrap_or(50) as usize;

    if pattern.is_empty() {
        return err_result("Pattern cannot be empty");
    }

    let resolved = match resolve_path(path, config) {
        Ok(p) => p,
        Err(e) => return err_result(&e),
    };

    if !resolved.exists() {
        return err_result(&format!("Path does not exist: {}", path));
    }

    let mut cmd = Command::new("rg");
    cmd.arg("--color")
        .arg("never")
        .arg("--heading")
        .arg("-n")
        .arg("-H")
        .arg("-m")
        .arg(max_results.to_string());

    if case_insensitive {
        cmd.arg("-i");
    }
    if !file_pattern.is_empty() {
        cmd.arg("-g").arg(file_pattern);
    }
    cmd.arg(pattern).arg(&resolved);

    match cmd.output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if output.status.success() || !stdout.trim().is_empty() {
                ok_result(&stdout)
            } else {
                if output.status.code() == Some(1) && stderr.trim().is_empty() {
                    ok_result(&format!("No matches found for '{}' in {}", pattern, path))
                } else {
                    err_result(&format!("rg failed: {}", stderr.trim()))
                }
            }
        }
        Err(e) => err_result(&format!(
            "Failed to execute rg. Is it installed? Error: {}",
            e
        )),
    }
}

pub async fn execute_create_file(args: &Value, config: &AppConfig) -> ToolResult {
    let path = match args["path"].as_str() {
        Some(p) => p,
        None => return err_result("Missing required argument: path"),
    };
    if path.ends_with('/') {
        let dir_path = path.trim_end_matches('/');
        if dir_path.is_empty() {
            return err_result("Invalid directory path");
        }
        let resolved = match resolve_path(dir_path, config) {
            Ok(p) => p,
            Err(e) => return err_result(&e),
        };
        return match std::fs::create_dir_all(&resolved) {
            Ok(()) => ok_result(&format!("Created directory: {}/", dir_path)),
            Err(e) => err_result(&format!("Failed to create directory: {}", e)),
        };
    }
    let content = match args["content"].as_str() {
        Some(c) => c,
        None => return err_result("Missing required argument: content"),
    };
    let resolved = match resolve_path(path, config) {
        Ok(p) => p,
        Err(e) => return err_result(&e),
    };
    let file_existed = resolved.exists();
    if let Some(parent) = resolved.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return err_result(&format!("Failed to create parent directories: {}", e));
            }
        }
    }
    match std::fs::write(&resolved, content) {
        Ok(()) => {
            let msg = if file_existed {
                format!("Overwrote file: {} ({} bytes)", path, content.len())
            } else {
                format!("Created file: {} ({} bytes)", path, content.len())
            };
            ok_result(&msg)
        }
        Err(e) => err_result(&format!("Failed to write file: {}", e)),
    }
}

pub async fn execute_gitdiff(args: &Value, config: &AppConfig) -> ToolResult {
    let path = args["path"].as_str().unwrap_or(".");
    let cached = args["cached"].as_bool().unwrap_or(false);
    let head = args["head"].as_bool().unwrap_or(false);
    let stat = args["stat"].as_bool().unwrap_or(false);
    let resolved = match resolve_path(path, config) {
        Ok(p) => p,
        Err(e) => return err_result(&e),
    };
    let mut cmd = Command::new("git");
    cmd.arg("diff");
    if cached {
        cmd.arg("--cached");
    }
    if head {
        cmd.arg("HEAD");
    }
    if stat {
        cmd.arg("--stat");
    }
    cmd.arg("--").arg(&resolved);
    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            return HashMap::from([
                ("has_changes".into(), json!(false)),
                (
                    "error".into(),
                    json!(format!("Failed to run git diff: {}", e)),
                ),
            ]);
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let has_changes = !stdout.trim().is_empty();
    let mut result = HashMap::new();
    result.insert("has_changes".into(), json!(has_changes));
    result.insert("output".into(), json!(stdout));
    if !stderr.trim().is_empty() {
        result.insert("error".into(), json!(stderr.trim()));
    }
    result
}

pub async fn execute_search(args: &Value, config: &AppConfig) -> ToolResult {
    let path = match args["path"].as_str() {
        Some(p) => p,
        None => return err_result("Missing required argument: path"),
    };
    let pattern = match args["pattern"].as_str() {
        Some(p) => p,
        None => return err_result("Missing required argument: pattern"),
    };
    let resolved = match resolve_path(path, config) {
        Ok(p) => p,
        Err(e) => return err_result(&e),
    };
    if !resolved.exists() {
        return err_result(&format!("File does not exist: {}", path));
    }
    if resolved.is_dir() {
        return err_result(&format!(
            "Path is a directory, not a file: {}. Use codex_eyes_grep for directory-wide search.",
            path
        ));
    }
    let content = match std::fs::read_to_string(&resolved) {
        Ok(c) => c,
        Err(e) => return err_result(&format!("Failed to read file: {}", e)),
    };
    let mut matches = Vec::new();
    for (i, line) in content.lines().enumerate() {
        if line.contains(pattern) {
            matches.push(format!("{}: {}", i + 1, line));
        }
    }
    if matches.is_empty() {
        ok_result(&format!("No matches found for '{}' in {}", pattern, path))
    } else {
        ok_result(&format!(
            "Found {} match(es) for '{}' in {}:\n{}",
            matches.len(),
            pattern,
            path,
            matches.join("\n")
        ))
    }
}

pub async fn execute_find(args: &Value, config: &AppConfig) -> ToolResult {
    let path = args["path"].as_str().unwrap_or(".");
    let pattern = match args["pattern"].as_str() {
        Some(p) => p,
        None => return err_result("Missing required argument: pattern"),
    };
    let resolved = match resolve_path(path, config) {
        Ok(p) => p,
        Err(e) => return err_result(&e),
    };
    if !resolved.exists() {
        return err_result(&format!("Path does not exist: {}", path));
    }
    if !resolved.is_dir() {
        return err_result(&format!("Path is not a directory: {}", path));
    }

    let escaped_pattern = escape_regex(pattern);
    let mut cmd = Command::new("fd");
    cmd.arg("--color")
        .arg("never")
        .arg(&escaped_pattern)
        .arg(&resolved);

    match cmd.output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            if output.status.success() || !stdout.trim().is_empty() {
                let count = stdout.lines().filter(|l| !l.is_empty()).count();
                ok_result(&format!(
                    "Found {} file(s) matching '{}':\n{}",
                    count,
                    pattern,
                    stdout.trim()
                ))
            } else {
                ok_result(&format!(
                    "No files matching '{}' found in {}",
                    pattern, path
                ))
            }
        }
        Err(e) => err_result(&format!(
            "Failed to execute fd. Is it installed? Error: {}",
            e
        )),
    }
}

pub async fn execute_find_fuzzy(args: &Value, config: &AppConfig) -> ToolResult {
    let path = args["path"].as_str().unwrap_or(".");
    let pattern = match args["pattern"].as_str() {
        Some(p) => p,
        None => return err_result("Missing required argument: pattern"),
    };
    let resolved = match resolve_path(path, config) {
        Ok(p) => p,
        Err(e) => return err_result(&e),
    };
    if !resolved.exists() {
        return err_result(&format!("Path does not exist: {}", path));
    }
    if !resolved.is_dir() {
        return err_result(&format!("Path is not a directory: {}", path));
    }

    let mut fuzzy_pattern = String::new();
    for c in pattern.chars() {
        fuzzy_pattern.push_str(&escape_regex(&c.to_string()));
        fuzzy_pattern.push_str(".*");
    }

    let mut cmd = Command::new("fd");
    cmd.arg("--color")
        .arg("never")
        .arg(&fuzzy_pattern)
        .arg(&resolved);

    match cmd.output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            if output.status.success() || !stdout.trim().is_empty() {
                let count = stdout.lines().filter(|l| !l.is_empty()).count();
                ok_result(&format!(
                    "Found {} file(s) fuzzy-matching '{}':\n{}",
                    count,
                    pattern,
                    stdout.trim()
                ))
            } else {
                ok_result(&format!(
                    "No files fuzzy-matching '{}' found in {}",
                    pattern, path
                ))
            }
        }
        Err(e) => err_result(&format!(
            "Failed to execute fd. Is it installed? Error: {}",
            e
        )),
    }
}

pub async fn execute_read(args: &Value, config: &AppConfig) -> ToolResult {
    let path = match args["path"].as_str() {
        Some(p) => p,
        None => return err_result("Missing required argument: path"),
    };
    let start_line = args["start_line"].as_u64().unwrap_or(1) as usize;
    let end_line = args["end_line"].as_u64().unwrap_or(0) as usize;
    let resolved = match resolve_path(path, config) {
        Ok(p) => p,
        Err(e) => return err_result(&e),
    };
    if !resolved.exists() {
        return err_result(&format!("File does not exist: {}", path));
    }
    if resolved.is_dir() {
        return err_result(&format!("Path is a directory, not a file: {}", path));
    }
    let content = match std::fs::read_to_string(&resolved) {
        Ok(c) => c,
        Err(e) => return err_result(&format!("Failed to read file: {}", e)),
    };
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();
    let start = start_line.max(1).min(total_lines + 1);
    let end = if end_line == 0 {
        total_lines
    } else {
        end_line.max(start).min(total_lines)
    };
    let numbered: Vec<String> = lines[(start - 1)..end]
        .iter()
        .enumerate()
        .map(|(i, line)| format!("{:>6} | {}", start + i, line))
        .collect();
    let header = format!(
        "File: {} ({} lines total, showing {}-{})",
        path, total_lines, start, end
    );
    let output = format!("{}\n{}", header, numbered.join("\n"));
    let mut result = ok_result(&output);
    result.insert("line_number".into(), json!(start.to_string()));
    result.insert("total_lines".into(), json!(total_lines));
    result
}

pub async fn execute_undo(args: &Value, config: &AppConfig) -> ToolResult {
    let path = match args["path"].as_str() {
        Some(p) => p,
        None => return err_result("Missing required argument: path"),
    };
    let resolved = match resolve_path(path, config) {
        Ok(p) => p,
        Err(e) => return err_result(&e),
    };
    if !resolved.exists() {
        return err_result(&format!("File does not exist: {}", path));
    }
    let output = match Command::new("git")
        .arg("checkout")
        .arg("--")
        .arg(&resolved)
        .output()
    {
        Ok(o) => o,
        Err(e) => return err_result(&format!("Failed to run git checkout: {}", e)),
    };
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        ok_result(&format!(
            "Reverted {} to git state. {}",
            path,
            if stdout.trim().is_empty() {
                String::new()
            } else {
                stdout.trim().to_string()
            }
        ))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        err_result(&format!("git checkout failed: {}", stderr.trim()))
    }
}
