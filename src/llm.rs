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
    api_type: String,
}

impl LLMClient {
    pub fn new(
        base_url: &str,
        model: &str,
        timeout_secs: u64,
        api_key: Option<String>,
        num_ctx: u64,
        api_type: String,
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
            api_type,
        }
    }

    pub async fn chat(&self, messages: &[Value], tools: &[Value]) -> Result<Value> {
        if self.api_type == "anthropic" {
            return self.chat_anthropic(messages, tools).await;
        }
        self.chat_openai(messages, tools).await
    }

    async fn chat_anthropic(&self, messages: &[Value], tools: &[Value]) -> Result<Value> {
        let mut system_prompt = String::new();
        let mut anthropic_messages = Vec::new();

        for msg in messages {
            let role = msg["role"].as_str().unwrap_or("user");
            if role == "system" {
                system_prompt = msg["content"].as_str().unwrap_or("").to_string();
            } else if role == "tool" {
                let tool_call_id = msg["tool_call_id"].as_str().unwrap_or("");
                let content = msg["content"].as_str().unwrap_or("");
                anthropic_messages.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tool_call_id,
                        "content": content
                    }]
                }));
            } else if role == "assistant" {
                let mut content_blocks = Vec::new();
                if let Some(text) = msg["content"].as_str() {
                    if !text.is_empty() {
                        content_blocks.push(json!({"type": "text", "text": text}));
                    }
                }
                if let Some(tool_calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                    for tc in tool_calls {
                        let id = tc["id"].as_str().unwrap_or("");
                        let name = tc["function"]["name"].as_str().unwrap_or("");
                        let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
                        let input: Value = serde_json::from_str(args_str).unwrap_or(json!({}));
                        content_blocks.push(json!({
                            "type": "tool_use",
                            "id": id,
                            "name": name,
                            "input": input
                        }));
                    }
                }
                if !content_blocks.is_empty() {
                    anthropic_messages.push(json!({
                        "role": "assistant",
                        "content": content_blocks
                    }));
                }
            } else {
                anthropic_messages.push(msg.clone());
            }
        }

        let anthropic_tools: Vec<Value> = tools
            .iter()
            .filter_map(|t| {
                let func = &t["function"];
                let name = func["name"].as_str()?;
                Some(json!({
                    "name": name,
                    "description": func["description"].as_str().unwrap_or(""),
                    "input_schema": func["parameters"].clone()
                }))
            })
            .collect();

        let mut payload = json!({
            "model": self.model,
            "max_tokens": 4096,
            "system": system_prompt,
            "messages": anthropic_messages,
            "stream": false
        });

        if !anthropic_tools.is_empty() {
            payload["tools"] = json!(anthropic_tools);
        }

        debug::separator("LLM REQUEST");
        let url = if self.base_url.ends_with("/v1") || self.base_url.ends_with("/v1/") {
            format!("{}/messages", self.base_url.trim_end_matches('/'))
        } else {
            format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
        };
        debug::dbg_msg(format!("  URL: {}", url));
        debug::dbg_msg(format!("  Model: {}", self.model));
        debug::dbg_msg(format!("  API Type: Anthropic"));
        debug::dbg_msg(format!(
            "  Auth: {}",
            if self.api_key.is_some() {
                "Bearer *** (key set)"
            } else {
                "None"
            }
        ));
        debug::dbg_msg(format!("  Messages: {} items", anthropic_messages.len()));
        debug::separator("");

        let mut request = self.client.post(&url).json(&payload);
        if let Some(ref key) = self.api_key {
            request = request
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01");
        }

        let resp = request.send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("LLM Server Error: {} — {}", status, body));
        }

        let data: Value = resp.json().await?;
        let mut openai_response = json!({});
        let mut message = json!({"role": "assistant"});
        let mut text_content = String::new();
        let mut tool_calls = Vec::new();

        if let Some(content) = data["content"].as_array() {
            for block in content {
                if block["type"].as_str() == Some("text") {
                    text_content.push_str(block["text"].as_str().unwrap_or(""));
                } else if block["type"].as_str() == Some("tool_use") {
                    let id = block["id"].as_str().unwrap_or("").to_string();
                    let name = block["name"].as_str().unwrap_or("").to_string();
                    let input = &block["input"];
                    let args_str =
                        serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string());
                    tool_calls.push(json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": args_str
                        }
                    }));
                }
            }
        }

        if !text_content.is_empty() {
            message["content"] = json!(text_content);
        } else {
            message["content"] = Value::Null;
        }

        if !tool_calls.is_empty() {
            message["tool_calls"] = json!(tool_calls);
        }

        let finish_reason = if data["stop_reason"].as_str() == Some("tool_use") {
            "tool_calls"
        } else {
            "stop"
        };

        openai_response["choices"] = json!([{
            "message": message,
            "finish_reason": finish_reason
        }]);

        if let Some(usage) = data.get("usage") {
            openai_response["usage"] = json!({
                "prompt_tokens": usage["input_tokens"],
                "completion_tokens": usage["output_tokens"]
            });
        }

        debug::separator("LLM RESPONSE");
        if let Some(choice) = openai_response["choices"]
            .as_array()
            .and_then(|c| c.first())
        {
            let msg = &choice["message"];
            debug::dbg_msg(format!(
                "  Finish: {}",
                choice["finish_reason"].as_str().unwrap_or("?")
            ));
            if let Some(usage) = openai_response.get("usage") {
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
        }
        debug::separator("");
        Ok(openai_response)
    }

    async fn chat_openai(&self, messages: &[Value], tools: &[Value]) -> Result<Value> {
        let mut payload = json!({
            "model": self.model,
            "messages": messages,
            "stream": false,
        });

        if !tools.is_empty() {
            payload["tools"] = json!(tools);
        }

        if self.api_type == "ollama" {
            payload["options"] = json!({ "num_ctx": self.num_ctx });
        }
        debug::separator("LLM REQUEST");
        let url = if self.base_url.ends_with("/v1") || self.base_url.ends_with("/v1/") {
            format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
        } else {
            format!(
                "{}/v1/chat/completions",
                self.base_url.trim_end_matches('/')
            )
        };
        debug::dbg_msg(format!("  URL: {}", url));
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
        let mut request = self
            .client
            .post(&url)
            .header("Accept", "application/json")
            .json(&payload);
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
