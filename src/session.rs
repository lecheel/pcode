use anyhow::Result;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

pub struct Session {
    pub messages: Vec<Value>,
}

impl Session {
    pub fn new(system_prompt: &str) -> Self {
        Self {
            messages: vec![json!({
                "role": "system",
                "content": system_prompt
            })],
        }
    }

    /// Save session to a JSON file.
    pub fn save(&self, dir: &str, name: &str) -> Result<()> {
        fs::create_dir_all(dir)?;
        let path = Path::new(dir).join(format!("{}.json", name));
        let content = serde_json::to_string_pretty(&self.messages)?;
        fs::write(&path, content)?;
        Ok(())
    }

    /// Load session from a JSON file.
    pub fn load(dir: &str, name: &str) -> Result<Self> {
        let path = Path::new(dir).join(format!("{}.json", name));
        let content = fs::read_to_string(&path)?;
        let messages: Vec<Value> = serde_json::from_str(&content)?;
        Ok(Self { messages })
    }

    /// Reset to a fresh session with the original system prompt.
    pub fn reset(&mut self) {
        if let Some(system_msg) = self.messages.first().cloned() {
            self.messages.clear();
            self.messages.push(system_msg);
        }
    }
}
