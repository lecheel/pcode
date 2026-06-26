use crate::config::AppConfig;
use crate::patch;
use crate::tools::common::{err_result, ok_result, ToolResult};
use serde_json::Value;
use std::path::PathBuf;

pub async fn execute_apply_patch(args: &Value, config: &AppConfig) -> ToolResult {
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
