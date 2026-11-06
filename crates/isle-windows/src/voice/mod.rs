// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! Voice: dictation (typing what is said) and, sharing its microphone and speech model, the assistant. The island
//! shows what either is doing as the `voice` state ({mode, phase, text, handsFree}), with the microphone's level as
//! `voiceLevel` signals for its wave.

pub mod dictation;
pub mod mic;
pub mod speech;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use self::speech::Speech;

/// Tells the island: an event name and its payload.
pub type Emit = Arc<dyn Fn(&str, Value) + Send + Sync>;

pub struct Voice {
    pub speech: Arc<Speech>,
    emit: Emit,
    shown: Mutex<(Value, u64)>,
    /// Who is listening, dictation or the assistant: one at a time.
    mic: Mutex<Option<&'static str>>,
}

impl Voice {
    pub fn new(emit: Emit) -> Self {
        Self { speech: Arc::new(Speech::new()), emit, shown: Mutex::new((Value::Null, 0)), mic: Mutex::new(None) }
    }

    /// What the island shows now (for a page loading).
    pub fn state(&self) -> Value {
        self.shown.lock().unwrap().0.clone()
    }

    /// Shows `mode` ("dictation" or "assistant") in `phase` ("listening", "working", "done", "error").
    pub fn show(&self, mode: &str, phase: &str, text: &str, hands_free: bool) {
        let value = json!({"mode": mode, "phase": phase, "text": text, "handsFree": hands_free});
        {
            let mut shown = self.shown.lock().unwrap();
            shown.0 = value.clone();
            shown.1 += 1;
        }
        (self.emit)("voice", value);
    }

    /// Shows `phase` for `ms`, then nothing, unless something else showed meanwhile.
    pub fn flash(self: &Arc<Self>, mode: &str, phase: &str, text: &str, ms: u64) {
        self.show(mode, phase, text, false);
        let generation = self.shown.lock().unwrap().1;
        let voice = self.clone();
        let mode = mode.to_owned();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            if voice.shown.lock().unwrap().1 == generation {
                voice.hide(&mode);
            }
        });
    }

    /// Shows nothing, if `mode` is what shows.
    pub fn hide(&self, mode: &str) {
        {
            let mut shown = self.shown.lock().unwrap();
            if shown.0.get("mode").and_then(Value::as_str) != Some(mode) {
                return;
            }
            shown.0 = Value::Null;
            shown.1 += 1;
        }
        (self.emit)("voice", Value::Null);
    }

    /// The microphone's level for the wave, 0 to about 1.
    pub fn level(&self, level: f32) {
        (self.emit)("voiceLevel", json!(level));
    }

    /// Takes the microphone for `who`; false while the other has it.
    pub fn take_mic(&self, who: &'static str) -> bool {
        let mut mic = self.mic.lock().unwrap();
        if mic.is_some_and(|owner| owner != who) {
            return false;
        }
        *mic = Some(who);
        true
    }

    pub fn give_mic(&self) {
        *self.mic.lock().unwrap() = None;
    }

    /// Tells Settings how the speech model is.
    pub fn model_changed(&self) {
        (self.emit)("speechModel", serde_json::to_value(self.speech.model()).unwrap_or(Value::Null));
    }
}
