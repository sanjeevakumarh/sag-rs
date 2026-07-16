//! `sag-tools` — "let agents do things".
//!
//! Swap seam: [`Tool`]. Every capability an agent invokes — native trusted (git,
//! ripgrep, fs reads), sandboxed shell (bubblewrap / rootless podman), or MCP —
//! sits behind this one trait, so the engine invokes tools uniformly and the
//! sandbox policy is a swap, not a special case (ARCHITECTURE.md "Tools come in
//! three classes"). Step 1 defines the seam plus an offline [`FakeTool`].

use async_trait::async_trait;

/// A request to run a named tool with positional arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub name: String,
    pub args: Vec<String>,
}

/// Captured result of a tool invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub stdout: String,
    pub exit_code: i32,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),
}

/// The swap seam: a named capability the engine can invoke and capture output from.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    async fn invoke(&self, call: ToolCall) -> Result<ToolOutput, ToolError>;
}

/// Offline echo tool for tests: joins the args to stdout and exits 0, rejecting
/// any call not addressed to it. No subprocess, no sandbox, no network.
pub struct FakeTool {
    name: String,
}

impl FakeTool {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[async_trait]
impl Tool for FakeTool {
    fn name(&self) -> &str {
        &self.name
    }

    async fn invoke(&self, call: ToolCall) -> Result<ToolOutput, ToolError> {
        if call.name != self.name {
            return Err(ToolError::UnknownTool(call.name));
        }
        Ok(ToolOutput {
            stdout: call.args.join(" "),
            exit_code: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_tool_echoes_args_for_its_own_name() {
        let tool = FakeTool::new("echo");
        let out = tool
            .invoke(ToolCall {
                name: "echo".into(),
                args: vec!["hello".into(), "world".into()],
            })
            .await
            .unwrap();
        assert_eq!(out.stdout, "hello world");
        assert_eq!(out.exit_code, 0);
    }

    #[tokio::test]
    async fn fake_tool_rejects_a_call_for_another_tool() {
        let tool = FakeTool::new("echo");
        assert!(matches!(
            tool.invoke(ToolCall {
                name: "git".into(),
                args: vec![],
            })
            .await,
            Err(ToolError::UnknownTool(_))
        ));
    }
}
