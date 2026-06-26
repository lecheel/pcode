//--+ file:///src/agent.rs
use crate::config::AppConfig;
use crate::llm::LLMClient;
use crate::session::Session;
use crate::tools;
use serde_json::{json, Value};
use std::collections::HashMap;

pub const CHAT_PROMPT: &str =
    "You are a helpful conversational assistant. Respond naturally and concisely.";

pub const SYSTEM_PROMPT: &str = r#"You are an expert code editing assistant with surgical AST-level tools.
## WORKFLOW
### 1. DISCOVER — Understand the codebase efficiently (Pre-edit)
- Use `daemon_skeleton` to fetch the compressed code skeleton. This provides the big picture overview and shows file structures with AST body hashes.
- Use `daemon_get_hash` to fetch the full implementation of specific code blocks (structs, functions) by their hash. This limits context window cache usage by only loading necessary code.
- Use `daemon_file_info` to list all AST body hashes within a specific file.
- ALTERNATIVELY, you can use local tools like `codex_eyes_ls`, `codex_eyes_grep`, or `ast_grep_search` if you need to search the live filesystem directly.
### 2. EDIT — Make surgical changes locally
Choose the RIGHT tool for the job:
**Simple text replacement** → Use `codex_eyes_sed`
- Renaming a variable, fixing a typo, updating a string literal
- Example: `find="old_name"` `replace="new_name"` `path="src/main.rs"`
**Structural/Surgical code edit** → Use `apply_patch`
- Changing function signatures, wrapping expressions, refactoring patterns
- Format:
  filename src/foo.rs
  <<<<<<< SEARCH
  [exact lines of code to find]
  =======
  [lines of code to replace with]
  >>>>>>> REPLACE
- The SEARCH block must match the exact text in the file (including whitespace).
- You can include multiple SEARCH/REPLACE blocks in a single patch.
**Create or overwrite file / Create directory** → Use `codex_eyes_createFile`
- For new files that don't exist yet, OR for completely overwriting an existing file with new content
- To create a directory, end the path with a slash (e.g. `path="src/new_folder/"`)
- Pass the full file content in the `content` field (omit if just creating a directory)
### 2b. PATCH FAILURE RECOVERY — CRITICAL
If `apply_patch` fails, it means the SEARCH block didn't match the file exactly.
- Read the file again using `codex_eyes_read` to see the exact current content.
- Copy the exact text (including whitespace) into the SEARCH block.
- Retry `apply_patch`.
**WHEN TO USE sed INSTEAD OF apply_patch:**
- Renaming a variable, function, or type → sed
- Fixing a typo, string, or comment → sed
- Any change where you can see the exact text in the source → sed
- `apply_patch` is for multi-line edits, refactoring, or structural changes.
### 3. VERIFY — Check your work (Post-edit)
- **CRITICAL**: After modifying files, the daemon's skeleton and hash caches are STALE.
- Do NOT use `daemon_get_hash` or `daemon_skeleton` to verify edited files.
- Instead, use local read tools: `codex_eyes_read`, `codex_eyes_grep`, or `ast_grep_search`.
- Use `codex_eyes_gitdiff` to see what changed.
- Use `codex_eyes_cargo_check` to verify compilation.
- Use `task_step` to record each step with success/failure.
### 4. CLOSE — Complete the task
- Use `task_complete` when done.
- All steps are recorded in `.impl/task_HHMMSS_NNNN/changes.log`.
## CRITICAL RULES
1. **Always create a task first** before making changes.
2. **Use `apply_patch` for structural changes** — it's exact and won't break code structure.
3. **Use `codex_eyes_sed` for simple changes** — it's easier and more reliable for single lines.
4. **If `apply_patch` fails, read the file again** to get the exact text.
5. **Record every step** with `task_step` so there's a full audit trail.
6. **After code changes**, run `codex_eyes_cargo_check` to verify compilation.
7. **Use `codex_eyes_undo`** to revert a file if changes break it.
"#;
pub const AST_SYSTEM_PROMPT: &str = r#"You are an expert code editing assistant specializing in structural changes.
## WORKFLOW
### 1. DISCOVER — Understand the codebase efficiently (Pre-edit)
- Use `daemon_skeleton` to fetch the compressed code skeleton. This provides the big picture overview and shows file structures with AST body hashes.
- Use `daemon_get_hash` to fetch the full implementation of specific code blocks (structs, functions) by their hash. This limits context window cache usage by only loading necessary code.
- Use `daemon_file_info` to list all AST body hashes within a specific file.
- ALTERNATIVELY, you can use local tools like `codex_eyes_ls`, `codex_eyes_grep`, or `ast_grep_search` if you need to search the live filesystem directly.
### 1b. TODO.md WORKFLOW — Parse and execute user-defined tasks
- If the user asks to "do the todo" or similar, use `codex_eyes_read` to read `todo.md` (or `TODO.md`).
- The file may contain a list of tasks. Some tasks might specify patches directly, e.g.:
  `- [ ] Refactor: apply_patch path="src/main.rs" patch="<<<<<<< SEARCH\n...\n=======\n...\n>>>>>>> REPLACE"`
- **CRITICAL**: Before executing these patches, you MUST verify them.
- Extract the `path` and `patch` from the todo item.
- Read the file using `codex_eyes_read` to see the actual code structure.
- Compare the `patch` with the actual file content.
- If the patch looks incorrect (e.g., wrong whitespace), correct it.
- Execute `apply_patch`.
- Mark the task as complete in `todo.md` using `codex_eyes_sed` (change `- [ ]` to `- [x]`).
- Record the step using `task_step`.
### 2. EDIT — Make surgical changes locally
**Structural/Surgical code edit** → Use `apply_patch`
- Changing function signatures, wrapping expressions, refactoring patterns
- Format:
  <<<<<<< SEARCH
  [exact lines of code to find]
  =======
  [lines of code to replace with]
  >>>>>>> REPLACE
- The SEARCH block must match the exact text in the file (including whitespace).
- You can include multiple SEARCH/REPLACE blocks in a single patch.
### 2b. PATCH FAILURE RECOVERY — CRITICAL
If `apply_patch` fails, it means the SEARCH block didn't match the file exactly.
- Read the file again using `codex_eyes_read` to see the exact current content.
- Copy the exact text (including whitespace) into the SEARCH block.
- Retry `apply_patch`.
**WHEN TO USE sed INSTEAD OF apply_patch:**
- Renaming a variable, function, or type → sed
- Fixing a typo, string, or comment → sed
- Any change where you can see the exact text in the source → sed
- `apply_patch` is for multi-line edits, refactoring, or structural changes.
### 3. VERIFY — Check your work (Post-edit)
- **CRITICAL**: After modifying files, the daemon's skeleton and hash caches are STALE.
- Do NOT use `daemon_get_hash` or `daemon_skeleton` to verify edited files.
- Instead, use local read tools: `codex_eyes_read`, `codex_eyes_grep`, or `ast_grep_search`.
- Use `codex_eyes_gitdiff` to see what changed.
- Use `codex_eyes_cargo_check` to verify compilation.
- Use `task_step` to record each step with success/failure.
### 4. CLOSE — Complete the task
- Use `task_complete` when done.
- All steps are recorded in `.impl/task_HHMMSS_NNNN/changes.log`.
## CRITICAL RULES
1. **Always create a task first** before making changes.
2. **Use `apply_patch` for structural changes** — it's exact and won't break code structure.
3. **Use `codex_eyes_sed` for simple changes** — it's easier and more reliable for single lines.
4. **If `apply_patch` fails, read the file again** to get the exact text.
5. **Record every step** with `task_step` so there's a full audit trail.
6. **After code changes**, run `codex_eyes_cargo_check` to verify compilation.
7. **Use `codex_eyes_undo`** to revert a file if changes break it.
"#;

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Thinking {
        round: u32,
        max_rounds: u32,
    },
    Reasoning {
        preview: String,
    },
    RunningTool {
        name: String,
    },
    Verifying,
    ToolCall {
        name: String,
        summary: String,
    },
    ToolResult {
        name: String,
        success: bool,
        summary: String,
    },
    DiffLine {
        line: String,
    },
    Done,
}

#[derive(Debug, Clone)]
pub struct SkillGroup {
    pub name: String,
    pub description: String,
    pub emoji: String,
    pub tools: Vec<String>,
    pub prompt: String,
    pub aliases: Vec<String>,
}

const BUILT_IN_TOOL_NAMES: &[&str] = &[
    "daemon_skeleton",
    "daemon_get_hash",
    "daemon_file_info",
    "daemon_catalog",
    "daemon_loc_info",
    "codex_eyes_ls",
    "codex_eyes_grep",
    "codex_eyes_sed",
    "apply_patch",
    "ast_grep_search",
    "codex_eyes_createFile",
    "codex_eyes_gitdiff",
    "codex_eyes_search",
    "codex_eyes_find",
    "codex_eyes_find_fuzzy",
    "codex_eyes_read",
    "codex_eyes_cargo_check",
    "codex_eyes_undo",
    "task_create",
    "task_step",
    "task_complete",
    "task_list",
    "task_abort",
    "ast_explain",
];

// Replace the static array definition entirely with this function:
pub fn default_skill_groups() -> Vec<SkillGroup> {
    vec![
        SkillGroup {
            name: "Chat".to_string(),
            description: "Conversational mode, no tools".to_string(),
            emoji: "💬".to_string(),
            tools: vec![],
            prompt: CHAT_PROMPT.to_string(),
            aliases: vec![],
        },
        SkillGroup {
            name: "Daemon".to_string(),
            description: "Context daemon: skeleton, hash, catalog".to_string(),
            emoji: "🌐".to_string(),
            tools: vec![
                "daemon_skeleton".to_string(),
                "daemon_get_hash".to_string(),
                "daemon_file_info".to_string(),
                "daemon_catalog".to_string(),
                "daemon_loc_info".to_string(),
            ],
            prompt: SYSTEM_PROMPT.to_string(),
            aliases: vec!["net".to_string()],
        },
        SkillGroup {
            name: "Discover".to_string(),
            description: "Browse, search, AST patterns".to_string(),
            emoji: "🔍".to_string(),
            tools: vec![
                "codex_eyes_ls".to_string(),
                "codex_eyes_grep".to_string(),
                "codex_eyes_search".to_string(),
                "codex_eyes_find".to_string(),
                "codex_eyes_find_fuzzy".to_string(),
                "codex_eyes_read".to_string(),
                "ast_explain".to_string(),
                "ast_grep_search".to_string(),
            ],
            prompt: SYSTEM_PROMPT.to_string(),
            aliases: vec!["eyes".to_string()],
        },
        SkillGroup {
            name: "AST".to_string(),
            description: "AST-driven edits: explain, search, patch".to_string(),
            emoji: "🌲".to_string(),
            tools: vec![
                "ast_explain".to_string(),
                "ast_grep_search".to_string(),
                "apply_patch".to_string(),
                "codex_eyes_read".to_string(),
                "codex_eyes_search".to_string(),
                "codex_eyes_sed".to_string(),
                "task_create".to_string(),
                "task_step".to_string(),
                "task_complete".to_string(),
            ],
            prompt: AST_SYSTEM_PROMPT.to_string(),
            aliases: vec!["ast".to_string()],
        },
        SkillGroup {
            name: "Edit".to_string(),
            description: "Surgical edits: sed + patch + create".to_string(),
            emoji: "✏️".to_string(),
            tools: vec![
                "codex_eyes_sed".to_string(),
                "apply_patch".to_string(),
                "codex_eyes_createFile".to_string(),
                "codex_eyes_undo".to_string(),
                "codex_eyes_read".to_string(),
                "codex_eyes_search".to_string(),
                "ast_grep_search".to_string(),
                "task_create".to_string(),
                "task_step".to_string(),
                "task_complete".to_string(),
            ],
            prompt: SYSTEM_PROMPT.to_string(),
            aliases: vec!["hand".to_string()],
        },
        SkillGroup {
            name: "Verify".to_string(),
            description: "Check diffs, compilation, tasks".to_string(),
            emoji: "✅".to_string(),
            tools: vec![
                "codex_eyes_gitdiff".to_string(),
                "codex_eyes_cargo_check".to_string(),
                "codex_eyes_read".to_string(),
                "codex_eyes_search".to_string(),
                "task_list".to_string(),
            ],
            prompt: SYSTEM_PROMPT.to_string(),
            aliases: vec![],
        },
        SkillGroup {
            name: "Code".to_string(),
            description: "Discover + Edit (no verify)".to_string(),
            emoji: "🛠️".to_string(),
            tools: vec![
                "codex_eyes_ls".to_string(),
                "codex_eyes_grep".to_string(),
                "codex_eyes_search".to_string(),
                "codex_eyes_find".to_string(),
                "codex_eyes_find_fuzzy".to_string(),
                "codex_eyes_read".to_string(),
                "codex_eyes_sed".to_string(),
                "ast_grep_search".to_string(),
                "apply_patch".to_string(),
                "codex_eyes_createFile".to_string(),
                "codex_eyes_undo".to_string(),
                "task_create".to_string(),
                "task_step".to_string(),
                "task_complete".to_string(),
            ],
            prompt: SYSTEM_PROMPT.to_string(),
            aliases: vec![],
        },
        SkillGroup {
            name: "Full".to_string(),
            description: "All tools (default)".to_string(),
            emoji: "🚀".to_string(),
            tools: vec![
                "daemon_skeleton".to_string(),
                "daemon_get_hash".to_string(),
                "daemon_file_info".to_string(),
                "daemon_catalog".to_string(),
                "daemon_loc_info".to_string(),
                "codex_eyes_ls".to_string(),
                "codex_eyes_grep".to_string(),
                "codex_eyes_sed".to_string(),
                "ast_grep_search".to_string(),
                "apply_patch".to_string(),
                "codex_eyes_createFile".to_string(),
                "codex_eyes_gitdiff".to_string(),
                "codex_eyes_search".to_string(),
                "codex_eyes_find".to_string(),
                "codex_eyes_find_fuzzy".to_string(),
                "codex_eyes_read".to_string(),
                "codex_eyes_cargo_check".to_string(),
                "codex_eyes_undo".to_string(),
                "ast_explain".to_string(),
                "task_create".to_string(),
                "task_step".to_string(),
                "task_complete".to_string(),
                "task_list".to_string(),
                "task_abort".to_string(),
            ],
            prompt: SYSTEM_PROMPT.to_string(),
            aliases: vec!["all".to_string()],
        },
    ]
}

pub struct PatchAgent {
    client: LLMClient,
    bin_path: Option<String>,
    config: AppConfig,
    tools: Vec<Value>,
    pub session: Session,
    pub active_skill_group: usize,
    pub skill_groups: Vec<SkillGroup>,
    event_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
    pub system_prompt_override: Option<String>,
}

impl PatchAgent {
    pub fn new(client: LLMClient, bin_path: Option<String>, config: AppConfig) -> Self {
        let tools = tools::build_tools(&config);

        let override_path = std::path::Path::new(&config.tools.project_root).join("CODER.md");
        let system_prompt_override = if override_path.exists() {
            std::fs::read_to_string(&override_path).ok()
        } else {
            None
        };

        // Load skill groups from config, falling back to defaults
        let skill_groups = if config.repl.skills.is_empty() {
            default_skill_groups()
        } else {
            config
                .repl
                .skills
                .iter()
                .map(|s| SkillGroup {
                    name: s.name.clone(),
                    description: s.description.clone(),
                    emoji: s.emoji.clone(),
                    tools: s.tools.clone(),
                    prompt: s.prompt.clone(),
                    aliases: s.aliases.clone(),
                })
                .collect()
        };

        let idx = 0;
        let base_prompt = &skill_groups[idx].prompt;
        let initial_prompt = if idx != 0 {
            if let Some(over) = &system_prompt_override {
                over.clone()
            } else {
                base_prompt.to_string()
            }
        } else {
            base_prompt.to_string()
        };
        let session = Session::new(&initial_prompt);

        Self {
            client,
            bin_path,
            config,
            tools,
            session,
            active_skill_group: 0,
            skill_groups,
            event_tx: None,
            system_prompt_override,
        }
    }

    pub fn set_event_channel(&mut self, tx: tokio::sync::mpsc::UnboundedSender<AgentEvent>) {
        self.event_tx = Some(tx);
    }

    fn send_event(&self, event: AgentEvent) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(event);
        }
    }

    pub fn set_skill_group(&mut self, index: usize) -> usize {
        if self.skill_groups.is_empty() {
            return 0;
        }
        let idx = index.min(self.skill_groups.len() - 1);
        self.active_skill_group = idx;
        if let Some(msg) = self.session.messages.first_mut() {
            msg["content"] = json!(self.skill_groups[idx].prompt);
        }
        if idx == 0 && self.session.messages.len() > 1 {
            let system_msg = self.session.messages[0].clone();
            self.session.messages.clear();
            self.session.messages.push(system_msg);
        }
        idx
    }

    pub fn set_skill_group_by_name(&mut self, name: &str) -> Option<usize> {
        let name_lower = name.to_lowercase();
        let idx = self.skill_groups.iter().position(|g| {
            g.name.to_lowercase() == name_lower
                || g.aliases.iter().any(|a| a.to_lowercase() == name_lower)
        })?;
        self.set_skill_group(idx);
        Some(idx)
    }

    pub fn cycle_skill_group(&mut self) -> usize {
        if self.skill_groups.is_empty() {
            return 0;
        }
        let next = (self.active_skill_group + 1) % self.skill_groups.len();
        self.set_skill_group(next)
    }

    pub fn toggle_skills(&mut self) -> bool {
        if self.skill_groups.is_empty() {
            return false;
        }
        if self.active_skill_group == 0 {
            self.set_skill_group(self.skill_groups.len() - 1); // Switch to last group (usually Full)
        } else {
            self.set_skill_group(0); // Switch to Chat
        }
        self.active_skill_group != 0
    }

    pub fn active_tools(&self) -> Vec<Value> {
        if self.skill_groups.is_empty() {
            return vec![];
        }
        let group = &self.skill_groups[self.active_skill_group];
        if group.tools.is_empty() {
            return vec![];
        }
        self.tools
            .iter()
            .filter(|t| {
                let name = t["function"]["name"].as_str().unwrap_or("");
                group.tools.iter().any(|tn| tn == name) || !BUILT_IN_TOOL_NAMES.contains(&name)
            })
            .cloned()
            .collect()
    }

    pub async fn run_cycle(
        &mut self,
        user_input: &str,
        mut cancel_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> String {
        self.session.messages.push(json!({
            "role": "user",
            "content": user_input,
        }));

        let max_rounds = self.config.repl.max_rounds;
        for round_num in 0..max_rounds {
            self.send_event(AgentEvent::Thinking {
                round: round_num + 1,
                max_rounds,
            });
            let active_tools = self.active_tools();
            let tools_ref: &[Value] = &active_tools;
            let response = tokio::select! {
                r = self.client.chat(&self.session.messages, tools_ref) => match r {
                    Ok(r) => r,
                    Err(e) => return format!("❌ LLM Error: {}", e),
                },
                _ = &mut cancel_rx => {
                    self.session.messages.push(json!({
                        "role": "assistant",
                        "content": "⛔ Cancelled by user."
                    }));
                    self.send_event(AgentEvent::Done);
                    return "⛔ Task cancelled by user.".to_string();
                }
            };

            let choice = &response["choices"][0];
            let message = choice["message"].clone();

            if let Some(reasoning) = message.get("reasoning_content").and_then(|v| v.as_str()) {
                if !reasoning.is_empty() {
                    let preview: String = reasoning.chars().take(200).collect();
                    self.send_event(AgentEvent::Reasoning {
                        preview: format!("💭 {}", preview),
                    });
                }
            }

            let mut clean_message = message.clone();
            if let Some(obj) = clean_message.as_object_mut() {
                obj.remove("reasoning_content");
            }
            self.session.messages.push(clean_message);

            if let Some(tool_calls) = message.get("tool_calls").and_then(|v| v.as_array()) {
                for tc in tool_calls {
                    let func = &tc["function"];
                    let func_name = func["name"].as_str().unwrap_or("unknown");
                    let func_args = if let Some(args_str) = func["arguments"].as_str() {
                        serde_json::from_str::<Value>(args_str).unwrap_or(json!({}))
                    } else {
                        func["arguments"].clone()
                    };

                    let short = self.short_tool_name(func_name);
                    self.send_event(AgentEvent::RunningTool {
                        name: short.to_string(),
                    });
                    self.emit_tool_call(func_name, &func_args);

                    let result = tokio::select! {
                        r = tools::execute_tool(func_name, &func_args, &self.bin_path, &self.config) => r,
                        _ = &mut cancel_rx => {
                            self.session.messages.push(json!({
                                "role": "assistant",
                                "content": "⛔ Cancelled by user."
                            }));
                            self.send_event(AgentEvent::Done);
                            return "⛔ Task cancelled by user.".to_string();
                        }
                    };

                    self.emit_tool_result(func_name, &result);
                    let mut result_content =
                        serde_json::to_string_pretty(&result).unwrap_or_default();

                    // Inject active task context only after tool execution
                    if let Ok(Some(task)) = crate::task::Task::load_active() {
                        let task_info = format!(
                            "\n\n<active_task>\nID: {}\nTitle: {}\nStatus: {}\nSteps: {}\nPlan:\n{}\n</active_task>",
                            task.id, task.title, task.status, task.steps.len(), task.plan
                        );
                        result_content.push_str(&task_info);
                    }

                    self.session.messages.push(json!({
                        "role": "tool",
                        "content": result_content,
                        "tool_call_id": tc["id"]
                    }));
                }
                continue;
            }

            if let Some(content) = message.get("content").and_then(|v| v.as_str()) {
                self.send_event(AgentEvent::Done);
                return content.to_string();
            }

            self.send_event(AgentEvent::Done);
            return "⚠️  LLM returned empty response.".to_string();
        }
        self.send_event(AgentEvent::Done);
        "⚠️  Max tool-calling rounds reached.".to_string()
    }

    fn short_tool_name(&self, name: &str) -> &'static str {
        match name {
            "daemon_skeleton" => "skel",
            "daemon_get_hash" => "hash",
            "daemon_file_info" => "finfo",
            "daemon_catalog" => "cat",
            "daemon_loc_info" => "loc",
            "codex_eyes_ls" => "ls",
            "codex_eyes_grep" => "grep",
            "codex_eyes_sed" => "sed",
            "ast_grep_search" => "ast-search",
            "apply_patch" => "patch",
            "codex_eyes_createFile" => "create",
            "codex_eyes_gitdiff" => "diff",
            "codex_eyes_search" => "search",
            "codex_eyes_find" => "find",
            "codex_eyes_find_fuzzy" => "find~",
            "codex_eyes_read" => "read",
            "codex_eyes_cargo_check" => "cargo",
            "codex_eyes_undo" => "undo",
            "task_create" => "task+",
            "task_step" => "task.step",
            "task_complete" => "task✓",
            "task_list" => "tasks",
            "task_abort" => "task✗",
            "ast_explain" => "ast-explain",
            _ => "tool",
        }
    }

    fn emit_tool_call(&self, func_name: &str, func_args: &Value) {
        let summary = match func_name {
            "ast_grep_search" => {
                let pat: String = func_args["pattern"]
                    .as_str()
                    .unwrap_or("?")
                    .chars()
                    .take(50)
                    .collect();
                let lang = func_args["lang"].as_str().unwrap_or("rust");
                format!("🌲 ast-search [{}]: {}", lang, pat)
            }
            "ast_explain" => {
                let p = func_args["path"].as_str().unwrap_or("?");
                if let Some(line) = func_args["line"].as_u64() {
                    format!("🔬 AST explain: {} @L{}", p, line)
                } else if let Some(text) = func_args["text"].as_str() {
                    let t: String = text.chars().take(30).collect();
                    format!("🔬 AST explain: {} \"{}\"", p, t)
                } else {
                    format!("🔬 AST explain: {}", p)
                }
            }
            "apply_patch" => {
                let p = func_args["path"].as_str().unwrap_or("?");
                let patch: String = func_args["patch"]
                    .as_str()
                    .unwrap_or("?")
                    .lines()
                    .next()
                    .unwrap_or("?")
                    .chars()
                    .take(40)
                    .collect();
                format!("📝 Patch: {} {}", p, patch)
            }
            "task_create" => {
                let title = func_args["title"].as_str().unwrap_or("?");
                format!(
                    "📋 Create task: {}\n📄 Plan:\n{}",
                    title,
                    func_args["plan"].as_str().unwrap_or("")
                )
            }
            "task_step" => {
                let action = func_args["action"].as_str().unwrap_or("?");
                format!(
                    "📋 Step: {} — {}",
                    action,
                    func_args["description"].as_str().unwrap_or("")
                )
            }
            "task_complete" => "📋 Task complete ✓".to_string(),
            "task_list" => "📋 List tasks".to_string(),
            "task_abort" => "📋 Task aborted ✗".to_string(),
            "codex_eyes_sed" => {
                let p = func_args["path"].as_str().unwrap_or("?");
                let find: String = func_args["find"]
                    .as_str()
                    .unwrap_or("?")
                    .chars()
                    .take(40)
                    .collect();
                format!("✏️  Sed: {} s/{}/…/", p, find)
            }
            "codex_eyes_createFile" => {
                let p = func_args["path"].as_str().unwrap_or("?");
                format!("📝 Create: {}", p)
            }
            _ => self.default_call_summary(func_name, func_args),
        };
        self.send_event(AgentEvent::ToolCall {
            name: func_name.to_string(),
            summary,
        });
    }

    fn default_call_summary(&self, func_name: &str, func_args: &Value) -> String {
        match func_name {
            "codex_eyes_ls" => {
                format!("📂 Ls: {}", func_args["path"].as_str().unwrap_or("."))
            }
            "codex_eyes_gitdiff" => {
                format!("🔍 Diff: {}", func_args["path"].as_str().unwrap_or("all"))
            }
            "codex_eyes_cargo_check" => {
                format!("🦀 Cargo: {}", func_args["path"].as_str().unwrap_or("."))
            }
            "codex_eyes_read" => {
                format!("📖 Read: {}", func_args["path"].as_str().unwrap_or("?"))
            }
            "codex_eyes_undo" => {
                format!("↩️  Undo: {}", func_args["path"].as_str().unwrap_or("?"))
            }
            _ => format!("⚙️  {}", func_name),
        }
    }

    fn emit_tool_result(&self, func_name: &str, result: &HashMap<String, Value>) {
        let success = result
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let summary = match func_name {
            "ast_grep_search" => {
                let count = result
                    .get("matches")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                format!("{} matches", count)
            }
            "ast_explain" => {
                if let Some(nodes) = result.get("nodes").and_then(|v| v.as_array()) {
                    let count = nodes.len();
                    let patterns: Vec<String> = nodes
                        .iter()
                        .filter_map(|n| n["suggested_pattern"].as_str().map(|s| s.to_string()))
                        .collect();
                    if patterns.is_empty() {
                        format!("{} node(s)", count)
                    } else {
                        format!("{} node(s), suggestions: {}", count, patterns.join(" | "))
                    }
                } else {
                    String::new()
                }
            }
            "apply_patch" => {
                if success {
                    "patched".to_string()
                } else {
                    "failed".to_string()
                }
            }
            "task_create" => result
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("created")
                .to_string(),
            "task_step" => "recorded".to_string(),
            "task_complete" => "closed".to_string(),
            "task_list" => {
                let count = result
                    .get("tasks")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                format!("{} tasks", count)
            }
            "codex_eyes_sed" => {
                if success {
                    let count = result
                        .get("replacements")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    format!("{} replacement(s)", count)
                } else {
                    result
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("failed")
                        .to_string()
                }
            }
            "codex_eyes_createFile" => {
                if success {
                    "created".to_string()
                } else {
                    "failed".to_string()
                }
            }
            "codex_eyes_cargo_check" => {
                let exit = result
                    .get("exit_code")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(-1);
                if exit == 0 {
                    "passed ✅".to_string()
                } else {
                    format!("exit {}", exit)
                }
            }
            "codex_eyes_gitdiff" => {
                if result
                    .get("has_changes")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    if let Some(diff) = result["output"].as_str() {
                        self.emit_diff_lines(diff, 20);
                    }
                    "changes found".to_string()
                } else {
                    "no changes".to_string()
                }
            }
            "codex_eyes_read" => {
                if let Some(out) = result.get("output").and_then(|v| v.as_str()) {
                    for l in out.lines().take(20) {
                        self.send_event(AgentEvent::DiffLine {
                            line: format!("  {}", l),
                        });
                    }
                }
                String::new()
            }
            _ => String::new(),
        };
        self.send_event(AgentEvent::ToolResult {
            name: func_name.to_string(),
            success,
            summary,
        });
    }

    fn emit_diff_lines(&self, diff: &str, max_lines: usize) {
        let lines: Vec<&str> = diff.lines().collect();
        for dl in lines.iter().take(max_lines) {
            self.send_event(AgentEvent::DiffLine {
                line: dl.to_string(),
            });
        }
        if lines.len() > max_lines {
            self.send_event(AgentEvent::DiffLine {
                line: format!("... ({} more)", lines.len() - max_lines),
            });
        }
    }
}
