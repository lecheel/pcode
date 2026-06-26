// Public API surface stays identical to the old single-file module,
// so callers (main.rs / agent loop) only need to keep `use crate::tools;`.

pub mod common;
pub mod deps;
pub mod daemon;
pub mod fs;
pub mod cargo;
pub mod ast_grep;
pub mod custom;
pub mod tasks;
pub mod patch;
pub mod registry;
mod dispatch;

pub use common::{resolve_path, ToolResult};
pub use deps::{check_tool_dependencies, find_codex_eyes};
pub use registry::build_tools;
pub use dispatch::execute_tool;
