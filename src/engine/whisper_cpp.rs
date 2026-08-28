use super::Transcriber;
use crate::config::Engine;
use crate::models;
use anyhow::{bail, Result};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub struct WhisperCpp {
    ctx: WhisperContext,
}

impl WhisperCpp {
    pub fn load(model: &str) -> Result<Self> {
        let path = models::path(Engine::WhisperCpp, model);
        if !path.is_file() {
            bail!("the {model} model is not downloaded");
        }
        // whisper-rs derives use_gpu from its own `_gpu` feature, which only
        // the backends it forwards (cuda, metal, ...) turn on. Vulkan is
        // enabled one crate below, through whisper-rs-sys, so the flag has to
        // be set by hand or the weights stay on the CPU and the Vulkan device
        // is registered but never used.
        let mut params = WhisperContextParameters::default();
        params.use_gpu(cfg!(feature = "vulkan"));
        let ctx = WhisperContext::new_with_params(path.to_str().unwrap(), params)?;
        Ok(Self { ctx })
    }
}

impl Transcriber for WhisperCpp {
    fn transcribe(&mut self, pcm: &[f32], language: &str, prompt: &str) -> Result<String> {
        let mut state = self.ctx.create_state()?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(if language == "auto" { None } else { Some(language) });
        params.set_translate(false);

        // Short dictation: deterministic decoding, no context carried from one
        // recording to the next, and non-speech tokens suppressed. The
        // temperature fallback is what produces the usual hallucinations on a
        // noisy or very short clip.
        params.set_temperature(0.0);
        params.set_temperature_inc(0.0);
        params.set_no_context(true);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_no_speech_thold(0.6);
        if !prompt.is_empty() {
            params.set_initial_prompt(prompt);
        }

        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_n_threads(std::thread::available_parallelism().map_or(4, |n| n.get() as i32));
        state.full(params, pcm)?;

        let mut out = String::new();
        for i in 0..state.full_n_segments()? {
            out.push_str(&state.full_get_segment_text(i)?);
        }
        Ok(out.trim().to_string())
    }
}
