//! `sag-inference` — "talk to a model".
//!
//! Swap seam: [`InferenceEndpoint`]. Every model server (vLLM, llama.cpp, Ollama)
//! is normalized behind this one trait using an OpenAI-compatible request model.
//! The engine asks for *capabilities* (`code.edit`, `code.explore`, `reason.deep`)
//! and never for provider names. Step 3 builds the `reqwest` `/v1` client; step 1
//! defines the seam plus an offline [`FakeEndpoint`].

use async_trait::async_trait;

/// An OpenAI-compatible-ish generation request (trimmed for the skeleton).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerateRequest {
    pub model: String,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerateResponse {
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Ready,
    Unavailable,
}

#[derive(Debug, thiserror::Error)]
pub enum InferenceError {
    #[error("endpoint unavailable")]
    Unavailable,
}

/// The swap seam: a normalized handle to one model server.
#[async_trait]
pub trait InferenceEndpoint: Send + Sync {
    async fn generate(&self, req: GenerateRequest) -> Result<GenerateResponse, InferenceError>;
    async fn health(&self) -> Result<Health, InferenceError>;
}

/// Offline echo endpoint for tests: always healthy, deterministically prefixes
/// the prompt. No network, no GPU, no `reqwest`.
pub struct FakeEndpoint;

#[async_trait]
impl InferenceEndpoint for FakeEndpoint {
    async fn generate(&self, req: GenerateRequest) -> Result<GenerateResponse, InferenceError> {
        Ok(GenerateResponse {
            text: format!("[{}] {}", req.model, req.prompt),
        })
    }

    async fn health(&self) -> Result<Health, InferenceError> {
        Ok(Health::Ready)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_endpoint_is_ready_and_echoes() {
        assert_eq!(FakeEndpoint.health().await.unwrap(), Health::Ready);
        let resp = FakeEndpoint
            .generate(GenerateRequest {
                model: "qwen3-coder:30b".into(),
                prompt: "hello".into(),
            })
            .await
            .unwrap();
        assert_eq!(resp.text, "[qwen3-coder:30b] hello");
    }
}
