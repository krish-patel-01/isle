// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! The microphone: Windows' default one, 16 kHz mono, as the speech model takes it (Windows converts the rate
//! and the channels). Recorded on a thread of its own until stopped; each chunk can also be handed on live.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;

pub const RATE: u32 = 16_000;

/// Each chunk as it comes (for the assistant's pause detection).
pub type OnChunk = Box<dyn FnMut(&[f32]) + Send>;

/// A recording in progress.
pub struct Mic {
    stop: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
    thread: Option<JoinHandle<Vec<f32>>>,
}

impl Mic {
    /// Starts recording, or says why the microphone could not be opened.
    pub fn start(on_chunk: Option<OnChunk>) -> anyhow::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU32::new(0));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let thread = {
            let (stop, level) = (stop.clone(), level.clone());
            std::thread::Builder::new()
                .name("isle-mic".into())
                .spawn(move || imp::record(&stop, &level, on_chunk, ready_tx))?
        };
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { stop, level, thread: Some(thread) }),
            Ok(Err(err)) => {
                let _ = thread.join();
                Err(err)
            }
            Err(_) => anyhow::bail!("the microphone thread stopped"),
        }
    }

    /// How loud it is now, 0 to about 1 (the root mean square of the last chunk).
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    /// Stops recording and gives back all that was heard.
    pub fn stop(mut self) -> Vec<f32> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().and_then(|thread| thread.join().ok()).unwrap_or_default()
    }
}

impl Drop for Mic {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The root mean square of `samples`.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::mpsc::Sender;
    use std::time::Duration;

    use anyhow::Context;
    use windows::Win32::Media::Audio::{
        AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
        AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
        WAVEFORMATEX, eCapture, eConsole,
    };
    use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};

    use super::{OnChunk, RATE, rms};

    /// 32-bit float samples.
    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
    /// Windows' buffer: room for 200 ms, in 100 ns units.
    const BUFFER: i64 = 2_000_000;

    struct Client {
        audio: IAudioClient,
        capture: IAudioCaptureClient,
    }

    fn open() -> anyhow::Result<Client> {
        // SAFETY: plain COM calls on this thread's apartment, with a format that lives through Initialize.
        unsafe {
            let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = devices.GetDefaultAudioEndpoint(eCapture, eConsole).context("no microphone")?;
            let audio: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let format = WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
                nChannels: 1,
                nSamplesPerSec: RATE,
                nAvgBytesPerSec: RATE * 4,
                nBlockAlign: 4,
                wBitsPerSample: 32,
                cbSize: 0,
            };
            audio
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                    BUFFER,
                    0,
                    &format,
                    None,
                )
                .context("the microphone would not open")?;
            let capture: IAudioCaptureClient = audio.GetService()?;
            audio.Start()?;
            Ok(Client { audio, capture })
        }
    }

    pub fn record(
        stop: &AtomicBool,
        level: &AtomicU32,
        mut on_chunk: Option<OnChunk>,
        ready: Sender<anyhow::Result<()>>,
    ) -> Vec<f32> {
        // SAFETY: this thread's own apartment, left again before it ends.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let mut heard = Vec::new();
        match open() {
            Err(err) => {
                let _ = ready.send(Err(err));
            }
            Ok(client) => {
                let _ = ready.send(Ok(()));
                let mut chunk = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(10));
                    chunk.clear();
                    // SAFETY: each buffer Windows hands out is read for the frames it says, then given back.
                    unsafe {
                        while client.capture.GetNextPacketSize().unwrap_or(0) > 0 {
                            let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
                            if client.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None).is_err() {
                                break;
                            }
                            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                                chunk.extend(std::iter::repeat_n(0.0, frames as usize));
                            } else {
                                chunk.extend_from_slice(std::slice::from_raw_parts(data as *const f32, frames as usize));
                            }
                            let _ = client.capture.ReleaseBuffer(frames);
                        }
                    }
                    if chunk.is_empty() {
                        continue;
                    }
                    level.store(rms(&chunk).to_bits(), Ordering::Relaxed);
                    if let Some(on_chunk) = on_chunk.as_mut() {
                        on_chunk(&chunk);
                    }
                    heard.extend_from_slice(&chunk);
                }
                // SAFETY: stops the stream opened above.
                let _ = unsafe { client.audio.Stop() };
            }
        }
        level.store(0, Ordering::Relaxed);
        // SAFETY: pairs the CoInitializeEx above.
        unsafe { CoUninitialize() };
        heard
    }
}

#[cfg(not(windows))]
mod imp {
    use std::sync::atomic::{AtomicBool, AtomicU32};
    use std::sync::mpsc::Sender;

    use super::OnChunk;

    pub fn record(_stop: &AtomicBool, _level: &AtomicU32, _on_chunk: Option<OnChunk>, ready: Sender<anyhow::Result<()>>) -> Vec<f32> {
        let _ = ready.send(Err(anyhow::anyhow!("only on Windows")));
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    /// Records three seconds and says how loud it was: `cargo test --release mic -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn records() {
        let mic = super::Mic::start(None).expect("the microphone");
        let seconds = std::env::var("ISLE_MIC_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(3);
        std::thread::sleep(std::time::Duration::from_secs(seconds));
        let heard = mic.stop();
        let peak = heard.iter().fold(0f32, |m, s| m.max(s.abs()));
        let zeros = heard.iter().filter(|s| **s == 0.0).count();
        println!("{} samples, rms {:.5}, peak {:.5}, zeros {}", heard.len(), super::rms(&heard), peak, zeros);
        // Each 50 ms chunk's level, as the assistant's pause detector sees them.
        let chunks: Vec<String> = heard.chunks(super::RATE as usize / 20).map(|c| format!("{:.4}", super::rms(c))).collect();
        println!("chunks {}", chunks.join(" "));
    }
}
