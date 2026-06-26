use crate::config::AppConfig;
use crate::tools::common::{err_result, ok_result, resolve_path, ToolResult};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Command;
use std::sync::OnceLock;

struct SgBackend {
    binary: String,
}

static SG_BACKEND: OnceLock<Option<SgBackend>> = OnceLock::new();

fn detect_sg_backend() -> Option<SgBackend> {
    for bin in &["sg", "ast-grep"] {
        // Try --version first
        if Command::new(bin)
            .arg("--version")
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Some(SgBackend {
                binary: bin.to_string(),
            });
        }
        // Fallback to --help
        if Command::new(bin)
            .arg("--help")
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Some(SgBackend {
                binary: bin.to_string(),
            });
        }
        // Fallback to running with no args (ast-grep prints help and exits 0)
        if Command::new(bin)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Some(SgBackend {
                binary: bin.to_string(),
            });
        }
    }
    None
}

fn sg_backend() -> Option<&'static SgBackend> {
    SG_BACKEND.get_or_init(detect_sg_backend).as_ref()
}

pub fn execute_ast_grep_search(args: &Value) -> ToolResult {
    let pattern = args["pattern"].as_str().unwrap_or("");
    let lang = args["lang"].as_str().unwrap_or("rust");
    let path = args["path"].as_str().unwrap_or(".");
    if pattern.is_empty() {
        return err_result("Pattern cannot be empty");
    }
    let be = match sg_backend() {
        Some(b) => b,
        None => return err_result("ast-grep not found. Install: cargo install ast-grep-cli"),
    };
    let output = Command::new(&be.binary)
        .args(["run", "-p", pattern, "-l", lang, "--json", path])
        .output();
    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !out.status.success() && stdout.trim().is_empty() {
                return err_result(&format!(
                    "ast-grep failed (exit {}): {}",
                    out.status.code().unwrap_or(-1),
                    stderr.trim()
                ));
            }
            parse_sg_json(&stdout, stderr.trim(), "search complete")
        }
        Err(e) => err_result(&format!("Failed to run ast-grep: {}", e)),
    }
}

pub fn execute_ast_grep_replace(args: &Value) -> ToolResult {
    let pattern = args["pattern"].as_str().unwrap_or("");
    let rewrite = args["rewrite"].as_str().unwrap_or("");
    let lang = args["lang"].as_str().unwrap_or("rust");
    let path = args["path"].as_str().unwrap_or(".");
    if pattern.is_empty() {
        return err_result("Pattern cannot be empty");
    }
    if rewrite.is_empty() {
        return err_result("Rewrite pattern cannot be empty");
    }
    let be = match sg_backend() {
        Some(b) => b,
        None => return err_result("ast-grep not found. Install: cargo install ast-grep-cli"),
    };

    // 1. Dry run to count matches
    let search_out = match Command::new(&be.binary)
        .args(["run", "-p", pattern, "-l", lang, "--json", path])
        .output()
    {
        Ok(o) => o,
        Err(e) => return err_result(&format!("ast-grep search failed: {}", e)),
    };

    let search_stdout = String::from_utf8_lossy(&search_out.stdout);
    let search_stderr = String::from_utf8_lossy(&search_out.stderr);
    let search_matches: Vec<Value> = match serde_json::from_str(search_stdout.trim()) {
        Ok(arr) => arr,
        Err(_) => search_stdout
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect(),
    };

    if search_matches.is_empty() {
        let mut result = err_result(
            "Zero matches — pattern does not match any code. \
             Common causes: wrong indentation, extra/missing whitespace, \
             or AST structure differs from text. Try a simpler pattern \
             or use codex_eyes_sed for text replacement.",
        );
        if !search_stderr.trim().is_empty() {
            result.insert("sg_warning".into(), json!(search_stderr.trim()));
        }
        if let Some(sample) = suggest_simpler_pattern(be, pattern, lang, path) {
            result.insert("suggestion".into(), json!(sample));
        }
        result.insert("replacements".into(), json!(0));
        result.insert("dry_run".into(), json!(true));
        return result;
    }

    let first_match = &search_matches[0];
    let match_preview = extract_match_preview(first_match);
    let search_count = search_matches.len();

    // 2. Actual replacement (NO --json flag, otherwise -U fails to write)
    let replace_out = match Command::new(&be.binary)
        .args(["run", "-p", pattern, "-r", rewrite, "-l", lang, "-U", path])
        .output()
    {
        Ok(o) => o,
        Err(e) => return err_result(&format!("ast-grep replace failed: {}", e)),
    };

    let replace_stdout = String::from_utf8_lossy(&replace_out.stdout);
    let replace_stderr = String::from_utf8_lossy(&replace_out.stderr);

    if !replace_out.status.success() {
        return err_result(&format!(
            "ast-grep replace failed (exit {}): {}",
            replace_out.status.code().unwrap_or(-1),
            replace_stderr.trim()
        ));
    }

    // Since we removed --json, ast-grep -U outputs the changed file paths.
    // We can count the lines to see how many files were modified.
    let files_changed = replace_stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();

    let mut result = ok_result(&format!(
        "{} replacement(s) applied across {} file(s)",
        search_count, files_changed
    ));
    result.insert("pre_check_matches".into(), json!(search_count));
    result.insert("replacements".into(), json!(search_count));
    result.insert("files_changed".into(), json!(files_changed));
    result.insert("matched_preview".into(), json!(match_preview));
    result.insert("dry_run".into(), json!(false));

    if !replace_stderr.trim().is_empty() {
        result.insert("warnings".into(), json!(replace_stderr.trim()));
    }

    result
}

fn extract_match_preview(m: &Value) -> String {
    if let Some(text) = m.get("text").and_then(|v| v.as_str()) {
        return text.to_string();
    }
    if let Some(text) = m.get("matched").and_then(|v| v.as_str()) {
        return text.to_string();
    }
    if let Some(text) = m
        .get("node")
        .and_then(|n| n.get("text").and_then(|v| v.as_str()))
    {
        return text.to_string();
    }
    serde_json::to_string(m).unwrap_or_else(|_| "...".to_string())
}

pub async fn execute_ast_explain(args: &Value, config: &AppConfig) -> ToolResult {
    let path = match args["path"].as_str() {
        Some(p) => p,
        None => return err_result("Missing required argument: path"),
    };
    let line = args["line"].as_u64();
    let text = args["text"].as_str();
    let lang = args["lang"].as_str().unwrap_or("rust");

    if line.is_none() && text.is_none() {
        return err_result("Provide --line N or --text 'snippet'");
    }

    let resolved = match resolve_path(path, config) {
        Ok(p) => p,
        Err(e) => return err_result(&e),
    };
    if !resolved.exists() {
        return err_result(&format!("File does not exist: {}", path));
    }

    let content = match std::fs::read_to_string(&resolved) {
        Ok(c) => c,
        Err(e) => return err_result(&format!("Failed to read file: {}", e)),
    };

    let mut snippet = String::new();
    if let Some(t) = text {
        snippet = t.to_string();
    } else if let Some(l) = line {
        let target_line = l as usize;
        if target_line == 0 {
            return err_result("Line must be 1-based and > 0");
        }
        let lines: Vec<&str> = content.lines().collect();
        if target_line > lines.len() {
            return err_result(&format!(
                "Line {} out of bounds (file has {} lines)",
                target_line,
                lines.len()
            ));
        }
        snippet = lines[target_line - 1].to_string();

        // Naive multi-line capture: keep grabbing lines if we have unclosed braces or parens
        let mut end_line = target_line;
        let mut paren_depth = snippet.chars().filter(|&c| c == '(').count() as i32
            - snippet.chars().filter(|&c| c == ')').count() as i32;
        let mut brace_depth = snippet.chars().filter(|&c| c == '{').count() as i32
            - snippet.chars().filter(|&c| c == '}').count() as i32;

        while (paren_depth > 0
            || brace_depth > 0
            || snippet.trim_end().ends_with(',')
            || snippet.trim_end().ends_with("&&"))
            && end_line < lines.len()
        {
            end_line += 1;
            let next_line = lines[end_line - 1];
            snippet.push('\n');
            snippet.push_str(next_line);
            paren_depth += next_line.chars().filter(|&c| c == '(').count() as i32
                - next_line.chars().filter(|&c| c == ')').count() as i32;
            brace_depth += next_line.chars().filter(|&c| c == '{').count() as i32
                - next_line.chars().filter(|&c| c == '}').count() as i32;
        }
    }

    let suggested = suggest_pattern_from_snippet(&snippet);
    let mut result = ok_result("AST explanation");

    // Try to get real AST using ast-grep
    let mut ast_tree = String::new();
    if let Some(be) = sg_backend() {
        let out = Command::new(&be.binary)
            .args(["ast", "-l", lang, &snippet])
            .output();

        if let Ok(o) = out {
            let stdout = String::from_utf8_lossy(&o.stdout).to_string();
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            if !stdout.trim().is_empty() {
                ast_tree = stdout.trim().to_string();
            }
            if !stderr.trim().is_empty() {
                result.insert("warnings".into(), json!(stderr.trim()));
            }
        }
    }

    let node = json!({
        "text": snippet,
        "suggested_pattern": suggested,
        "ast_tree": ast_tree,
    });

    result.insert("nodes".into(), json!([node]));
    result
}

fn suggest_pattern_from_snippet(snippet: &str) -> String {
    let trimmed = snippet.trim();

    if trimmed.starts_with("let ") {
        return "let $VAR = $EXPR".to_string();
    }
    if trimmed.starts_with("if ") {
        return "if $COND { $$$BODY }".to_string();
    }
    if trimmed.starts_with("match ") {
        return "match $VAL { $$$ARMS }".to_string();
    }
    if trimmed.starts_with("for ") {
        return "for $VAR in $ITER { $$$BODY }".to_string();
    }
    if trimmed.starts_with("while ") {
        return "while $COND { $$$BODY }".to_string();
    }

    // Macro call: foo!(...)
    if let Some(idx) = trimmed.find("!(") {
        let name = &trimmed[..idx];
        return format!("{}!($$$ARGS)", name);
    }
    // Method call: foo.bar(...)
    if let Some(dot_idx) = trimmed.find('.') {
        if let Some(paren_idx) = trimmed[dot_idx..].find('(') {
            let obj = &trimmed[..dot_idx];
            let method = &trimmed[dot_idx + 1..dot_idx + paren_idx];
            let obj_pattern = if obj.chars().all(|c| c.is_alphanumeric() || c == '_') {
                "$VAL".to_string()
            } else {
                obj.to_string()
            };
            return format!("{}.{}($$$ARGS)", obj_pattern, method);
        }
    }
    // Function call: foo(...)
    if let Some(idx) = trimmed.find('(') {
        let name = &trimmed[..idx];
        return format!("{}($$$ARGS)", name);
    }

    // Fallback
    "$NODE".to_string()
}

fn suggest_simpler_pattern(
    be: &SgBackend,
    pattern: &str,
    lang: &str,
    path: &str,
) -> Option<String> {
    let simplified = if pattern.contains('(') {
        if let Some(idx) = pattern.find('(') {
            let name_part = &pattern[..idx];
            // Handle method calls properly
            if let Some(dot_idx) = name_part.rfind('.') {
                let obj = &name_part[..dot_idx];
                let method = &name_part[dot_idx + 1..];
                let obj_pattern = if obj.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    "$VAL".to_string()
                } else {
                    obj.to_string()
                };
                format!("{}.{}($$$ARGS)", obj_pattern, method)
            } else {
                format!("{}($$$ARGS)", name_part.trim())
            }
        } else {
            return None;
        }
    } else if pattern.contains('.') {
        if let Some(idx) = pattern.find('.') {
            let obj = &pattern[..idx];
            let obj_pattern = if obj.chars().all(|c| c.is_alphanumeric() || c == '_') {
                "$VAL".to_string()
            } else {
                obj.to_string()
            };
            let rest = &pattern[idx + 1..].trim();
            if rest.contains('(') {
                let method = &rest[..rest.find('(').unwrap()];
                format!("{}.{}($$$ARGS)", obj_pattern, method)
            } else {
                format!("{}.{}", obj_pattern, rest)
            }
        } else {
            return None;
        }
    } else {
        return None;
    };

    if simplified == pattern {
        return None;
    }
    let out = Command::new(&be.binary)
        .args(["run", "-p", &simplified, "-l", lang, "--json", path])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let matches: Vec<Value> = match serde_json::from_str(stdout.trim()) {
        Ok(arr) => arr,
        Err(_) => stdout
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect(),
    };
    if matches.is_empty() {
        return None;
    }
    let preview = extract_match_preview(&matches[0]);
    Some(format!(
        "Simplified pattern '{}' found {} match(es). First match:\n{}",
        simplified,
        matches.len(),
        preview
    ))
}

fn parse_sg_json(stdout: &str, stderr: &str, default_msg: &str) -> ToolResult {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        let mut result = ok_result("no matches");
        result.insert("matches".into(), json!([]));
        result.insert("count".into(), json!(0));
        return result;
    }
    if let Ok(arr) = serde_json::from_str::<Vec<Value>>(trimmed) {
        let count = arr.len();
        let mut result = ok_result(default_msg);
        result.insert("matches".into(), json!(arr));
        result.insert("count".into(), json!(count));
        if !stderr.is_empty() {
            result.insert("warnings".into(), json!(stderr));
        }
        return result;
    }
    if let Ok(obj) = serde_json::from_str::<Value>(trimmed) {
        let count = match &obj {
            Value::Array(a) => a.len(),
            Value::Object(_) => 1,
            _ => 0,
        };
        let mut result = ok_result(default_msg);
        result.insert("matches".into(), obj);
        result.insert("count".into(), json!(count));
        if !stderr.is_empty() {
            result.insert("warnings".into(), json!(stderr));
        }
        return result;
    }
    let lines: Vec<Value> = trimmed
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect();
    if !lines.is_empty() {
        let count = lines.len();
        let mut result = ok_result(default_msg);
        result.insert("matches".into(), json!(lines));
        result.insert("count".into(), json!(count));
        if !stderr.is_empty() {
            result.insert("warnings".into(), json!(stderr));
        }
        return result;
    }
    let mut result = ok_result(default_msg);
    result.insert("output".into(), json!(trimmed));
    result.insert("count".into(), json!(0));
    result
}

#[allow(dead_code)]
fn parse_ast_grep_output(stdout: &str) -> HashMap<String, Value> {
    let mut matches = Vec::new();
    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(4, ':').collect();
        if parts.len() >= 4 {
            matches.push(json!({
                "file": parts[0],
                "line": parts[1],
                "col": parts[2],
                "text": parts[3]
            }));
        } else if !line.is_empty() {
            matches.push(json!({ "text": line }));
        }
    }
    let mut result = ok_result("search complete");
    result.insert("matches".into(), json!(matches));
    result.insert("count".into(), json!(matches.len()));
    result
}

#[allow(dead_code)]
fn parse_ast_replace_output(out: &std::process::Output) -> HashMap<String, Value> {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let success = out.status.success();
    let mut result = if success {
        ok_result("replacement applied")
    } else {
        err_result(&stderr)
    };
    let count = stdout.lines().count();
    result.insert("replacements".into(), json!(count));
    result.insert("output".into(), json!(stdout.to_string()));
    result
}
