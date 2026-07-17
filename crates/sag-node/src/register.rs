//! The node's client side of the registration wire: announce this node to a
//! controller, and probe a candidate controller's `/status` (for discovery and,
//! later, leader election).

use sag_proto::{
    NodeDescriptor, RegisterRequest, RegisterResponse, StatusResponse, PROTOCOL_VERSION,
};

#[derive(Debug, thiserror::Error)]
pub enum RegisterError {
    #[error("transport error talking to {url}: {source}")]
    Transport { url: String, source: reqwest::Error },
    #[error("controller {url} rejected registration: {message}")]
    Rejected { url: String, message: String },
}

/// A handle to one controller endpoint. Cheap to clone-construct; reuses one
/// `reqwest::Client` (connection pool) per instance.
pub struct ControllerClient {
    base_url: String,
    http: reqwest::Client,
}

impl ControllerClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Self {
            base_url,
            http: reqwest::Client::new(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Probe whether a live controller answers here, returning its `/status`.
    /// Discovery (increment 4) calls this across the known peer list.
    pub async fn status(&self) -> Result<StatusResponse, RegisterError> {
        let url = format!("{}/status", self.base_url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|source| RegisterError::Transport {
                url: url.clone(),
                source,
            })?;
        resp.json()
            .await
            .map_err(|source| RegisterError::Transport { url, source })
    }

    /// Announce `node` to this controller (an upsert on the controller side).
    pub async fn register(&self, node: NodeDescriptor) -> Result<RegisterResponse, RegisterError> {
        let url = format!("{}/register", self.base_url);
        let req = RegisterRequest {
            protocol_version: PROTOCOL_VERSION,
            node,
        };
        let resp = self
            .http
            .post(&url)
            .json(&req)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|source| RegisterError::Transport {
                url: url.clone(),
                source,
            })?;
        let resp: RegisterResponse =
            resp.json()
                .await
                .map_err(|source| RegisterError::Transport {
                    url: url.clone(),
                    source,
                })?;
        if !resp.accepted {
            return Err(RegisterError::Rejected {
                url,
                message: resp.message.unwrap_or_default(),
            });
        }
        Ok(resp)
    }
}
