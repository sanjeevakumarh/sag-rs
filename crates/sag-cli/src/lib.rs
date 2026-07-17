//! `sag-cli` — "how I talk to it".
//!
//! Swap seam: [`CommandDispatch`]. The CLI parses a [`Command`] and dispatches it
//! (to the controller), returning a line for the terminal. Keeping dispatch behind
//! a trait lets the whole CLI be driven in tests without a TTY or a live
//! controller. Step 1 defines the seam plus a [`FakeCli`]; the real one is `clap`
//! (derive) for parsing and `ratatui` for the `attach` TUI, over a real controller.

use async_trait::async_trait;

/// A parsed command the CLI can dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `sag run fix "<request>"`
    RunFix { request: String },
    /// `sag doctor`
    Doctor,
}

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("empty request")]
    EmptyRequest,
}

/// The swap seam: dispatch a parsed command and produce terminal output.
#[async_trait]
pub trait CommandDispatch: Send + Sync {
    async fn dispatch(&self, cmd: Command) -> Result<String, CliError>;
}

/// Offline dispatcher for tests: renders each command to a fixed line without a
/// controller or TTY, so command parsing and routing can be tested headless.
pub struct FakeCli;

#[async_trait]
impl CommandDispatch for FakeCli {
    async fn dispatch(&self, cmd: Command) -> Result<String, CliError> {
        Ok(match cmd {
            Command::RunFix { request } if request.trim().is_empty() => {
                return Err(CliError::EmptyRequest)
            }
            Command::RunFix { request } => format!("submitted fix: {request}"),
            Command::Doctor => "all systems nominal".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dispatches_run_fix_and_doctor() {
        assert_eq!(
            FakeCli
                .dispatch(Command::RunFix {
                    request: "fix cache expiry".into()
                })
                .await
                .unwrap(),
            "submitted fix: fix cache expiry"
        );
        assert_eq!(
            FakeCli.dispatch(Command::Doctor).await.unwrap(),
            "all systems nominal"
        );
    }

    #[tokio::test]
    async fn empty_run_fix_request_is_an_error() {
        assert!(matches!(
            FakeCli
                .dispatch(Command::RunFix { request: "".into() })
                .await,
            Err(CliError::EmptyRequest)
        ));
    }
}
