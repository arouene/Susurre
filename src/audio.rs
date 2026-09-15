use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};

pub const TARGET_RATE: u32 = 16_000;

/// Always the default input. Inside the Flatpak sandbox cpal only sees the ALSA
/// host, whose device list is a pile of plugin PCMs (`upmix`, `front:CARD=…`,
/// `dsnoop`, …) rather than real microphones, and enumerating them makes
/// libasound print errors about `/dev/dsp` and missing `audio` groups. The one
/// entry that matters is `default`, which PipeWire routes according to the
/// input chosen in GNOME Settings, so device selection belongs there.
fn default_input() -> Result<cpal::Device> {
    cpal::default_host()
        .default_input_device()
        .ok_or_else(|| anyhow!("no input device"))
}

/// Push-to-talk recorder. A cpal stream is not `Send`: create and drop it on
/// the same thread (here the GTK main thread).
pub struct Recorder {
    _stream: cpal::Stream,
    buf: Arc<Mutex<Vec<f32>>>,
    rate: u32,
    channels: u16,
}

impl Recorder {
    pub fn start() -> Result<Self> {
        let device = default_input()?;
        let cfg = device.default_input_config()?;
        let rate = cfg.sample_rate().0;
        let channels = cfg.channels();
        let buf = Arc::new(Mutex::new(Vec::<f32>::with_capacity(rate as usize * 10)));

        let sink = buf.clone();
        let err = |e| log::error!("audio stream: {e}");
        let stream = match cfg.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &cfg.into(),
                move |data: &[f32], _: &_| sink.lock().unwrap().extend_from_slice(data),
                err,
                None,
            )?,
            cpal::SampleFormat::I16 => device.build_input_stream(
                &cfg.into(),
                move |data: &[i16], _: &_| {
                    let mut b = sink.lock().unwrap();
                    b.extend(data.iter().map(|s| *s as f32 / i16::MAX as f32));
                },
                err,
                None,
            )?,
            cpal::SampleFormat::U16 => device.build_input_stream(
                &cfg.into(),
                move |data: &[u16], _: &_| {
                    let mut b = sink.lock().unwrap();
                    b.extend(data.iter().map(|s| *s as f32 / u16::MAX as f32 - 0.5));
                },
                err,
                None,
            )?,
            f => return Err(anyhow!("unsupported sample format: {f:?}")),
        };
        stream.play()?;
        Ok(Self { _stream: stream, buf, rate, channels })
    }

    /// Stops the recording and returns mono 16 kHz PCM.
    pub fn finish(self) -> Vec<f32> {
        let raw = std::mem::take(&mut *self.buf.lock().unwrap());
        let mono = downmix(&raw, self.channels);
        resample(&mono, self.rate, TARGET_RATE)
    }
}

fn downmix(samples: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    let n = channels as usize;
    samples
        .chunks_exact(n)
        .map(|f| f.iter().sum::<f32>() / n as f32)
        .collect()
}

/// Linear resampling. Good enough for speech down to 16 kHz.
/// ponytail: linear interpolation, move to a polyphase resampler (rubato) only
/// if transcription quality suffers.
fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let out_len = (input.len() as f64 / ratio).floor() as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let idx = pos.floor() as usize;
            let frac = (pos - idx as f64) as f32;
            let a = input[idx];
            let b = *input.get(idx + 1).unwrap_or(&a);
            a + (b - a) * frac
        })
        .collect()
}

/// Analysis window of the voice activity detector: 20 ms.
const VAD_WINDOW: usize = TARGET_RATE as usize / 50;
/// Margin kept around the detected speech.
const VAD_MARGIN_WINDOWS: usize = 10;

fn rms(window: &[f32]) -> f32 {
    (window.iter().map(|s| s * s).sum::<f32>() / window.len() as f32).sqrt()
}

/// Energy based voice activity detection: trims leading and trailing silence,
/// and rejects an entirely silent capture. Without it Whisper hallucinates on
/// silence ("Subtitles by the Amara.org community...").
///
/// ponytail: an energy threshold, not a neural VAD. faster-whisper runs Silero
/// on its side; whisper.cpp does not expose one through whisper-rs. Good enough
/// for push-to-talk, where the user speaks deliberately.
pub fn trim_silence(pcm: &[f32]) -> Option<Vec<f32>> {
    if pcm.len() < VAD_WINDOW {
        return None;
    }
    let levels: Vec<f32> = pcm.chunks(VAD_WINDOW).map(rms).collect();

    // Noise floor: 20th percentile of the windows.
    let mut sorted = levels.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let noise = sorted[sorted.len() / 5];
    let threshold = (noise * 3.0).max(0.005);

    let first = levels.iter().position(|l| *l > threshold)?;
    let last = levels.iter().rposition(|l| *l > threshold)?;

    let start = first.saturating_sub(VAD_MARGIN_WINDOWS) * VAD_WINDOW;
    let end = ((last + VAD_MARGIN_WINDOWS + 1) * VAD_WINDOW).min(pcm.len());
    Some(normalize(&pcm[start..end]))
}

/// Lifts the level of quiet microphones. The gain is capped so background hiss
/// is not amplified.
fn normalize(pcm: &[f32]) -> Vec<f32> {
    let peak = pcm.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak < 1e-6 {
        return pcm.to_vec();
    }
    let gain = (0.9 / peak).min(8.0);
    if gain <= 1.0 {
        return pcm.to_vec();
    }
    pcm.iter().map(|s| (s * gain).clamp(-1.0, 1.0)).collect()
}

/// Writes mono 16 kHz PCM as 16 bit WAV (input of the external engines).
pub fn write_wav(path: &std::path::Path, pcm: &[f32]) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    for s in pcm {
        w.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    w.finalize()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ./dev.sh test -- --ignored capture_latency --nocapture
    #[test]
    #[ignore]
    fn capture_latency() {
        let r = Recorder::start().expect("recorder");
        let started = std::time::Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(3000));
        let wall = started.elapsed().as_secs_f32();
        let pcm = r.finish();
        let got = pcm.len() as f32 / TARGET_RATE as f32;
        println!("wall {wall:.3} s, captured {got:.3} s, lost {:.0} ms", (wall - got) * 1000.0);
    }

    #[test]
    fn downmix_averages_channels() {
        assert_eq!(downmix(&[0.0, 1.0, 1.0, 3.0], 2), vec![0.5, 2.0]);
    }

    fn tone(len: usize, amp: f32) -> Vec<f32> {
        (0..len).map(|i| amp * (i as f32 * 0.2).sin()).collect()
    }

    #[test]
    fn trim_silence_rejects_silence() {
        assert!(trim_silence(&vec![0.0; TARGET_RATE as usize]).is_none());
    }

    #[test]
    fn trim_silence_keeps_speech_and_drops_the_rest() {
        let mut pcm = vec![0.0; TARGET_RATE as usize];
        let speech_at = TARGET_RATE as usize / 2;
        pcm.splice(speech_at..speech_at + 1600, tone(1600, 0.3));
        let out = trim_silence(&pcm).expect("speech detected");
        // 0.1 s of speech framed by 2 x 0.2 s of margin: the rest of the
        // silence is dropped.
        assert_eq!(out.len(), 1600 + 2 * VAD_MARGIN_WINDOWS * VAD_WINDOW);
    }

    #[test]
    fn normalize_amplifies_a_quiet_signal() {
        let out = normalize(&tone(1000, 0.1));
        let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.7, "peak {peak}");
    }

    #[test]
    fn resample_halves_length() {
        let input: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let out = resample(&input, 32_000, 16_000);
        assert_eq!(out.len(), 50);
        assert!((out[10] - 20.0).abs() < 1e-3);
    }
}
