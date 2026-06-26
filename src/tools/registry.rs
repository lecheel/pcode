use crate::config::AppConfig;
use crate::tools::common::is_disabled;
use colored::Colorize;
use serde_json::{json, Value};

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
