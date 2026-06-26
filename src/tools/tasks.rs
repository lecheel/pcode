use crate::task::Task;
use crate::tools::common::{err_result, ok_result, ToolResult};
use serde_json::{json, Value};

pub fn execute_task_create(args: &Value) -> ToolResult {
    let title = args["title"].as_str().unwrap_or("Untitled");
    let description = args["description"].as_str().unwrap_or("");
    let plan = args["plan"].as_str().unwrap_or("");
    match Task::create(title, description, plan) {
        Ok(task) => {
            let mut result = ok_result("task created");
            result.insert("task_id".into(), json!(task.id));
            result.insert("title".into(), json!(task.title));
            result.insert("path".into(), json!(format!(".impl/{}", task.id)));
            result
        }
        Err(e) => err_result(&format!("Failed to create task: {}", e)),
    }
}

pub fn execute_task_step(args: &Value) -> ToolResult {
    let action = args["action"].as_str().unwrap_or("unknown");
    let target = args["target"].as_str().unwrap_or("");
    let description = args["description"].as_str().unwrap_or("");
    let success = args["success"].as_bool().unwrap_or(true);
    match Task::load_active() {
        Ok(Some(mut task)) => match task.add_step(action, target, description, success) {
            Ok(()) => {
                let mut result = ok_result("step recorded");
                result.insert("step".into(), json!(task.steps.len()));
                result.insert("task_id".into(), json!(task.id));
                result
            }
            Err(e) => err_result(&format!("Failed to record step: {}", e)),
        },
        Ok(None) => err_result("No active task. Create one with task_create first."),
        Err(e) => err_result(&format!("Failed to load task: {}", e)),
    }
}

pub fn execute_task_complete() -> ToolResult {
    match Task::load_active() {
        Ok(Some(mut task)) => match task.complete() {
            Ok(()) => {
                let mut result = ok_result("task completed");
                result.insert("task_id".into(), json!(task.id));
                result.insert("steps".into(), json!(task.steps.len()));
                result
            }
            Err(e) => err_result(&format!("Failed to complete task: {}", e)),
        },
        Ok(None) => err_result("No active task."),
        Err(e) => err_result(&format!("Failed to load task: {}", e)),
    }
}

pub fn execute_task_list() -> ToolResult {
    match Task::list_all() {
        Ok(tasks) => {
            let task_list: Vec<Value> = tasks
                .iter()
                .map(|t| {
                    json!({
                        "id": t.id,
                        "title": t.title,
                        "status": t.status,
                        "steps": t.steps.len(),
                        "created_at": t.created_at
                    })
                })
                .collect();
            let mut result = ok_result("tasks listed");
            result.insert("tasks".into(), json!(task_list));
            result
        }
        Err(e) => err_result(&format!("Failed to list tasks: {}", e)),
    }
}

pub fn execute_task_abort() -> ToolResult {
    match Task::load_active() {
        Ok(Some(mut task)) => match task.abort() {
            Ok(()) => {
                let mut result = ok_result("task aborted");
                result.insert("task_id".into(), json!(task.id));
                result
            }
            Err(e) => err_result(&format!("Failed to abort task: {}", e)),
        },
        Ok(None) => err_result("No active task."),
        Err(e) => err_result(&format!("Failed to load task: {}", e)),
    }
}
