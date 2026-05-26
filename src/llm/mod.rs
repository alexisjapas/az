use thiserror::Error;

pub mod ollama;

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("http: {0}")]
    Http(String),
    #[error("decode: {0}")]
    Decode(String),
    #[error("backend: {0}")]
    Backend(String),
}

#[derive(Debug, Clone)]
pub struct GenerationRequest {
    pub system: String,
    pub user: String,
    pub model: String,
    pub temperature: f32,
    pub json_mode: bool,
}

#[derive(Debug, Clone)]
pub struct GenerationResponse {
    pub text: String,
}

pub trait Llm: Send + Sync {
    fn generate(&self, req: GenerationRequest) -> Result<GenerationResponse, LlmError>;
}

/// Backend capable de produire des embeddings de texte (séparé de `Llm` car
/// tous les backends ne font pas les deux).
pub trait EmbeddingsLlm: Send + Sync {
    fn embed(&self, model: &str, text: &str) -> Result<Vec<f32>, LlmError>;
}
