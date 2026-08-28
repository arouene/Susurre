use crate::config::{data_dir, Engine};
use anyhow::{anyhow, bail, Context, Result};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub struct ModelInfo {
    pub id: &'static str,
    pub label: &'static str,
    /// Approximate size on disk, in MB (whisper.cpp ggml weights; the
    /// CTranslate2 int8 conversion is roughly half of it).
    pub size_mb: u32,
    /// Hugging Face repository holding the CTranslate2 conversion. Not every
    /// model has one under Systran, so it is spelled out per entry.
    pub ct2_repo: &'static str,
}

macro_rules! m {
    ($id:literal, $label:literal, $mb:literal, $repo:literal) => {
        ModelInfo { id: $id, label: $label, size_mb: $mb, ct2_repo: $repo }
    };
}

pub const CATALOG: &[ModelInfo] = &[
    m!("tiny", "Tiny", 75, "Systran/faster-whisper-tiny"),
    m!("base", "Base", 142, "Systran/faster-whisper-base"),
    m!("small", "Small", 466, "Systran/faster-whisper-small"),
    m!("medium", "Medium", 1500, "Systran/faster-whisper-medium"),
    m!("large-v3", "Large v3", 3100, "Systran/faster-whisper-large-v3"),
    // Systran never published a turbo conversion; this is the reference
    // community one.
    m!("large-v3-turbo", "Large v3 Turbo", 1600, "deepdml/faster-whisper-large-v3-turbo-ct2"),
];

/// Files a CTranslate2 repository must provide for faster-whisper to load it.
const CT2_REQUIRED: &[&str] = &["config.json", "model.bin", "tokenizer.json"];
/// Files only some repositories carry: `preprocessor_config.json` is absent
/// from the smaller Systran conversions, and the vocabulary is `.json` on some
/// repositories and `.txt` on others. A 404 on these is not a failure.
const CT2_OPTIONAL: &[&str] = &["preprocessor_config.json", "vocabulary.json", "vocabulary.txt"];

pub fn info(id: &str) -> Result<&'static ModelInfo> {
    CATALOG
        .iter()
        .find(|m| m.id == id)
        .ok_or_else(|| anyhow!("unknown model {id}"))
}

pub fn models_root() -> PathBuf {
    data_dir().join("models")
}

/// Model location: a .bin file for whisper.cpp, a directory for CTranslate2.
pub fn path(engine: Engine, id: &str) -> PathBuf {
    let root = models_root().join(engine.slug());
    match engine {
        Engine::WhisperCpp => root.join(format!("ggml-{id}.bin")),
        Engine::FasterWhisper => root.join(id),
    }
}

pub fn is_installed(engine: Engine, id: &str) -> bool {
    complete(engine, &path(engine, id))
}

/// A model is only usable once every required file is there. Downloads land in
/// a staging path and are moved into place as a whole, so a half-downloaded
/// model never reports itself as installed.
fn complete(engine: Engine, root: &Path) -> bool {
    match engine {
        Engine::WhisperCpp => root.is_file(),
        Engine::FasterWhisper => {
            CT2_REQUIRED.iter().all(|f| root.join(f).is_file())
                && ["vocabulary.json", "vocabulary.txt"]
                    .iter()
                    .any(|f| root.join(f).is_file())
        }
    }
}

/// `(url, destination inside the staging path, required)`.
fn downloads(engine: Engine, info: &ModelInfo, staging: &Path) -> Vec<(String, PathBuf, bool)> {
    match engine {
        Engine::WhisperCpp => vec![(
            format!(
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{}.bin?download=true",
                info.id
            ),
            staging.to_path_buf(),
            true,
        )],
        Engine::FasterWhisper => CT2_REQUIRED
            .iter()
            .map(|f| (*f, true))
            .chain(CT2_OPTIONAL.iter().map(|f| (*f, false)))
            .map(|(f, required)| {
                (
                    format!(
                        "https://huggingface.co/{}/resolve/main/{f}?download=true",
                        info.ct2_repo
                    ),
                    staging.join(f),
                    required,
                )
            })
            .collect(),
    }
}

fn staging_path(dest: &Path) -> PathBuf {
    let name = dest.file_name().unwrap().to_string_lossy().into_owned();
    dest.with_file_name(format!("{name}.part"))
}

fn remove(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else if path.exists() {
        std::fs::remove_file(path)
    } else {
        Ok(())
    }
}

/// Downloads the model. `progress` gets a fraction between 0.0 and 1.0.
/// Nothing is published under the final path until every required file has
/// been fetched.
pub fn download(engine: Engine, id: &str, progress: &dyn Fn(f64)) -> Result<()> {
    let info = info(id)?;
    let dest = path(engine, id);
    let staging = staging_path(&dest);
    let _ = remove(&staging);

    let files = downloads(engine, info, &staging);
    let total = files.len() as f64;
    for (i, (url, out_path, required)) in files.iter().enumerate() {
        let base = i as f64 / total;
        progress(base);
        let res = match ureq::get(url).call() {
            Ok(r) => r,
            Err(ureq::Error::Status(404, _)) if !required => continue,
            Err(e) => {
                let _ = remove(&staging);
                return Err(e).with_context(|| format!("downloading {url}"));
            }
        };
        let fetch = fetch(res, out_path, &|f| progress(base + f / total));
        if let Err(e) = fetch {
            let _ = remove(&staging);
            return Err(e).with_context(|| format!("downloading {url}"));
        }
    }

    if !complete(engine, &staging) {
        let _ = remove(&staging);
        bail!("{} is missing files in {}", info.label, info.ct2_repo);
    }
    remove(&dest)?;
    std::fs::rename(&staging, &dest).with_context(|| format!("installing {dest:?}"))?;
    progress(1.0);
    Ok(())
}

fn fetch(res: ureq::Response, dest: &Path, progress: &dyn Fn(f64)) -> Result<()> {
    std::fs::create_dir_all(dest.parent().unwrap())?;
    let len: u64 = res
        .header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut out = std::fs::File::create(dest)?;
    let mut reader = res.into_reader();
    let mut buf = vec![0u8; 256 * 1024];
    let mut done: u64 = 0;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        done += n as u64;
        if len > 0 {
            progress(done as f64 / len as f64);
        }
    }
    out.flush()?;
    Ok(())
}

pub fn delete(engine: Engine, id: &str) -> Result<()> {
    let p = path(engine, id);
    if !p.exists() {
        bail!("model {id} is not installed");
    }
    remove(&p)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_entry_has_a_ct2_repository() {
        for m in CATALOG {
            assert!(m.ct2_repo.contains('/'), "{}: {:?}", m.id, m.ct2_repo);
        }
    }

    #[test]
    fn staging_is_a_sibling_of_the_destination() {
        assert_eq!(staging_path(Path::new("/m/small")), Path::new("/m/small.part"));
        assert_eq!(
            staging_path(Path::new("/m/ggml-small.bin")),
            Path::new("/m/ggml-small.bin.part")
        );
    }

    /// Real download against Hugging Face. Guards the reported failure:
    /// `preprocessor_config.json` is absent from the smaller Systran
    /// conversions, and a 404 there used to abort after `model.bin` had
    /// already landed, leaving a directory that passed for an installed model.
    /// Ignored by default because it pulls ~75 MB.
    #[test]
    #[ignore]
    fn downloads_a_ct2_model_end_to_end() {
        let _ = delete(Engine::FasterWhisper, "tiny");
        download(Engine::FasterWhisper, "tiny", &|_| {}).expect("download");
        assert!(is_installed(Engine::FasterWhisper, "tiny"));
        assert!(!staging_path(&path(Engine::FasterWhisper, "tiny")).exists());
        delete(Engine::FasterWhisper, "tiny").unwrap();
    }

    /// The bug this guards: `model.bin` lands, a later optional file 404s, and
    /// the half-written directory used to pass for an installed model.
    #[test]
    fn a_ct2_directory_without_a_vocabulary_is_not_complete() {
        let dir = std::env::temp_dir().join("susurre-test-incomplete");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in CT2_REQUIRED {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        assert!(!complete(Engine::FasterWhisper, &dir));
        std::fs::write(dir.join("vocabulary.txt"), b"x").unwrap();
        assert!(complete(Engine::FasterWhisper, &dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
