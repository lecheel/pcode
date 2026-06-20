use crate::debug;
use anyhow::{anyhow, Result};
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

pub struct LLMClient {
    pub base_url: String,
    pub model: String,
    api_key: Option<String>,
    client: Client,
    num_ctx: u64,
}

impl LLMClient {
    pub fn new(
        base_url: &str,
        model: &str,
        timeout_secs: u64,
        api_key: Option<String>,
        num_ctx: u64,
    ) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .expect("Failed to create HTTP client");
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            api_key,
            client,
            num_ctx,
        }
    }

    pub async fn chat(&self, messages: &[Value], tools: &[Value]) -> Result<Value> {
        let mut payload = json!({
            "model": self.model,
            "messages": messages,
            "tools": tools,
            "stream": false,
        });
        payload["options"] = json!({ "num_ctx": self.num_ctx });

        debug::separator("LLM REQUEST");
        // Fix: Log the actual URL that will be requested
        let url = if self.base_url.ends_with("/v1") || self.base_url.ends_with("/v1/") {
            format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
        } else {
            format!(
                "{}/v1/chat/completions",
                self.base_url.trim_end_matches('/')
            )
        };
        debug::dbg_msg(format!("  URL: {}", url)); // Use the variable here!
        debug::dbg_msg(format!("  Model: {}", self.model));
        debug::dbg_msg(format!(
            "  Auth: {}",
            if self.api_key.is_some() {
                "Bearer *** (key set)"
            } else {
                "None"
            }
        ));
        debug::dbg_msg(format!("  Messages: {} items", messages.len()));
        for (idx, msg) in messages.iter().enumerate() {
            let role = msg["role"].as_str().unwrap_or("?");
            if let Some(tc) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                debug::dbg_msg(format!(
                    "  [{}] role={} tool_calls={}:",
                    idx,
                    role,
                    tc.len()
                ));
                for t in tc {
                    debug::dbg_msg(format!(
                        "        → {}",
                        t["function"]["name"].as_str().unwrap_or("?")
                    ));
                }
            } else if let Some(content) = msg["content"].as_str() {
                let preview: String = content.chars().take(200).collect();
                let suffix = if content.len() > 200 { "..." } else { "" };
                debug::dbg_msg(format!(
                    "  [{}] role={}: {}{}",
                    idx,
                    role,
                    preview.replace('\n', "\\n"),
                    suffix
                ));
            }
        }
        debug::separator("");

        // Inside pub async fn chat(...)
        let url = if self.base_url.ends_with("/v1") || self.base_url.ends_with("/v1/") {
            format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
        } else {
            format!(
                "{}/v1/chat/completions",
                self.base_url.trim_end_matches('/')
            )
        };
        let mut request = self.client.post(&url).json(&payload);
        if let Some(ref key) = self.api_key {
            request = request.bearer_auth(key);
        }
        let resp = request.send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("LLM Server Error: {} — {}", status, body));
        }
        let data: Value = resp.json().await?;

        debug::separator("LLM RESPONSE");
        if let Some(choice) = data["choices"].as_array().and_then(|c| c.first()) {
            let msg = &choice["message"];
            debug::dbg_msg(format!(
                "  Finish: {}",
                choice["finish_reason"].as_str().unwrap_or("?")
            ));
            if let Some(usage) = data.get("usage") {
                debug::dbg_msg(format!(
                    "  Tokens: prompt={} completion={}",
                    usage["prompt_tokens"], usage["completion_tokens"]
                ));
            }
            if let Some(tc) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                debug::dbg_msg(format!("  Tool calls: {}", tc.len()));
                for t in tc {
                    let fn_name = t["function"]["name"].as_str().unwrap_or("?");
                    debug::dbg_msg(format!("    → {}", fn_name));
                    if let Some(args_str) = t["function"]["arguments"].as_str() {
                        if let Ok(args) = serde_json::from_str::<Value>(args_str) {
                            debug::dbg_json("args", &args, 1000);
                        }
                    }
                }
            } else if let Some(content) = msg["content"].as_str() {
                let preview: String = content.chars().take(300).collect();
                let suffix = if content.len() > 300 { "..." } else { "" };
                debug::dbg_msg(format!(
                    "  Content: {}{}",
                    preview.replace('\n', "\\n"),
                    suffix
                ));
            }
            if let Some(rc) = msg.get("reasoning_content").and_then(|v| v.as_str()) {
                let preview: String = rc.chars().take(200).collect();
                debug::dbg_msg(format!("  Reasoning: {}...", preview.replace('\n', "\\n")));
            }
        }
        debug::separator("");
        Ok(data)
    }

    pub async fn health(&self) -> bool {
        if self.api_key.is_some() {
            let models_url = if self.base_url.ends_with("/v1") || self.base_url.ends_with("/v1/") {
                format!("{}/models", self.base_url.trim_end_matches('/'))
            } else {
                format!("{}/v1/models", self.base_url.trim_end_matches('/'))
            };
            let mut req = self.client.get(&models_url);
            if let Some(ref key) = self.api_key {
                req = req.bearer_auth(key);
            }
            if let Ok(resp) = req.send().await {
                return resp.status().is_success();
            }
            return false;
        }
        if let Ok(resp) = self
            .client
            .get(format!("{}/health", self.base_url))
            .send()
            .await
        {
            if resp.status().is_success() {
                return true;
            }
        }
        if let Ok(resp) = self.client.get(&self.base_url).send().await {
            return resp.status().is_success();
        }
        false
    }
}
