mod faster_whisper;
mod whisper_cpp;

use crate::config::Engine;
use anyhow::Result;
use std::sync::Mutex;

pub trait Transcriber: Send {
    /// `prompt` is the free-form decoder prompt the user typed, in their own
    /// language.
    fn transcribe(&mut self, pcm: &[f32], language: &str, prompt: &str) -> Result<String>;
}

fn build(engine: Engine, model: &str) -> Result<Box<dyn Transcriber>> {
    Ok(match engine {
        Engine::WhisperCpp => Box::new(whisper_cpp::WhisperCpp::load(model)?),
        Engine::FasterWhisper => Box::new(faster_whisper::FasterWhisper::new(model)),
    })
}

type Loaded = (Engine, String, Box<dyn Transcriber>);

/// Loading a model costs seconds: the last one is kept in memory.
static CACHE: Mutex<Option<Loaded>> = Mutex::new(None);

pub fn transcribe(
    engine: Engine,
    model: &str,
    pcm: &[f32],
    language: &str,
    prompt: &str,
) -> Result<String> {
    let mut cache = CACHE.lock().unwrap();
    let stale = !matches!(&*cache, Some((e, m, _)) if *e == engine && m == model);
    if stale {
        *cache = Some((engine, model.to_string(), build(engine, model)?));
    }
    cache.as_mut().unwrap().2.transcribe(pcm, language, prompt)
}

/// Drops the in-memory model (engine switched, model deleted).
pub fn unload() {
    *CACHE.lock().unwrap() = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Engine;

    /// Times whisper.cpp on a fixed clip, to compare backends.
    ///
    /// Run it once per build to measure the effect of a feature:
    ///
    /// ```sh
    /// SUSURRE_BENCH_WAV=/path/jfk.wav SUSURRE_BENCH_MODEL=large-v3-turbo \
    ///     ./dev.sh test -- --ignored bench --nocapture
    /// ./dev.sh test --features vulkan -- --ignored bench --nocapture
    /// ```
    ///
    /// The first pass loads the model from disk and is reported separately:
    /// only the later ones say anything about decoding speed.
    #[test]
    #[ignore]
    fn bench_whisper_cpp() {
        let wav = std::env::var("SUSURRE_BENCH_WAV").expect("SUSURRE_BENCH_WAV");
        let model = std::env::var("SUSURRE_BENCH_MODEL").unwrap_or_else(|_| "small".into());

        let mut reader = hound::WavReader::open(&wav).expect("opening the clip");
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, 16_000, "the clip must be 16 kHz");
        assert_eq!(spec.channels, 1, "the clip must be mono");
        let pcm: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / i16::MAX as f32)
            .collect();
        let seconds = pcm.len() as f32 / 16_000.0;

        println!("\nmodel {model}, clip {seconds:.1} s, vulkan={}", cfg!(feature = "vulkan"));
        for pass in 0..4 {
            unload();
            let started = std::time::Instant::now();
            let text = transcribe(Engine::WhisperCpp, &model, &pcm, "en", "").expect("transcribe");
            let cold = started.elapsed();

            let started = std::time::Instant::now();
            transcribe(Engine::WhisperCpp, &model, &pcm, "en", "").expect("transcribe");
            let warm = started.elapsed();

            println!(
                "pass {pass}: load+decode {:.2} s, decode alone {:.2} s ({:.1}x realtime)",
                cold.as_secs_f32(),
                warm.as_secs_f32(),
                seconds / warm.as_secs_f32(),
            );
            if pass == 0 {
                println!("transcript: {text:?}");
            }
        }
    }
}
