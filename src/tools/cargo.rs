use crate::config::AppConfig;
use crate::tools::common::{err_result, resolve_path, ToolResult};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

static LAST_CARGO_CHECK: AtomicU64 = AtomicU64::new(0);

pub async fn execute_cargo_check(args: &Value, config: &AppConfig) -> ToolResult {
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
