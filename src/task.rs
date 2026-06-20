use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStep {
    pub step: u32,
    pub action: String, // "sed" | "ast-grep" | "create" | "search" | "verify"
    pub target: String, // file path or pattern
    pub description: String,
    pub success: bool,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String, // e.g. "task_143025_0001"
    pub title: String,
    pub description: String,
    pub plan: String, // markdown plan
    pub steps: Vec<TaskStep>,
    pub status: String, // "active" | "done" | "aborted"
    pub created_at: String,
}

impl Task {
    pub fn dir_base() -> PathBuf {
        PathBuf::from(".impl")
    }

    /// Create a new task with timestamped ID
    pub fn create(title: &str, description: &str, plan: &str) -> Result<Self> {
        let now = std::time::SystemTime::now();
        let duration = now.duration_since(std::time::UNIX_EPOCH)?;
        let secs = duration.as_secs();
        let hours = (secs / 3600) % 24;
        let mins = (secs / 60) % 60;
        let secs = secs % 60;
        let ts = format!("{:02}{:02}{:02}", hours, mins, secs);

        // Find next sequence number
        let seq = Self::next_seq(&ts)?;

        let id = format!("task_{}_{}{:04}", ts, "", seq);
        let id = format!("task_{}_{:04}", ts, seq);

        let created_at = format!("{:02}:{:02}:{:02}", hours, mins, secs);

        let task = Self {
            id,
            title: title.to_string(),
            description: description.to_string(),
            plan: plan.to_string(),
            steps: Vec::new(),
            status: "active".to_string(),
            created_at,
        };

        // Ensure .impl/ exists
        fs::create_dir_all(Self::dir_base())?;

        // Write plan.md
        let task_dir = Self::dir_base().join(&task.id);
        fs::create_dir_all(&task_dir)?;
        fs::write(task_dir.join("plan.md"), &task.plan)?;

        // Write task metadata
        let json = serde_json::to_string_pretty(&task)?;
        fs::write(task_dir.join("task.json"), json)?;

        Ok(task)
    }

    fn next_seq(ts: &str) -> Result<u32> {
        let base = Self::dir_base();
        if !base.exists() {
            return Ok(1);
        }
        let prefix = format!("task_{}", ts);
        let mut max_seq: u32 = 0;
        for entry in fs::read_dir(&base)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&prefix) {
                if let Some(seq_str) = name
                    .trim_start_matches(&prefix)
                    .trim_start_matches('_')
                    .strip_suffix("")
                {
                    // Handle task_143025_0001 format
                    let parts: Vec<&str> = name.split('_').collect();
                    if parts.len() >= 4 {
                        if let Ok(seq) = parts[3].parse::<u32>() {
                            max_seq = max_seq.max(seq);
                        }
                    }
                }
            }
        }
        Ok(max_seq + 1)
    }

    /// Add a step to the current task
    pub fn add_step(
        &mut self,
        action: &str,
        target: &str,
        description: &str,
        success: bool,
    ) -> Result<()> {
        let now = std::time::SystemTime::now();
        let duration = now.duration_since(std::time::UNIX_EPOCH)?;
        let secs = duration.as_secs();
        let timestamp = format!(
            "{:02}:{:02}:{:02}",
            (secs / 3600) % 24,
            (secs / 60) % 60,
            secs % 60
        );

        let step = TaskStep {
            step: self.steps.len() as u32 + 1,
            action: action.to_string(),
            target: target.to_string(),
            description: description.to_string(),
            success,
            timestamp,
        };
        self.steps.push(step);
        self.save()?;
        Ok(())
    }

    /// Mark task as done
    pub fn complete(&mut self) -> Result<()> {
        self.status = "done".to_string();
        self.save()?;
        Ok(())
    }

    /// Mark task as aborted
    pub fn abort(&mut self) -> Result<()> {
        self.status = "aborted".to_string();
        self.save()?;
        Ok(())
    }

    /// Save task to disk
    pub fn save(&self) -> Result<()> {
        let task_dir = Self::dir_base().join(&self.id);
        fs::create_dir_all(&task_dir)?;
        let json = serde_json::to_string_pretty(self)?;
        fs::write(task_dir.join("task.json"), json)?;
        // Also write changes log
        if !self.steps.is_empty() {
            let log = self
                .steps
                .iter()
                .map(|s| {
                    format!(
                        "[{}] Step {}: {} → {} | {} | {}",
                        s.timestamp,
                        s.step,
                        s.action,
                        s.target,
                        s.description,
                        if s.success { "✅" } else { "❌" }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            fs::write(task_dir.join("changes.log"), log)?;
        }
        Ok(())
    }

    /// Load current active task
    pub fn load_active() -> Result<Option<Self>> {
        let base = Self::dir_base();
        if !base.exists() {
            return Ok(None);
        }
        for entry in fs::read_dir(&base)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                let task_file = path.join("task.json");
                if task_file.exists() {
                    let content = fs::read_to_string(&task_file)?;
                    let task: Task = serde_json::from_str(&content)?;
                    if task.status == "active" {
                        return Ok(Some(task));
                    }
                }
            }
        }
        Ok(None)
    }

    /// List all tasks
    pub fn list_all() -> Result<Vec<Self>> {
        let base = Self::dir_base();
        if !base.exists() {
            return Ok(Vec::new());
        }
        let mut tasks = Vec::new();
        for entry in fs::read_dir(&base)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                let task_file = path.join("task.json");
                if task_file.exists() {
                    let content = fs::read_to_string(&task_file)?;
                    if let Ok(task) = serde_json::from_str::<Task>(&content) {
                        tasks.push(task);
                    }
                }
            }
        }
        tasks.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(tasks)
    }
}
