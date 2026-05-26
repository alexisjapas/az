use std::path::Path;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use super::{SpeechToText, SttError, Transcript};

pub struct WhisperStt {
    ctx: WhisperContext,
    language: String,
    n_threads: i32,
}

impl WhisperStt {
    pub fn load<P: AsRef<Path>>(
        model_path: P,
        language: impl Into<String>,
    ) -> Result<Self, SttError> {
        let path_str = model_path
            .as_ref()
            .to_str()
            .ok_or_else(|| SttError::ModelLoad("chemin modèle non UTF-8".into()))?;
        let ctx = WhisperContext::new_with_params(path_str, WhisperContextParameters::default())
            .map_err(|e| SttError::ModelLoad(e.to_string()))?;
        let n_threads = std::thread::available_parallelism()
            .map(|n| n.get() as i32)
            .unwrap_or(4)
            .min(8);
        Ok(Self {
            ctx,
            language: language.into(),
            n_threads,
        })
    }
}

impl SpeechToText for WhisperStt {
    fn transcribe(&mut self, samples: &[f32]) -> Result<Transcript, SttError> {
        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| SttError::Transcribe(e.to_string()))?;

        let lang_opt = if self.language == "auto" {
            None
        } else {
            Some(self.language.as_str())
        };

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.n_threads);
        params.set_translate(false);
        params.set_language(lang_opt);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        state
            .full(params, samples)
            .map_err(|e| SttError::Transcribe(e.to_string()))?;

        let n = state
            .full_n_segments()
            .map_err(|e| SttError::Transcribe(e.to_string()))?;
        let mut text = String::new();
        for i in 0..n {
            let seg = state
                .full_get_segment_text(i)
                .map_err(|e| SttError::Transcribe(e.to_string()))?;
            text.push_str(&seg);
        }
        Ok(Transcript {
            text: text.trim().to_string(),
            language: lang_opt.map(|s| s.to_string()),
        })
    }
}
