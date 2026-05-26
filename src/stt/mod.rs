use thiserror::Error;

pub mod whisper;

#[derive(Debug, Error)]
pub enum SttError {
    #[error("chargement du modèle: {0}")]
    ModelLoad(String),
    #[error("transcription: {0}")]
    Transcribe(String),
}

#[derive(Debug, Clone)]
pub struct Transcript {
    pub text: String,
    pub language: Option<String>,
}

pub trait SpeechToText: Send {
    fn transcribe(&mut self, samples: &[f32]) -> Result<Transcript, SttError>;
}
