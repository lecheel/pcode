//--+ file:///src/config.rs
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SkillGroupConfig {
    pub name: String,
    pub description: String,
    pub emoji: String,
    #[serde(default)]
    pub tools: Vec<String>,
    pub prompt: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AppConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default)]
    pub tools: ToolsConfig,
    #[serde(default)]
    pub debug: DebugConfig,
    #[serde(default)]
    pub repl: ReplConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            daemon: DaemonConfig::default(),
            tools: ToolsConfig::default(),
            debug: DebugConfig::default(),
            repl: ReplConfig::default(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DaemonConfig {
    #[serde(default = "default_daemon_url")]
    pub base_url: String,
    #[serde(default)]
    pub active_repo: Option<String>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            base_url: default_daemon_url(),
            active_repo: None,
        }
    }
}

fn default_daemon_url() -> String {
    "http://127.0.0.1:7890".into()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ServerConfig {
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default = "default_num_ctx")]
    pub num_ctx: u64,
    #[serde(default = "default_api_type")]
    pub api_type: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            model: default_model(),
            timeout: default_timeout(),
            api_key: None,
            num_ctx: default_num_ctx(),
            api_type: default_api_type(),
        }
    }
}

fn default_base_url() -> String {
    "http://localhost:11434".into()
}
fn default_model() -> String {
    "qwen3:32b".into()
}
fn default_api_type() -> String {
    "openai".into()
}
fn default_timeout() -> u64 {
    120
}
fn default_cargo_check_cooldown() -> u64 {
    10
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ToolsConfig {
    #[serde(default)]
    pub codex_eyes_binary: Option<String>,
    #[serde(default = "default_true")]
    pub auto_verify: bool,
    #[serde(default)]
    pub disabled: HashMap<String, bool>,
    #[serde(default)]
    pub custom: Vec<CustomTool>,
    #[serde(default)]
    pub project_root: String,
    #[serde(default)]
    pub allow_paths: Vec<String>,
    #[serde(default = "default_cargo_check_cooldown")]
    pub cargo_check_cooldown_secs: u64,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        let mut disabled = HashMap::new();
        disabled.insert("codex_eyes_shell".into(), true);
        Self {
            codex_eyes_binary: None,
            auto_verify: true,
            disabled,
            custom: Vec::new(),
            project_root: String::new(),
            allow_paths: vec!["/tmp".to_string()],
            cargo_check_cooldown_secs: default_cargo_check_cooldown(),
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CustomTool {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub parameters: HashMap<String, ParameterDef>,
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub execute: ExecuteConfig,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ParameterDef {
    #[serde(rename = "type", default = "default_param_type")]
    pub param_type: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

impl Default for ParameterDef {
    fn default() -> Self {
        Self {
            param_type: "string".into(),
            description: String::new(),
            required: false,
        }
    }
}

fn default_param_type() -> String {
    "string".into()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ExecuteConfig {
    #[serde(rename = "type", default = "default_exec_type")]
    pub exec_type: String,
    #[serde(default)]
    pub command_template: String,
    #[serde(default = "default_exec_timeout")]
    pub timeout_secs: u64,
}

impl Default for ExecuteConfig {
    fn default() -> Self {
        Self {
            exec_type: default_exec_type(),
            command_template: String::new(),
            timeout_secs: default_exec_timeout(),
        }
    }
}

fn default_exec_type() -> String {
    "shell".into()
}
fn default_exec_timeout() -> u64 {
    30
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DebugConfig {
    #[serde(default)]
    pub enabled: bool,
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self { enabled: false }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ReplConfig {
    #[serde(default = "default_history_file")]
    pub history_file: String,
    #[serde(default = "default_command_history_file")]
    pub command_history_file: String,
    #[serde(default = "default_sessions_dir")]
    pub sessions_dir: String,
    #[serde(default = "default_max_rounds")]
    pub max_rounds: u32,
    #[serde(default = "default_true")]
    pub auto_enable_tools_on_code_request: bool,
    #[serde(default)]
    pub skills: Vec<SkillGroupConfig>,
}

impl Default for ReplConfig {
    fn default() -> Self {
        Self {
            history_file: default_history_file(),
            command_history_file: default_command_history_file(),
            sessions_dir: default_sessions_dir(),
            max_rounds: default_max_rounds(),
            auto_enable_tools_on_code_request: true,
            skills: Vec::new(),
        }
    }
}

fn pcode_dir() -> std::path::PathBuf {
    dirs::config_dir()
        .expect("Cannot determine config directory")
        .join("pcode")
}

fn ensure_pcode_dirs() {
    let dir = pcode_dir();
    let _ = std::fs::create_dir_all(dir.join("sessions"));
}

fn default_command_history_file() -> String {
    ensure_pcode_dirs();
    pcode_dir()
        .join("cmd_history.txt")
        .to_string_lossy()
        .to_string()
}

fn default_history_file() -> String {
    ensure_pcode_dirs();
    pcode_dir()
        .join("history.txt")
        .to_string_lossy()
        .to_string()
}

fn default_sessions_dir() -> String {
    ensure_pcode_dirs();
    pcode_dir().join("sessions").to_string_lossy().to_string()
}

fn default_max_rounds() -> u32 {
    15
}
fn default_num_ctx() -> u64 {
    65536
}

pub fn load_config(path: Option<&Path>) -> Result<AppConfig> {
    if let Some(p) = path {
        let content =
            std::fs::read_to_string(p).with_context(|| format!("Read config: {}", p.display()))?;
        let cfg: AppConfig =
            toml::from_str(&content).with_context(|| format!("Parse config: {}", p.display()))?;
        return Ok(cfg);
    }
    let default_path = pcode_dir().join("config.toml");
    if default_path.exists() {
        let content = std::fs::read_to_string(&default_path)
            .with_context(|| format!("Read config: {}", default_path.display()))?;
        let cfg: AppConfig = toml::from_str(&content)
            .with_context(|| format!("Parse config: {}", default_path.display()))?;
        return Ok(cfg);
    }
    Ok(AppConfig::default())
}

pub fn ensure_dirs(config: &AppConfig) {
    if let Some(parent) = std::path::Path::new(&config.repl.history_file).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::create_dir_all(&config.repl.sessions_dir);
}
