use serde::{Deserialize, Serialize};

use super::{EmbeddingsLlm, GenerationRequest, GenerationResponse, Llm, LlmError};

pub const DEFAULT_URL: &str = "http://localhost:11434";

pub struct OllamaClient {
    base_url: String,
}

impl OllamaClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }

    pub fn from_env() -> Self {
        let url = std::env::var("AZ_OLLAMA_URL").unwrap_or_else(|_| DEFAULT_URL.to_string());
        Self::new(url)
    }
}

#[derive(Debug, Serialize)]
struct OllamaRequest<'a> {
    model: &'a str,
    prompt: String,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<&'a str>,
    options: OllamaOptions,
}

#[derive(Debug, Serialize)]
struct OllamaOptions {
    temperature: f32,
}

#[derive(Debug, Deserialize)]
struct OllamaResponse {
    response: String,
}

#[derive(Debug, Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    prompt: &'a str,
}

#[derive(Debug, Deserialize)]
struct EmbedResponse {
    embedding: Vec<f32>,
}

impl EmbeddingsLlm for OllamaClient {
    fn embed(&self, model: &str, text: &str) -> Result<Vec<f32>, LlmError> {
        let url = format!(
            "{}/api/embeddings",
            self.base_url.trim_end_matches('/')
        );
        let body = EmbedRequest { model, prompt: text };
        let resp = ureq::post(&url)
            .send_json(&body)
            .map_err(|e| LlmError::Http(e.to_string()))?;
        let parsed: EmbedResponse = resp
            .into_json()
            .map_err(|e| LlmError::Decode(e.to_string()))?;
        if parsed.embedding.is_empty() {
            return Err(LlmError::Backend(format!(
                "ollama a renvoyé un embedding vide pour le modèle '{model}'"
            )));
        }
        Ok(parsed.embedding)
    }
}

impl Llm for OllamaClient {
    fn generate(&self, req: GenerationRequest) -> Result<GenerationResponse, LlmError> {
        let prompt = if req.system.is_empty() {
            req.user.clone()
        } else {
            format!("{}\n\n{}", req.system, req.user)
        };
        let body = OllamaRequest {
            model: &req.model,
            prompt,
            stream: false,
            format: if req.json_mode { Some("json") } else { None },
            options: OllamaOptions {
                temperature: req.temperature,
            },
        };
        let url = format!("{}/api/generate", self.base_url.trim_end_matches('/'));
        let resp = ureq::post(&url)
            .send_json(&body)
            .map_err(|e| LlmError::Http(e.to_string()))?;
        let parsed: OllamaResponse = resp
            .into_json()
            .map_err(|e| LlmError::Decode(e.to_string()))?;
        Ok(GenerationResponse {
            text: parsed.response,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    #[test]
    fn ollama_generate_basic_response() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/api/generate");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"response":"bonjour le monde","done":true}"#);
        });

        let client = OllamaClient::new(server.base_url());
        let resp = client
            .generate(GenerationRequest {
                system: "sys".into(),
                user: "usr".into(),
                model: "test-model".into(),
                temperature: 0.5,
                json_mode: false,
            })
            .unwrap();
        assert_eq!(resp.text, "bonjour le monde");
        mock.assert();
    }

    #[test]
    fn ollama_generate_sets_json_format() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/api/generate")
                .json_body_partial(r#"{"format":"json"}"#);
            then.status(200)
                .body(r#"{"response":"{\"k\":1}"}"#);
        });

        let client = OllamaClient::new(server.base_url());
        let resp = client
            .generate(GenerationRequest {
                system: "".into(),
                user: "give me json".into(),
                model: "m".into(),
                temperature: 0.0,
                json_mode: true,
            })
            .unwrap();
        assert_eq!(resp.text, r#"{"k":1}"#);
        mock.assert();
    }

    #[test]
    fn ollama_embed_basic() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/api/embeddings");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"embedding":[0.1,0.2,0.3]}"#);
        });
        let client = OllamaClient::new(server.base_url());
        let v = client.embed("nomic-embed-text", "bonjour").unwrap();
        assert_eq!(v, vec![0.1, 0.2, 0.3]);
        mock.assert();
    }

    #[test]
    fn ollama_embed_rejects_empty_response() {
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(POST).path("/api/embeddings");
            then.status(200).body(r#"{"embedding":[]}"#);
        });
        let client = OllamaClient::new(server.base_url());
        let err = client.embed("m", "x").unwrap_err();
        assert!(matches!(err, LlmError::Backend(_)));
    }

    #[test]
    fn ollama_http_error_propagates() {
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(POST).path("/api/generate");
            then.status(500).body("boom");
        });
        let client = OllamaClient::new(server.base_url());
        let err = client
            .generate(GenerationRequest {
                system: "".into(),
                user: "u".into(),
                model: "m".into(),
                temperature: 0.0,
                json_mode: false,
            })
            .unwrap_err();
        assert!(matches!(err, LlmError::Http(_)));
    }
}
