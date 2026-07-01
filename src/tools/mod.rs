// Public API surface stays identical to the old single-file module,
// so callers (main.rs / agent loop) only need to keep `use crate::tools;`.

pub mod ast_grep;
pub mod cargo;
pub mod common;
pub mod custom;
pub mod daemon;
pub mod deps;
mod dispatch;
pub mod fs;
pub mod patch;
pub mod registry;
pub mod tasks;

pub use common::{resolve_path, ToolResult};
pub use deps::{check_tool_dependencies, find_codex_eyes};
pub use dispatch::execute_tool;
pub use registry::build_tools;
