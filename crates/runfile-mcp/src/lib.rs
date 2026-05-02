pub mod install;
pub mod server;
pub mod tools;

pub use install::{install_for_agent, mcp_config_snippet, InstallResult};
pub use server::{run_server, RunfileMcpServer};
pub use tools::{build_tool_defs, inspect_json, ToolDef};

#[cfg(test)]
mod tests;
