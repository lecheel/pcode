use crate::config::{AppConfig, CustomTool};
use crate::tools::common::{err_result, ToolResult};
use serde_json::{json, Value};
use std::collections::HashMap;

pub async fn execute_custom_tool(
    custom: &CustomTool,
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
