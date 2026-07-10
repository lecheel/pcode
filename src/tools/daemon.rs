use crate::config::AppConfig;
use crate::tools::common::{err_result, ok_result, ToolResult};
use reqwest::header::HeaderMap;
use serde_json::{json, Value};

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

pub async fn execute_daemon_skeleton(config: &AppConfig) -> ToolResult {
    match daemon_get(config, "/skeleton").await {
        Ok((text, _)) => ok_result(&text),
        Err(e) => err_result(&e),
    }
}

pub async fn execute_daemon_get_hash(args: &Value, config: &AppConfig) -> ToolResult {
    let hash = match args["hash"].as_str() {
        Some(h) => h,
        None => return err_result("Missing required argument: hash"),
    };
    match daemon_get(config, &format!("/{}", hash)).await {
        Ok((text, _)) => ok_result(&text),
        Err(e) => err_result(&e),
    }
}

pub async fn execute_daemon_file_info(args: &Value, config: &AppConfig) -> ToolResult {
    let path = match args["path"].as_str() {
        Some(p) => p,
        None => return err_result("Missing required argument: path"),
    };
    match daemon_get(config, &format!("/file-info/{}", path)).await {
        Ok((text, _)) => ok_result(&text),
        Err(e) => err_result(&e),
    }
}

pub async fn execute_daemon_catalog(config: &AppConfig) -> ToolResult {
    match daemon_get(config, "/catalog").await {
        Ok((text, _)) => ok_result(&text),
        Err(e) => err_result(&e),
    }
}

pub async fn execute_daemon_loc_info(config: &AppConfig) -> ToolResult {
    match daemon_get(config, "/loc-info").await {
        Ok((text, _)) => ok_result(&text),
        Err(e) => err_result(&e),
    }
}

pub async fn fetch_repos(config: &AppConfig) -> Result<Vec<Value>, String> {
    let (text, _) = daemon_get(config, "/repos").await?;
    let arr: Vec<Value> = serde_json::from_str(&text).map_err(|e| format!("Failed to parse repos: {}", e))?;
    Ok(arr)
}

pub async fn resolve_repo_by_path(config: &AppConfig, path: &str) -> Result<String, String> {
    let base_url = config.daemon.base_url.trim_end_matches('/');
    let url = format!("{}/resolve", base_url);
    let client = reqwest::Client::new();
    let body = json!({ "path": path });
    let resp = client.post(&url).json(&body).send().await
        .map_err(|e| format!("Daemon request failed: {}", e))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("Failed to read daemon response: {}", e))?;
    if !status.is_success() {
        return Err(format!("Daemon error {}: {}", status, text));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("Failed to parse resolve response: {}", e))?;
    v["id"].as_str().map(|s| s.to_string()).ok_or_else(|| "No id in resolve response".to_string())
}

pub async fn sync_repo(config: &AppConfig, repo_id: &str) -> Result<String, String> {
    let base_url = config.daemon.base_url.trim_end_matches('/');
    let url = format!("{}/sync/{}", base_url, repo_id);
    let client = reqwest::Client::new();
    let resp = client.post(&url).send().await
        .map_err(|e| format!("Daemon request failed: {}", e))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("Failed to read daemon response: {}", e))?;
    if !status.is_success() {
        return Err(format!("Daemon error {}: {}", status, text));
    }
    Ok(text)
}