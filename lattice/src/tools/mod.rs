pub mod sandbox;
pub mod tool_definitions;
pub mod tool_error;
pub mod tool_registry;
pub mod executor;

use crate::core::types::ToolCall;

/// Executes a tool call and returns the result string.
#[async_trait::async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(&self, call: &ToolCall) -> String;
}

pub use sandbox::SandboxConfig;
pub use tool_definitions::default_tool_definitions;
pub use executor::DefaultToolExecutor;
pub use executor::RegistryToolAccess;
