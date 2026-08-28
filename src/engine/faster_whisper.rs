use super::Transcriber;
use crate::config::Engine;
use crate::{audio, models};
use anyhow::{bail, Result};
use std::process::Command;

/// CTranslate2 has no maintained Rust binding: this goes through the python
/// helper shipped in the package, which loads faster-whisper.
pub struct FasterWhisper {
    model: String,
}

impl FasterWhisper {
    pub fn new(model: &str) -> Self {
        Self { model: model.to_string() }
    }
}

fn helper() -> String {
    std::env::var("SUSURRE_CT2_HELPER").unwrap_or_else(|_| "/app/libexec/susurre-ct2.py".into())
}

impl Transcriber for FasterWhisper {
    fn transcribe(&mut self, pcm: &[f32], language: &str, prompt: &str) -> Result<String> {
        let dir = models::path(Engine::FasterWhisper, &self.model);
        if !dir.join("model.bin").is_file() {
            bail!("the {} model is not downloaded", self.model);
        }
        let wav = crate::config::data_dir().join("capture.wav");
        std::fs::create_dir_all(wav.parent().unwrap())?;
        audio::write_wav(&wav, pcm)?;

        let out = Command::new("python3")
            .arg(helper())
            .arg(&dir)
            .arg(&wav)
            .arg(language)
            .arg(prompt)
            .output()?;
        let _ = std::fs::remove_file(&wav);
        if !out.status.success() {
            bail!("faster-whisper: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}
