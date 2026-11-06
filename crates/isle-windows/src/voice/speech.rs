// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! Speech to text on this computer: NVIDIA's Parakeet TDT 110M (English, 8-bit) through sherpa-onnx, as LocalFlow
//! runs it. About a tenth of a second for a sentence on the processor; nothing leaves the computer. The model
//! (108 MB to download, 131 MB unpacked) is fetched once, from sherpa-onnx's own releases, when asked in Settings;
//! one LocalFlow already has is used as it is.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use serde::Serialize;

/// The model's folder, as its archive unpacks.
const MODEL: &str = "sherpa-onnx-nemo-parakeet_tdt_transducer_110m-en-36000-int8";
const URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/\
                   sherpa-onnx-nemo-parakeet_tdt_transducer_110m-en-36000-int8.tar.bz2";
const NEEDED: [&str; 4] = ["encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"];
/// Two threads: as fast as four for a sentence, and lighter (LocalFlow's measure).
const THREADS: i32 = 2;

/// Where the model is, or would go.
fn own_dir() -> PathBuf {
    crate::store::data_dir().join("models").join(MODEL)
}

fn complete(dir: &Path) -> bool {
    NEEDED.iter().all(|file| dir.join(file).is_file())
}

/// The model's folder, Isle's own or LocalFlow's.
pub fn model_dir() -> Option<PathBuf> {
    let localflow = dirs::data_local_dir().map(|dir| dir.join("LocalFlow").join("models").join(MODEL));
    std::iter::once(own_dir()).chain(localflow).find(|dir| complete(dir))
}

/// What Settings shows of the model.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct ModelState {
    /// "missing", "downloading", "ready" or "failed".
    pub state: String,
    /// While downloading, 0 to 1.
    pub progress: f64,
    pub error: String,
}

/// The loaded model, loaded on first use and kept, with what Settings shows of it.
pub struct Speech {
    engine: Mutex<Option<Arc<Engine>>>,
    model: Mutex<ModelState>,
}

impl Speech {
    pub fn new() -> Self {
        let state = if model_dir().is_some() { "ready" } else { "missing" };
        Self { engine: Mutex::new(None), model: Mutex::new(ModelState { state: state.into(), ..Default::default() }) }
    }

    pub fn model(&self) -> ModelState {
        self.model.lock().unwrap().clone()
    }

    pub fn ready(&self) -> bool {
        self.model.lock().unwrap().state == "ready"
    }

    /// The engine, loading it the first time (about a second).
    pub fn engine(&self) -> anyhow::Result<Arc<Engine>> {
        let mut engine = self.engine.lock().unwrap();
        if let Some(engine) = engine.as_ref() {
            return Ok(engine.clone());
        }
        let dir = model_dir().context("the speech model is not downloaded")?;
        let started = std::time::Instant::now();
        let loaded = Arc::new(Engine::load(&dir)?);
        log::info!(target: "isle::voice", "speech model loaded from {} in {:?}", dir.display(), started.elapsed());
        *engine = Some(loaded.clone());
        Ok(loaded)
    }

    /// Downloads and unpacks the model, telling `changed` as it goes.
    pub fn download(&self, changed: impl Fn(&ModelState)) {
        let set = |state: &str, progress: f64, error: String| {
            let now = ModelState { state: state.into(), progress, error };
            *self.model.lock().unwrap() = now.clone();
            changed(&now);
        };
        {
            let model = self.model.lock().unwrap();
            if model.state == "downloading" || model.state == "ready" {
                return;
            }
        }
        set("downloading", 0.0, String::new());
        let mut last = 0.0;
        match fetch(|progress| {
            // A percent at a time is plenty.
            if progress - last >= 0.01 {
                last = progress;
                set("downloading", progress, String::new());
            }
        }) {
            Ok(()) => set("ready", 1.0, String::new()),
            Err(err) => {
                log::warn!(target: "isle::voice", "the speech model could not be downloaded: {err:#}");
                set("failed", 0.0, format!("{err:#}"));
            }
        }
    }
}

/// Downloads the archive beside where it goes, then unpacks it with Windows' own tar.
fn fetch(progress: impl FnMut(f64)) -> anyhow::Result<()> {
    let models = own_dir().parent().map(Path::to_path_buf).context("no folder for models")?;
    std::fs::create_dir_all(&models)?;
    let archive = models.join(format!("{MODEL}.tar.bz2"));
    download(URL, &archive, progress)?;
    let tar = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from).join(r"System32\tar.exe");
    let mut command = std::process::Command::new(tar);
    command.arg("-xjf").arg(&archive).arg("-C").arg(&models);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashing up.
        command.creation_flags(0x0800_0000);
    }
    let status = command.status().context("could not run tar")?;
    let _ = std::fs::remove_file(&archive);
    anyhow::ensure!(status.success(), "tar could not unpack the model ({status})");
    anyhow::ensure!(complete(&own_dir()), "the model's files were not all there");
    // The archive also has test recordings, not needed.
    let _ = std::fs::remove_dir_all(own_dir().join("test_wavs"));
    Ok(())
}

fn download(url: &str, to: &Path, mut progress: impl FnMut(f64)) -> anyhow::Result<()> {
    let response = ureq::get(url).call().context("could not reach GitHub")?;
    let total = response.headers().get("content-length").and_then(|value| value.to_str().ok()?.parse::<u64>().ok());
    let mut body = response.into_body().into_reader();
    let part = to.with_extension("part");
    let mut file = std::fs::File::create(&part)?;
    let (mut buffer, mut done) = (vec![0u8; 256 * 1024], 0u64);
    loop {
        let read = body.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])?;
        done += read as u64;
        if let Some(total) = total.filter(|&total| total > 0) {
            progress(done as f64 / total as f64);
        }
    }
    file.flush()?;
    drop(file);
    if let Some(total) = total {
        anyhow::ensure!(done == total, "the download stopped early");
    }
    std::fs::rename(&part, to)?;
    Ok(())
}

/// The model, ready to transcribe.
pub struct Engine {
    recognizer: sherpa_onnx::OfflineRecognizer,
}

impl Engine {
    fn load(dir: &Path) -> anyhow::Result<Self> {
        use sherpa_onnx::{OfflineModelConfig, OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig};
        let file = |name: &str| Some(dir.join(name).to_string_lossy().into_owned());
        let config = OfflineRecognizerConfig {
            model_config: OfflineModelConfig {
                transducer: OfflineTransducerModelConfig {
                    encoder: file("encoder.int8.onnx"),
                    decoder: file("decoder.int8.onnx"),
                    joiner: file("joiner.int8.onnx"),
                },
                tokens: file("tokens.txt"),
                num_threads: THREADS,
                provider: Some("cpu".into()),
                model_type: Some("nemo_transducer".into()),
                ..Default::default()
            },
            decoding_method: Some("greedy_search".into()),
            ..Default::default()
        };
        let recognizer = OfflineRecognizer::create(&config).context("the speech model would not load")?;
        Ok(Self { recognizer })
    }

    /// The words in `audio` (16 kHz mono).
    pub fn transcribe(&self, audio: &[f32]) -> String {
        if audio.is_empty() {
            return String::new();
        }
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(super::mic::RATE as i32, audio);
        self.recognizer.decode(&stream);
        stream.get_result().map(|result| result.text.trim().to_owned()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    /// LocalFlow's sample read back: `cargo test --release speech -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn transcribes_a_recording() {
        let speech = super::Speech::new();
        let engine = speech.engine().expect("the model");
        let wav = std::fs::read(r"K:\Projects\LocalFlow\localflow\test_audio\sample.wav").expect("the sample");
        // 16-bit PCM after a 44-byte header, 16 kHz mono.
        let samples: Vec<f32> = wav[44..].chunks_exact(2).map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32768.0).collect();
        let started = std::time::Instant::now();
        let text = engine.transcribe(&samples);
        println!("{:.1}s of audio in {:?}: {text}", samples.len() as f32 / 16000.0, started.elapsed());
        assert!(!text.is_empty());
    }
}
