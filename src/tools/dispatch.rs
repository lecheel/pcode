use crate::config::AppConfig;
use crate::tools::ast_grep;
use crate::tools::cargo;
use crate::tools::common::{err_result, is_disabled, ToolResult};
use crate::tools::custom;
use crate::tools::daemon;
use crate::tools::fs;
use crate::tools::patch;
use crate::tools::tasks;
use serde_json::Value;

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
        "daemon_skeleton" => daemon::execute_daemon_skeleton(config).await,
        "daemon_get_hash" => daemon::execute_daemon_get_hash(args, config).await,
        "daemon_file_info" => daemon::execute_daemon_file_info(args, config).await,
        "daemon_catalog" => daemon::execute_daemon_catalog(config).await,
        "daemon_loc_info" => daemon::execute_daemon_loc_info(config).await,

        "codex_eyes_ls" => fs::execute_ls(args, config).await,
        "codex_eyes_grep" => fs::execute_grep(args, config).await,
        "codex_eyes_sed" => fs::execute_sed(args, config).await,
        "codex_eyes_createFile" => fs::execute_create_file(args, config).await,
        "codex_eyes_gitdiff" => fs::execute_gitdiff(args, config).await,
        "codex_eyes_search" => fs::execute_search(args, config).await,
        "codex_eyes_find" => fs::execute_find(args, config).await,
        "codex_eyes_find_fuzzy" => fs::execute_find_fuzzy(args, config).await,
        "codex_eyes_read" => fs::execute_read(args, config).await,
        "codex_eyes_undo" => fs::execute_undo(args, config).await,

        "codex_eyes_cargo_check" => cargo::execute_cargo_check(args, config).await,

        "apply_patch" => patch::execute_apply_patch(args, config).await,

        "ast_grep_search" => ast_grep::execute_ast_grep_search(args),
        "ast_grep_replace" => ast_grep::execute_ast_grep_replace(args),
        "ast_explain" => ast_grep::execute_ast_explain(args, config).await,

        "task_create" => tasks::execute_task_create(args),
        "task_step" => tasks::execute_task_step(args),
        "task_complete" => tasks::execute_task_complete(),
        "task_list" => tasks::execute_task_list(),
        "task_abort" => tasks::execute_task_abort(),

        _ => {
            if let Some(custom) = config.tools.custom.iter().find(|t| t.name == name) {
                custom::execute_custom_tool(custom, args, config).await
            } else {
                err_result(&format!("Unknown tool: {}", name))
            }
        }
    }
}
