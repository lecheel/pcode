//--+ file:///src/tools.rs
use crate::config::AppConfig;
use crate::patch;
use crate::task::Task;
use colored::Colorize;
use reqwest::header::HeaderMap;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

struct SgBackend {
    binary: String,
}

pub type ToolResult = HashMap<String, Value>;

static LAST_CARGO_CHECK: AtomicU64 = AtomicU64::new(0);
static SG_BACKEND: OnceLock<Option<SgBackend>> = OnceLock::new();

const BLOCKED_DIRS: &[&str] = &[
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

fn is_blocked_component(name: &str) -> bool {
    name.starts_with('.') || BLOCKED_DIRS.contains(&name)
}

fn path_contains_blocked_dir(path: &Path) -> bool {
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

pub fn find_codex_eyes(config: &AppConfig) -> Option<String> {
    if let Some(ref bin) = config.tools.codex_eyes_binary {
        if Path::new(bin).exists() {
            return Some(bin.clone());
        }
    }
    if let Ok(output) = std::process::Command::new("which")
        .arg("codex-eyes")
        .output()
    {
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
    let mut deps = Vec::new();
    deps.push(("rg", check_binary("rg")));
    deps.push(("fd", check_binary("fd")));
    deps.push(("sg", check_binary("sg") || check_binary("ast-grep")));
    deps.push(("cargo", check_binary("cargo")));
    deps.push(("git", check_binary("git")));
    deps
}

fn check_binary(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn escape_regex(s: &str) -> String {
    let mut escaped = String::new();
    for c in s.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

async fn daemon_get(config: &AppConfig, endpoint: &str) -> Result<(String, HeaderMap), String> {
    let base_url = config.daemon.base_url.trim_end_matches('/');
    let url = format!("{}{}", base_url, endpoint);

    let client = reqwest::Client::new();
    let mut req = client.get(&url);
    if let Some(repo) = &config.daemon.active_repo {
        req = req.query(&[("repo", repo)]);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("Daemon request failed: {}", e))?;
    let status = resp.status();
    let headers = resp.headers().clone();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("Failed to read daemon response: {}", e))?;

    if !status.is_success() {
        return Err(format!("Daemon error {}: {}", status, text));
    }
    Ok((text, headers))
}

pub fn build_tools(config: &AppConfig) -> Vec<Value> {
    let mut tools = Vec::new();

    // --- Daemon Tools (Read-Only Snapshot) ---
    if !is_disabled(config, "daemon_skeleton") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "daemon_skeleton",
                "description": "Fetch the compressed code skeleton for the active repo from the daemon. Resets LOC counters. Use this to get the big picture overview and find code block hashes. NOTE: This is a pre-indexed snapshot. Do not use to verify recent local edits.",
                "parameters": { "type": "object", "properties": {} }
            }
        }));
    }
    if !is_disabled(config, "daemon_get_hash") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "daemon_get_hash",
                "description": "Fetch the full implementation of a specific code block (struct, impl, function) by its hash from the daemon. Use this to limit context cache usage by only loading necessary code. NOTE: This is a pre-indexed snapshot. Do not use to verify recent local edits.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "hash": { "type": "string", "description": "The hash of the code block (e.g., '56532741a4a7')" }
                    },
                    "required": ["hash"]
                }
            }
        }));
    }
    if !is_disabled(config, "daemon_file_info") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "daemon_file_info",
                "description": "Get metadata and extracted AST body hashes for a specific file from the daemon.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "The file path" }
                    },
                    "required": ["path"]
                }
            }
        }));
    }
    if !is_disabled(config, "daemon_catalog") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "daemon_catalog",
                "description": "List all indexed files in the daemon cache.",
                "parameters": { "type": "object", "properties": {} }
            }
        }));
    }
    if !is_disabled(config, "daemon_loc_info") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "daemon_loc_info",
                "description": "Get the running total of Lines of Code (LOC) fetched in the current daemon session.",
                "parameters": { "type": "object", "properties": {} }
            }
        }));
    }

    if !is_disabled(config, "apply_patch") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "apply_patch",
                "description": "Apply a SEARCH/REPLACE block to a file. Use this for multi-line edits, refactoring, and structural changes. For simple single-line replacements use codex_eyes_sed.\nFormat:\n<<<<<<< SEARCH\n[exact lines of code to find]\n=======\n[lines of code to replace with]\n>>>>>>> REPLACE",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File path to edit (relative to project root)." },
                        "patch": { "type": "string", "description": "The SEARCH/REPLACE block" }
                    },
                    "required": ["path", "patch"]
                }
            }
        }));
    }

    // --- Local Tools ---
    if !is_disabled(config, "codex_eyes_ls") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "codex_eyes_ls",
                "description": "List files and directories within the project. Returns the directory listing with file names, sizes, and types. All paths are sandboxed to the project root.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Directory path to list (relative to project root). Defaults to project root." },
                        "recursive": { "type": "boolean", "description": "Whether to list recursively. Defaults to false." }
                    },
                    "required": []
                }
            }
        }));
    }
    if !is_disabled(config, "codex_eyes_grep") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "codex_eyes_grep",
                "description": "Recursively search for a text pattern across ALL files under a directory using ripgrep. Returns matching lines with file paths and line numbers. Use this to find where a function, struct, variable, or string is used across the project. For single-file search use codex_eyes_search instead.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Directory to search recursively (relative to project root). Defaults to project root." },
                        "pattern": { "type": "string", "description": "Text pattern to search for (substring match)." },
                        "case_insensitive": { "type": "boolean", "description": "Case-insensitive matching. Defaults to false." },
                        "file_pattern": { "type": "string", "description": "Only search files matching this glob pattern (e.g. '*.rs', '*.toml'). Defaults to all files." },
                        "max_results": { "type": "integer", "description": "Maximum number of matching lines to return. Defaults to 50." }
                    },
                    "required": ["pattern"]
                }
            }
        }));
    }
    if !is_disabled(config, "codex_eyes_createFile") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "codex_eyes_createFile",
                "description": "Create a new file, overwrite an existing file, or create a directory. If the path ends with a forward slash (e.g. 'src/new_dir/'), it creates a directory (like mkdir -p). Otherwise, it writes the full content to the file.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path for the file or directory (relative to project root). End with '/' to create a directory." },
                        "content": { "type": "string", "description": "Full content of the file (ignored if creating a directory)." }
                    },
                    "required": ["path"]
                }
            }
        }));
    }
    if !is_disabled(config, "codex_eyes_gitdiff") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "codex_eyes_gitdiff",
                "description": "Show git diff for a file or the entire project. Path must be within the project sandbox. Useful for verifying changes after patching or creating files.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File or directory path to diff (relative to project root). Defaults to all files." },
                        "cached": { "type": "boolean", "description": "Show staged changes only. Defaults to false." },
                        "head": { "type": "boolean", "description": "Show changes compared to HEAD. Defaults to false." },
                        "stat": { "type": "boolean", "description": "Show stat summary only. Defaults to false." }
                    },
                    "required": []
                }
            }
        }));
    }
    if !is_disabled(config, "codex_eyes_search") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "codex_eyes_search",
                "description": "Search for a text pattern in a single file, returning matching lines with line numbers. Use this to find exact line numbers before patching. Works on project files and allowed external paths. Useful for searching ./debug.log for error messages or impl.md for requirements.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File path to search in. Use './debug.log' for crash/compiler errors, 'impl.md' or '/tmp/impl.md' for spec requirements, or any project file." },
                        "pattern": { "type": "string", "description": "Text pattern to search for (case-sensitive substring match)." }
                    },
                    "required": ["path", "pattern"]
                }
            }
        }));
    }
    if !is_disabled(config, "codex_eyes_find") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "codex_eyes_find",
                "description": "Find files by name pattern within the project directory tree using fd. Returns matching file paths. Search is sandboxed to the project root.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Directory to search in (relative to project root). Defaults to project root." },
                        "pattern": { "type": "string", "description": "File name pattern to match (case-insensitive substring match)." }
                    },
                    "required": ["pattern"]
                }
            }
        }));
    }
    if !is_disabled(config, "codex_eyes_find_fuzzy") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "codex_eyes_find_fuzzy",
                "description": "Fuzzy find files by name pattern within the project using fd. More tolerant matching than codex_eyes_find. Search is sandboxed to the project root.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Directory to search in (relative to project root). Defaults to project root." },
                        "pattern": { "type": "string", "description": "Fuzzy pattern to match against file names." }
                    },
                    "required": ["pattern"]
                }
            }
        }));
    }
    if !is_disabled(config, "codex_eyes_read") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "codex_eyes_read",
                "description": "Read the contents of a file. Returns the file content with line numbers. IMPORTANT paths: (1) 'impl.md' or '/tmp/impl.md' — read the implementation spec before coding; (2) './debug.log' — read compiler errors and crash logs when bugfixing; (3) any project file. Paths outside the project root must be in allow_paths.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File path to read. Key files: 'impl.md' or '/tmp/impl.md' (implementation spec), './debug.log' (compiler errors/crash logs), or any project source file." },
                        "start_line": { "type": "integer", "description": "Starting line number (1-based, inclusive). Defaults to 1." },
                        "end_line": { "type": "integer", "description": "Ending line number (inclusive). Defaults to end of file." }
                    },
                    "required": ["path"]
                }
            }
        }));
    }
    if !is_disabled(config, "codex_eyes_cargo_check") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "codex_eyes_cargo_check",
                "description": "Run `cargo check` on a Rust project to verify that code changes compile correctly. This is a safe, read-only verification step — it does NOT execute any project code or run arbitrary commands. If this fails, read ./debug.log for detailed compiler errors. Path must be within the project sandbox.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to the Rust project root directory (relative to project root). Defaults to project root." }
                    },
                    "required": []
                }
            }
        }));
    }
    if !is_disabled(config, "codex_eyes_sed") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "codex_eyes_sed",
                "description": "Simple find-and-replace in a file. Much easier and less error-prone than apply_patch for small changes like renaming a variable, fixing a typo, or updating a string. For complex multi-line edits, use apply_patch instead.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File path to edit (relative to project root)." },
                        "find": { "type": "string", "description": "The exact text to find. Must match exactly (including whitespace and indentation)." },
                        "replace": { "type": "string", "description": "The text to replace it with." },
                        "replace_all": { "type": "boolean", "description": "Replace ALL occurrences (true) or only the FIRST occurrence (false). Defaults to true." }
                    },
                    "required": ["path", "find", "replace"]
                }
            }
        }));
    }
    if !is_disabled(config, "codex_eyes_undo") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "codex_eyes_undo",
                "description": "Revert a file to its last git-committed state using git checkout. Path must be within the project sandbox. Useful for undoing a failed patch.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File path to revert (relative to project root). Must be within the project sandbox." }
                    },
                    "required": ["path"]
                }
            }
        }));
    }
    tools.push(json!({
        "type": "function",
        "function": {
            "name": "ast_grep_search",
            "description": "Search code using AST patterns. Returns matching nodes with file paths and line numbers. Use $NAME for single nodes, $$$ARGS for multiple. Always search before replacing.",
            "parameters": {
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "AST pattern to search for, e.g. 'unwrap()' or '$FN($$$ARGS)'" },
                    "lang": { "type": "string", "description": "Language: rust, python, javascript, typescript, go, java, c, cpp, etc.", "default": "rust" },
                    "path": { "type": "string", "description": "File or directory to search", "default": "." }
                },
                "required": ["pattern"]
            }
        }
    }));
    tools.push(json!({
        "type": "function",
        "function": {
            "name": "task_create",
            "description": "Create a new implementation task with a plan. Creates .impl/task_HHMMSS_NNNN/ directory with plan.md and task.json.",
            "parameters": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Short task title" },
                    "description": { "type": "string", "description": "What this task accomplishes" },
                    "plan": { "type": "string", "description": "Step-by-step implementation plan in markdown" }
                },
                "required": ["title", "plan"]
            }
        }
    }));
    tools.push(json!({
        "type": "function",
        "function": {
            "name": "task_step",
            "description": "Record a step in the current active task. Call after each code modification.",
            "parameters": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "description": "Tool used: sed, ast-grep, create, search, verify" },
                    "target": { "type": "string", "description": "File path or pattern targeted" },
                    "description": { "type": "string", "description": "What was done and why" },
                    "success": { "type": "boolean", "description": "Whether the step succeeded" }
                },
                "required": ["action", "target", "description", "success"]
            }
        }
    }));
    tools.push(json!({
        "type": "function",
        "function": {
            "name": "task_complete",
            "description": "Mark the current active task as completed. Writes final changes.log.",
            "parameters": { "type": "object", "properties": {} }
        }
    }));
    tools.push(json!({
        "type": "function",
        "function": {
            "name": "task_list",
            "description": "List all tasks and their status.",
            "parameters": { "type": "object", "properties": {} }
        }
    }));
    tools.push(json!({
        "type": "function",
        "function": {
            "name": "task_abort",
            "description": "Abort the current active task.",
            "parameters": { "type": "object", "properties": {} }
        }
    }));
    tools.push(json!({
        "type": "function",
        "function": {
            "name": "ast_explain",
            "description": "Show the AST structure at a line or matching a text snippet. Returns node types, byte ranges, and SUGGESTED ast-grep patterns. USE THIS when ast_grep_search returns 0 matches to understand the actual code structure before retrying.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Source file to analyze" },
                    "line": { "type": "integer", "description": "Line number (1-based). Either this or text is required." },
                    "text": { "type": "string", "description": "Text snippet to find AST nodes for. Either this or line is required." },
                    "lang": { "type": "string", "description": "Language: rust, python, javascript, typescript, go, java, c, cpp, etc.", "default": "rust" }
                },
                "required": ["path"]
            }
        }
    }));

    let built_in_names: &[&str] = &[
        "codex_eyes_ls",
        "codex_eyes_grep",
        "codex_eyes_sed",
        "apply_patch",
        "codex_eyes_createFile",
        "codex_eyes_gitdiff",
        "codex_eyes_search",
        "codex_eyes_find",
        "codex_eyes_find_fuzzy",
        "codex_eyes_read",
        "codex_eyes_cargo_check",
        "ast_explain",
        "codex_eyes_undo",
    ];
    for custom in &config.tools.custom {
        if is_disabled(config, &custom.name) {
            continue;
        }
        if built_in_names.contains(&custom.name.as_str()) {
            eprintln!(
                "  ⚠️  Custom tool '{}' shadows built-in — ignoring",
                custom.name.yellow()
            );
            continue;
        }
        if custom.execute.command_template.trim().is_empty() {
            eprintln!(
                "  ⚠️  Custom tool '{}' has no command_template — ignoring",
                custom.name.yellow()
            );
            continue;
        }
        let mut properties = serde_json::Map::new();
        for (param_name, param_def) in &custom.parameters {
            properties.insert(
                param_name.clone(),
                json!({
                    "type": param_def.param_type,
                    "description": param_def.description,
                }),
            );
        }
        tools.push(json!({
            "type": "function",
            "function": {
                "name": custom.name,
                "description": custom.description,
                "parameters": {
                    "type": "object",
                    "properties": properties,
                    "required": custom.required,
                }
            }
        }));
    }
    tools
}

fn is_disabled(config: &AppConfig, name: &str) -> bool {
    config.tools.disabled.get(name).copied().unwrap_or(false)
}

pub async fn execute_tool(
    name: &str,
    args: &Value,
    _bin_path: &Option<String>,
    config: &AppConfig,
) -> ToolResult {
    if is_disabled(config, name) {
        return err_result(&format!("Tool '{}' is disabled in config", name));
    }
    match name {
        "daemon_skeleton" => return execute_daemon_skeleton(config).await,
        "daemon_get_hash" => return execute_daemon_get_hash(args, config).await,
        "daemon_file_info" => return execute_daemon_file_info(args, config).await,
        "daemon_catalog" => return execute_daemon_catalog(config).await,
        "daemon_loc_info" => return execute_daemon_loc_info(config).await,
        "codex_eyes_ls" => return execute_ls(args, config).await,
        "codex_eyes_grep" => return execute_grep(args, config).await,
        "codex_eyes_sed" => return execute_sed(args, config).await,
        "codex_eyes_createFile" => return execute_create_file(args, config).await,
        "codex_eyes_gitdiff" => return execute_gitdiff(args, config).await,
        "codex_eyes_search" => return execute_search(args, config).await,
        "codex_eyes_find" => return execute_find(args, config).await,
        "codex_eyes_find_fuzzy" => return execute_find_fuzzy(args, config).await,
        "codex_eyes_read" => return execute_read(args, config).await,
        "codex_eyes_cargo_check" => return execute_cargo_check(args, config).await,
        "codex_eyes_undo" => return execute_undo(args, config).await,
        "ast_grep_search" => return execute_ast_grep_search(args),
        "ast_grep_replace" => return execute_ast_grep_replace(args),
        "task_create" => return execute_task_create(args),
        "task_step" => return execute_task_step(args),
        "task_complete" => return execute_task_complete(),
        "task_list" => return execute_task_list(),
        "ast_explain" => return execute_ast_explain(args, config).await,
        "task_abort" => return execute_task_abort(),
        _ => {
            if let Some(custom) = config.tools.custom.iter().find(|t| t.name == name) {
                return execute_custom_tool(custom, args, config).await;
            }
            return err_result(&format!("Unknown tool: {}", name));
        }
    }
}

async fn execute_daemon_skeleton(config: &AppConfig) -> ToolResult {
    match daemon_get(config, "/skeleton").await {
        Ok((text, _)) => ok_result(&text),
        Err(e) => err_result(&e),
    }
}

async fn execute_daemon_get_hash(args: &Value, config: &AppConfig) -> ToolResult {
    let hash = match args["hash"].as_str() {
        Some(h) => h,
        None => return err_result("Missing required argument: hash"),
    };
    let endpoint = format!("/{}", hash);
    match daemon_get(config, &endpoint).await {
        Ok((text, _)) => ok_result(&text),
        Err(e) => err_result(&e),
    }
}

async fn execute_daemon_file_info(args: &Value, config: &AppConfig) -> ToolResult {
    let path = match args["path"].as_str() {
        Some(p) => p,
        None => return err_result("Missing required argument: path"),
    };
    let endpoint = format!("/file-info/{}", path);
    match daemon_get(config, &endpoint).await {
        Ok((text, _)) => ok_result(&text),
        Err(e) => err_result(&e),
    }
}

async fn execute_daemon_catalog(config: &AppConfig) -> ToolResult {
    match daemon_get(config, "/catalog").await {
        Ok((text, _)) => ok_result(&text),
        Err(e) => err_result(&e),
    }
}

async fn execute_daemon_loc_info(config: &AppConfig) -> ToolResult {
    match daemon_get(config, "/loc-info").await {
        Ok((text, _)) => ok_result(&text),
        Err(e) => err_result(&e),
    }
}

async fn exec_binary(bin: &str, command: &str, args: &Value) -> ToolResult {
    let bin = bin.to_string();
    let command = command.to_string();
    let args_str = serde_json::to_string(args).unwrap_or_default();
    let result = tokio::task::spawn_blocking(move || {
        std::process::Command::new(&bin)
            .arg(&command)
            .arg(&args_str)
            .output()
    })
    .await;
    match result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if output.status.success() {
                if let Ok(map) = serde_json::from_str::<HashMap<String, Value>>(&stdout) {
                    return map;
                }
                ok_result(&stdout)
            } else {
                HashMap::from([
                    ("success".into(), json!(false)),
                    ("error".into(), json!(stderr.trim())),
                    ("output".into(), json!(stdout)),
                ])
            }
        }
        Ok(Err(e)) => err_result(&format!("Failed to execute codex-eyes: {}", e)),
        Err(e) => err_result(&format!("codex-eyes task panicked: {}", e)),
    }
}

async fn execute_ls(args: &Value, config: &AppConfig) -> ToolResult {
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

async fn execute_sed(args: &Value, config: &AppConfig) -> ToolResult {
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

async fn execute_grep(args: &Value, config: &AppConfig) -> ToolResult {
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

    let mut cmd = std::process::Command::new("rg");
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

async fn execute_create_file(args: &Value, config: &AppConfig) -> ToolResult {
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

async fn execute_gitdiff(args: &Value, config: &AppConfig) -> ToolResult {
    let path = args["path"].as_str().unwrap_or(".");
    let cached = args["cached"].as_bool().unwrap_or(false);
    let head = args["head"].as_bool().unwrap_or(false);
    let stat = args["stat"].as_bool().unwrap_or(false);
    let resolved = match resolve_path(path, config) {
        Ok(p) => p,
        Err(e) => return err_result(&e),
    };
    let mut cmd = std::process::Command::new("git");
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

async fn execute_search(args: &Value, config: &AppConfig) -> ToolResult {
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

async fn execute_find(args: &Value, config: &AppConfig) -> ToolResult {
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
    let mut cmd = std::process::Command::new("fd");
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

async fn execute_find_fuzzy(args: &Value, config: &AppConfig) -> ToolResult {
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

    let mut cmd = std::process::Command::new("fd");
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

async fn execute_read(args: &Value, config: &AppConfig) -> ToolResult {
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

async fn execute_cargo_check(args: &Value, config: &AppConfig) -> ToolResult {
    let path = args["path"].as_str().unwrap_or(".");
    let resolved = match resolve_path(path, config) {
        Ok(p) => p,
        Err(e) => return err_result(&e),
    };
    if !resolved.exists() {
        return err_result(&format!("Path does not exist: {}", path));
    }
    let cargo_toml = resolved.join("Cargo.toml");
    if !cargo_toml.exists() {
        return err_result(&format!(
            "No Cargo.toml found in {}. Not a Rust project root.",
            path
        ));
    }
    let cooldown_secs = config.tools.cargo_check_cooldown_secs;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let last = LAST_CARGO_CHECK.load(Ordering::Relaxed);
    let elapsed = now.saturating_sub(last);
    if elapsed < cooldown_secs {
        let wait_secs = cooldown_secs - elapsed;
        eprintln!("     ⏳ cargo check cooldown: waiting {}s...", wait_secs);
        tokio::time::sleep(std::time::Duration::from_secs(wait_secs)).await;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    LAST_CARGO_CHECK.store(now, Ordering::Relaxed);

    let timeout_secs = config
        .tools
        .custom
        .iter()
        .find(|t| t.name == "codex_eyes_cargo_check")
        .map(|t| t.execute.timeout_secs)
        .unwrap_or(180);

    let resolved_owned = resolved.clone();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        tokio::task::spawn_blocking(move || {
            std::process::Command::new("cargo")
                .arg("check")
                .arg("--color=never")
                .current_dir(&resolved_owned)
                .output()
        }),
    )
    .await;

    match result {
        Ok(Ok(Ok(output))) => {
            let exit_code = output.status.code().unwrap_or(-1);
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let mut map = HashMap::new();
            map.insert("success".into(), json!(exit_code == 0));
            map.insert("exit_code".into(), json!(exit_code));
            map.insert("stdout".into(), json!(stdout));
            map.insert("stderr".into(), json!(stderr));
            if exit_code == 0 {
                map.insert(
                    "output".into(),
                    json!("cargo check passed — no compilation errors"),
                );
            } else {
                map.insert(
                    "output".into(),
                    json!(format!("cargo check failed with exit code {}", exit_code)),
                );
            }
            map
        }
        Ok(Ok(Err(e))) => err_result(&format!("Failed to execute cargo check: {}", e)),
        Ok(Err(e)) => err_result(&format!("cargo check task panicked: {}", e)),
        Err(_) => err_result(&format!("cargo check timed out after {}s", timeout_secs)),
    }
}

async fn execute_undo(args: &Value, config: &AppConfig) -> ToolResult {
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
    let output = match std::process::Command::new("git")
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

async fn execute_custom_tool(
    custom: &crate::config::CustomTool,
    args: &Value,
    _config: &AppConfig,
) -> ToolResult {
    let exec_type = &custom.execute.exec_type;
    let template = &custom.execute.command_template;
    let timeout_secs = custom.execute.timeout_secs;
    if template.trim().is_empty() {
        return err_result(&format!(
            "Custom tool '{}' has no command_template",
            custom.name
        ));
    }
    let mut command = template.clone();
    if let Some(obj) = args.as_object() {
        for (key, val) in obj {
            let placeholder = format!("{{{{{}}}}}", key);
            let val_str = match val.as_str() {
                Some(s) => s.to_string(),
                _ => val.to_string(),
            };
            command = command.replace(&placeholder, &val_str);
        }
    }
    match exec_type.as_str() {
        "shell" => {
            let command_owned = command.clone();
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(timeout_secs),
                tokio::task::spawn_blocking(move || {
                    std::process::Command::new("sh")
                        .arg("-c")
                        .arg(&command_owned)
                        .output()
                }),
            )
            .await;
            match result {
                Ok(Ok(Ok(output))) => {
                    let exit_code = output.status.code().unwrap_or(-1);
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    let mut map = HashMap::new();
                    map.insert("success".into(), json!(exit_code == 0));
                    map.insert("exit_code".into(), json!(exit_code));
                    map.insert("stdout".into(), json!(stdout));
                    map.insert("stderr".into(), json!(stderr));
                    map
                }
                Ok(Ok(Err(e))) => err_result(&format!("Command execution failed: {}", e)),
                Ok(Err(e)) => err_result(&format!("Command task panicked: {}", e)),
                Err(_) => err_result(&format!(
                    "Command timed out after {}s: {}",
                    timeout_secs, command
                )),
            }
        }
        _ => err_result(&format!(
            "Unsupported exec_type '{}' for custom tool '{}'",
            exec_type, custom.name
        )),
    }
}

fn execute_ast_grep_search(args: &Value) -> ToolResult {
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
async fn execute_apply_patch(args: &Value, config: &AppConfig) -> ToolResult {
    let path = match args["path"].as_str() {
        Some(p) => p,
        None => return err_result("Missing required argument: path"),
    };
    let patch_text = match args["patch"].as_str() {
        Some(p) => p,
        None => return err_result("Missing required argument: patch"),
    };
    if patch_text.trim().is_empty() {
        return err_result("Empty patch text");
    }
    let project_root = PathBuf::from(&config.tools.project_root);
    match patch::apply_patch(path, patch_text, &project_root, &config.tools.allow_paths) {
        Ok(msg) => ok_result(&msg),
        Err(e) => err_result(&e),
    }
}
fn execute_ast_grep_replace(args: &Value) -> ToolResult {
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

// src/tools.rs

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

fn execute_task_create(args: &Value) -> HashMap<String, Value> {
    let title = args["title"].as_str().unwrap_or("Untitled");
    let description = args["description"].as_str().unwrap_or("");
    let plan = args["plan"].as_str().unwrap_or("");
    match Task::create(title, description, plan) {
        Ok(task) => {
            let mut result = ok_result("task created");
            result.insert("task_id".into(), json!(task.id));
            result.insert("title".into(), json!(task.title));
            result.insert("path".into(), json!(format!(".impl/{}", task.id)));
            result
        }
        Err(e) => err_result(&format!("Failed to create task: {}", e)),
    }
}

fn execute_task_step(args: &Value) -> HashMap<String, Value> {
    let action = args["action"].as_str().unwrap_or("unknown");
    let target = args["target"].as_str().unwrap_or("");
    let description = args["description"].as_str().unwrap_or("");
    let success = args["success"].as_bool().unwrap_or(true);
    match Task::load_active() {
        Ok(Some(mut task)) => match task.add_step(action, target, description, success) {
            Ok(()) => {
                let mut result = ok_result("step recorded");
                result.insert("step".into(), json!(task.steps.len()));
                result.insert("task_id".into(), json!(task.id));
                result
            }
            Err(e) => err_result(&format!("Failed to record step: {}", e)),
        },
        Ok(None) => err_result("No active task. Create one with task_create first."),
        Err(e) => err_result(&format!("Failed to load task: {}", e)),
    }
}

fn execute_task_complete() -> HashMap<String, Value> {
    match Task::load_active() {
        Ok(Some(mut task)) => match task.complete() {
            Ok(()) => {
                let mut result = ok_result("task completed");
                result.insert("task_id".into(), json!(task.id));
                result.insert("steps".into(), json!(task.steps.len()));
                result
            }
            Err(e) => err_result(&format!("Failed to complete task: {}", e)),
        },
        Ok(None) => err_result("No active task."),
        Err(e) => err_result(&format!("Failed to load task: {}", e)),
    }
}

fn execute_task_list() -> HashMap<String, Value> {
    match Task::list_all() {
        Ok(tasks) => {
            let task_list: Vec<Value> = tasks
                .iter()
                .map(|t| {
                    json!({
                        "id": t.id,
                        "title": t.title,
                        "status": t.status,
                        "steps": t.steps.len(),
                        "created_at": t.created_at
                    })
                })
                .collect();
            let mut result = ok_result("tasks listed");
            result.insert("tasks".into(), json!(task_list));
            result
        }
        Err(e) => err_result(&format!("Failed to list tasks: {}", e)),
    }
}

fn execute_task_abort() -> HashMap<String, Value> {
    match Task::load_active() {
        Ok(Some(mut task)) => match task.abort() {
            Ok(()) => {
                let mut result = ok_result("task aborted");
                result.insert("task_id".into(), json!(task.id));
                result
            }
            Err(e) => err_result(&format!("Failed to abort task: {}", e)),
        },
        Ok(None) => err_result("No active task."),
        Err(e) => err_result(&format!("Failed to load task: {}", e)),
    }
}

fn ok_result(msg: &str) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert("success".into(), json!(true));
    m.insert("output".into(), json!(msg));
    m
}

fn err_result(msg: &str) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert("success".into(), json!(false));
    m.insert("error".into(), json!(msg));
    m
}
