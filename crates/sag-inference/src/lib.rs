//! `sag-inference` — "talk to a model".
//!
//! Swap seam: [`InferenceEndpoint`]. Every model server (vLLM, llama.cpp, Ollama)
//! is normalized behind this one trait using an OpenAI-compatible request model.
//! The engine asks for *capabilities* (`code.edit`, `code.explore`, `reason.deep`)
//! and never for provider names. [`FakeEndpoint`] is the offline echo used in
//! tests; [`OpenAiCompatEndpoint`] is the real `reqwest` client that speaks the
//! OpenAI `/v1` surface (so it works against vLLM, llama.cpp, and Ollama alike).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// An OpenAI-compatible-ish generation request (trimmed to what we need).
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
    #[error("inference transport error: {0}")]
    Transport(String),
    #[error("model response contained no choices")]
    EmptyResponse,
}

/// The swap seam: a normalized handle to one model server.
#[async_trait]
pub trait InferenceEndpoint: Send + Sync {
    async fn generate(&self, req: GenerateRequest) -> Result<GenerateResponse, InferenceError>;
    async fn health(&self) -> Result<Health, InferenceError>;
}

// ── OpenAI-compatible wire shapes (only the fields we read/write) ─────────────

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: [ChatMessage<'a>; 1],
    stream: bool,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    #[serde(default)]
    content: String,
}

#[derive(Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelObject>,
}

#[derive(Deserialize)]
struct ModelObject {
    id: String,
}

/// A real endpoint over an OpenAI-compatible server: `/v1/chat/completions` for
/// generation, `/v1/models` for the catalog and a health probe. `base_url` may be
/// the server root (`http://host:11434`) or already include `/v1`; both work.
pub struct OpenAiCompatEndpoint {
    /// Server root, with any trailing `/` or `/v1` stripped.
    base: String,
    http: reqwest::Client,
}

impl OpenAiCompatEndpoint {
    pub fn new(base_url: impl Into<String>) -> Self {
        let trimmed = base_url.into().trim_end_matches('/').to_string();
        let base = trimmed
            .strip_suffix("/v1")
            .map(|s| s.to_string())
            .unwrap_or(trimmed);
        Self {
            base,
            http: reqwest::Client::new(),
        }
    }

    /// List the model ids the server advertises (`GET /v1/models`). Used by the
    /// node to discover its catalog when none is configured explicitly.
    pub async fn list_models(&self) -> Result<Vec<String>, InferenceError> {
        let url = format!("{}/v1/models", self.base);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| InferenceError::Transport(e.to_string()))?;
        let parsed: ModelsResponse = resp
            .json()
            .await
            .map_err(|e| InferenceError::Transport(e.to_string()))?;
        Ok(parsed.data.into_iter().map(|m| m.id).collect())
    }
}

#[async_trait]
impl InferenceEndpoint for OpenAiCompatEndpoint {
    async fn generate(&self, req: GenerateRequest) -> Result<GenerateResponse, InferenceError> {
        let url = format!("{}/v1/chat/completions", self.base);
        let body = ChatRequest {
            model: &req.model,
            messages: [ChatMessage {
                role: "user",
                content: &req.prompt,
            }],
            stream: false,
        };
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| InferenceError::Transport(e.to_string()))?;
        let parsed: ChatResponse = resp
            .json()
            .await
            .map_err(|e| InferenceError::Transport(e.to_string()))?;
        let text = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or(InferenceError::EmptyResponse)?;
        Ok(GenerateResponse { text })
    }

    async fn health(&self) -> Result<Health, InferenceError> {
        let url = format!("{}/v1/models", self.base);
        match self
            .http
            .get(&url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
        {
            Ok(_) => Ok(Health::Ready),
            Err(_) => Ok(Health::Unavailable),
        }
    }
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

    #[test]
    fn openai_endpoint_normalizes_base_url() {
        // A `/v1` suffix or trailing slash must not double up in the request path.
        for input in [
            "http://h:11434",
            "http://h:11434/",
            "http://h:11434/v1",
            "http://h:11434/v1/",
        ] {
            assert_eq!(OpenAiCompatEndpoint::new(input).base, "http://h:11434");
        }
    }

    /// A local axum stub stands in for a real model server, so `generate`,
    /// `list_models`, and `health` are exercised over real HTTP — no GPU, no
    /// external network, just loopback.
    mod stub_server {
        use super::*;
        use axum::routing::{get, post};
        use axum::{Json, Router};

        async fn spawn() -> String {
            let app = Router::new()
                .route(
                    "/v1/models",
                    get(|| async {
                        Json(serde_json::json!({
                            "data": [{ "id": "qwen3-coder:30b" }, { "id": "gpt-oss:20b" }]
                        }))
                    }),
                )
                .route(
                    "/v1/chat/completions",
                    post(|Json(body): Json<serde_json::Value>| async move {
                        let content = body["messages"][0]["content"].as_str().unwrap_or("");
                        Json(serde_json::json!({
                            "choices": [{ "message": { "role": "assistant", "content": format!("echo: {content}") } }]
                        }))
                    }),
                );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            format!("http://{addr}")
        }

        #[tokio::test]
        async fn generate_list_and_health_against_stub() {
            let base = spawn().await;
            let ep = OpenAiCompatEndpoint::new(&base);

            assert_eq!(ep.health().await.unwrap(), Health::Ready);
            assert_eq!(
                ep.list_models().await.unwrap(),
                ["qwen3-coder:30b", "gpt-oss:20b"]
            );
            let resp = ep
                .generate(GenerateRequest {
                    model: "qwen3-coder:30b".into(),
                    prompt: "hi".into(),
                })
                .await
                .unwrap();
            assert_eq!(resp.text, "echo: hi");
        }

        #[tokio::test]
        async fn health_is_unavailable_when_server_is_down() {
            // Nothing listening on this port → health reports Unavailable, not an error.
            let ep = OpenAiCompatEndpoint::new("http://127.0.0.1:1");
            assert_eq!(ep.health().await.unwrap(), Health::Unavailable);
        }
    }
}
