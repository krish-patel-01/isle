// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! Dictation, as LocalFlow does it: hold a key, speak, let go, and the words are typed where the cursor is. The key
//! with Space keeps listening hands-free until pressed again. The island shows the microphone's wave meanwhile,
//! then the words typed. Speech is turned into text on this computer (speech.rs); nothing leaves it.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use regex::Regex;

use super::Voice;
use super::mic::{Mic, RATE};

/// How often the keys are read.
const TICK: Duration = Duration::from_millis(15);
/// Shorter than this is a tap, not dictation.
const MIN_SECONDS: f32 = 0.3;
/// How long the island says the words were typed, or what went wrong.
const DONE_MS: u64 = 900;
const ERROR_MS: u64 = 2200;
/// Some apps (browsers, Electron) read the clipboard late: what was on it goes back only after this.
const RESTORE_AFTER: Duration = Duration::from_secs(1);

/// What Settings say.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub on: bool,
    /// "right-ctrl", "right-alt" or "ctrl-win".
    pub key: String,
    pub hands_free: bool,
    /// "paste" (the clipboard and Ctrl+V, put back after) or "type" (each character as a key press).
    pub insert: String,
    pub fillers: bool,
}

/// The keys that start dictation, each group any of its keys.
fn hold_keys(key: &str) -> Vec<Vec<u16>> {
    match key {
        "right-alt" => vec![vec![0xA5]],
        "ctrl-win" => vec![vec![0x11], vec![0x5B, 0x5C]],
        _ => vec![vec![0xA3]],
    }
}

/// The hold keys with Space: hands-free.
fn toggle_keys(key: &str) -> Vec<Vec<u16>> {
    let mut keys = hold_keys(key);
    keys.push(vec![0x20]);
    keys
}

/// Left and right modifiers also light up their generic code (Right Ctrl, Ctrl).
const GENERIC: [(u16, u16); 6] = [(0xA0, 0x10), (0xA1, 0x10), (0xA2, 0x11), (0xA3, 0x11), (0xA4, 0x12), (0xA5, 0x12)];

/// The keys of the combos (and their generic codes): any other held means a shortcut, not dictation.
fn allowed(combos: &[&[Vec<u16>]]) -> Vec<u16> {
    let mut allowed = Vec::new();
    for groups in combos {
        for vk in groups.iter().flatten() {
            allowed.push(*vk);
            if let Some((_, generic)) = GENERIC.iter().find(|(specific, _)| specific == vk) {
                allowed.push(*generic);
            }
        }
    }
    allowed
}

/// A job for the worker: the audio, and the app it was spoken into.
struct Job {
    audio: Vec<f32>,
    config: Config,
}

/// Reads the keys for good, dictating as Settings say.
pub fn run(voice: Arc<Voice>, config: impl Fn() -> Config) {
    let (jobs, inbox) = channel::<Job>();
    {
        let voice = voice.clone();
        std::thread::Builder::new()
            .name("isle-dictation".into())
            .spawn(move || work(&voice, inbox))
            .expect("could not start dictation's worker");
    }
    let mut state = State::default();
    let mut config_read = Instant::now() - Duration::from_secs(1);
    let mut settings = config();
    let mut level_tick = 0u32;
    loop {
        std::thread::sleep(TICK);
        if config_read.elapsed() >= Duration::from_millis(500) {
            config_read = Instant::now();
            settings = config();
        }
        if !settings.on {
            if state.mic.is_some() {
                state.cancel(&voice);
            }
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        state.tick(&voice, &settings, &jobs);
        // The wave: the level about 30 times a second.
        level_tick += 1;
        if let Some(mic) = state.mic.as_ref()
            && level_tick % 2 == 0
        {
            voice.level(mic.level());
        }
    }
}

#[derive(Default)]
struct State {
    mic: Option<Mic>,
    hands_free: bool,
    hold_was_down: bool,
    toggle_was_down: bool,
    /// The key was pressed without the model: said once per press.
    told_missing: bool,
}

impl State {
    fn tick(&mut self, voice: &Arc<Voice>, config: &Config, jobs: &Sender<Job>) {
        let hold = hold_keys(&config.key);
        let toggle = toggle_keys(&config.key);
        let allowed = allowed(&[&hold, &toggle]);
        let hold_down = keys::combo_down(&hold);
        let toggle_down = config.hands_free && keys::combo_down(&toggle);

        if toggle_down && !self.toggle_was_down {
            if self.hands_free {
                self.hands_free = false;
                self.stop(voice, config, jobs);
            } else {
                self.hands_free = true;
                // The hold keys are part of the toggle: already listening, most likely.
                if self.mic.is_none() {
                    self.start(voice);
                }
                if self.mic.is_some() {
                    voice.show("dictation", "listening", "Hands-free", true);
                } else {
                    self.hands_free = false;
                }
            }
        }
        self.toggle_was_down = toggle_down;

        if !self.hands_free {
            if hold_down && self.mic.is_none() {
                // Only a clean press: not while typing a shortcut.
                if !self.hold_was_down && !keys::other_down(&allowed) {
                    self.start(voice);
                }
            } else if hold_down && self.mic.is_some() && keys::other_down(&allowed) {
                // Right Ctrl + C: a shortcut, not dictation.
                self.cancel(voice);
            } else if !hold_down && self.mic.is_some() {
                self.stop(voice, config, jobs);
            }
        }
        if !hold_down {
            self.told_missing = false;
        }
        self.hold_was_down = hold_down;
    }

    fn start(&mut self, voice: &Arc<Voice>) {
        if !voice.speech.ready() {
            if !self.told_missing {
                self.told_missing = true;
                voice.flash("dictation", "error", "Get the speech model in Settings", ERROR_MS);
            }
            return;
        }
        if !voice.take_mic("dictation") {
            return;
        }
        match Mic::start(None) {
            Ok(mic) => {
                self.mic = Some(mic);
                voice.show("dictation", "listening", "", false);
            }
            Err(err) => {
                voice.give_mic();
                log::warn!(target: "isle::voice", "the microphone would not open: {err:#}");
                voice.flash("dictation", "error", "The microphone would not open", ERROR_MS);
            }
        }
    }

    fn stop(&mut self, voice: &Arc<Voice>, config: &Config, jobs: &Sender<Job>) {
        let Some(mic) = self.mic.take() else { return };
        let audio = mic.stop();
        voice.give_mic();
        if (audio.len() as f32) / (RATE as f32) < MIN_SECONDS {
            voice.hide("dictation");
            return;
        }
        voice.show("dictation", "working", "", false);
        let _ = jobs.send(Job { audio, config: config.clone() });
    }

    fn cancel(&mut self, voice: &Arc<Voice>) {
        if let Some(mic) = self.mic.take() {
            drop(mic.stop());
            voice.give_mic();
        }
        self.hands_free = false;
        voice.hide("dictation");
    }
}

/// Transcribes, cleans up and types each recording, off the keys' thread.
fn work(voice: &Arc<Voice>, inbox: Receiver<Job>) {
    while let Ok(Job { audio, config }) = inbox.recv() {
        let started = Instant::now();
        let raw = match voice.speech.engine() {
            Ok(engine) => engine.transcribe(&audio),
            Err(err) => {
                log::warn!(target: "isle::voice", "no speech model: {err:#}");
                voice.flash("dictation", "error", "The speech model would not load", ERROR_MS);
                continue;
            }
        };
        let text = clean(&raw, config.fillers);
        let seconds = audio.len() as f32 / RATE as f32;
        log::info!(target: "isle::voice", "dictated {seconds:.1}s in {:?}: {} words", started.elapsed(), text.split_whitespace().count());
        if text.is_empty() {
            voice.flash("dictation", "error", "No speech heard", 1200);
            continue;
        }
        let typed = if config.insert == "type" { insert::type_text(&text) } else { insert::paste(&text) };
        if typed {
            let words = text.split_whitespace().count();
            let said = if words == 1 { "Typed 1 word".to_owned() } else { format!("Typed {words} words") };
            voice.flash("dictation", "done", &said, DONE_MS);
        } else {
            voice.flash("dictation", "error", "Couldn’t type into that app", ERROR_MS);
        }
    }
}

/// Pastes `text` where the cursor is (the assistant's `write`).
pub fn paste_text(text: &str) -> bool {
    insert::paste(text)
}

/// Spoken commands, written.
const REPLACEMENTS: [(&str, &str); 2] = [("new paragraph", "\n\n"), ("new line", "\n")];

/// Raw speech to typed text (LocalFlow's cleanup.py rules): fillers out, spoken commands written, spaces tidied,
/// each sentence capitalised.
pub fn clean(text: &str, fillers: bool) -> String {
    use std::sync::LazyLock;
    static FILLERS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)(^|[^\w'])(?:um+|uh+|erm+|uhm+|hmm+)\b[,.]?\s*").unwrap());
    // Parakeet sometimes writes a spoken "uh" as a lone "U" mid-sentence ("on Friday U after lunch").
    static LONE_U: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\w),?\s+[Uu],?(\s+[a-z])").unwrap());
    static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]{2,}").unwrap());
    static SPACE_AT_BREAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]*\n[ \t]*").unwrap());
    static SENTENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(^|\n|[.!?]\s+)([a-z])").unwrap());
    static ABBREVIATION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?:\b\w\.|\b(?:Dr|Mr|Mrs|Ms|vs|etc|approx)\.)$").unwrap());

    let mut text = text.to_owned();
    if fillers {
        text = FILLERS.replace_all(&text, "$1").into_owned();
        text = LONE_U.replace_all(&text, "$1$2").into_owned();
    }
    for (spoken, written) in REPLACEMENTS {
        // Commands often come with punctuation around them ("New line."): it goes too.
        let pattern = Regex::new(&format!(r"(?i)[,.]?\s*\b{}\b[,.]?\s*", regex::escape(spoken))).unwrap();
        text = pattern.replace_all(&text, regex::NoExpand(written)).into_owned();
    }
    text = SPACES.replace_all(&text, " ").into_owned();
    text = SPACE_AT_BREAK.replace_all(&text, "\n").into_owned();
    let text = text.trim().to_owned();
    SENTENCE
        .replace_all(&text, |caps: &regex::Captures| {
            let whole = caps.get(0).unwrap();
            let before = text[..whole.start() + caps[1].len().min(1)].trim_end();
            if !caps[1].is_empty() && ABBREVIATION.is_match(before) {
                return whole.as_str().to_owned();
            }
            format!("{}{}", &caps[1], caps[2].to_uppercase())
        })
        .into_owned()
}

/// Reading the keyboard's state as it is now, for any app in front.
mod keys {
    /// Real keyboard keys only (no mouse buttons, no reserved or IME codes).
    const SCAN: [std::ops::Range<u16>; 5] = [0x08..0x5E, 0x60..0x88, 0xA0..0xA6, 0xBA..0xE3, 0..0];

    pub fn down(vk: u16) -> bool {
        #[cfg(windows)]
        {
            use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
            // SAFETY: a plain query of a key's state.
            (unsafe { GetAsyncKeyState(i32::from(vk)) } as u16 & 0x8000) != 0
        }
        #[cfg(not(windows))]
        {
            let _ = vk;
            false
        }
    }

    pub fn combo_down(groups: &[Vec<u16>]) -> bool {
        groups.iter().all(|group| group.iter().any(|&vk| down(vk)))
    }

    /// Whether a key outside the hotkey is held (a shortcut being typed).
    pub fn other_down(allowed: &[u16]) -> bool {
        SCAN.iter().cloned().flatten().any(|vk| !allowed.contains(&vk) && down(vk))
    }
}

/// Putting the words where the cursor is.
mod insert {
    use std::time::{Duration, Instant};

    use crate::clipboard::{Copied, imp};

    /// Ctrl, Shift, Alt (generic and each side) and both Windows keys.
    const MODIFIERS: [u16; 11] = [0x10, 0x11, 0x12, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0x5B, 0x5C];

    /// Waits for the modifiers to be let go (Ctrl+V must not be Ctrl+Win+V); still held after two seconds (a stuck
    /// key), they are let go for the app.
    fn wait_for_modifiers() {
        let end = Instant::now() + Duration::from_secs(2);
        while Instant::now() < end {
            if !MODIFIERS.iter().any(|&vk| super::keys::down(vk)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        send(&MODIFIERS.iter().filter(|&&vk| super::keys::down(vk)).map(|&vk| key(vk, 0, true)).collect::<Vec<_>>());
    }

    #[cfg(windows)]
    type Input = windows::Win32::UI::Input::KeyboardAndMouse::INPUT;
    #[cfg(not(windows))]
    type Input = ();

    #[cfg(windows)]
    fn key(vk: u16, scan: u16, up: bool) -> Input {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY,
        };
        let mut flags = if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) };
        if vk == 0 {
            flags |= KEYEVENTF_UNICODE;
        }
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, ..Default::default() } },
        }
    }
    #[cfg(not(windows))]
    fn key(_vk: u16, _scan: u16, _up: bool) -> Input {}

    /// Presses the keys; false when Windows blocked some (an app run as administrator).
    fn send(inputs: &[Input]) -> bool {
        #[cfg(windows)]
        {
            use windows::Win32::UI::Input::KeyboardAndMouse::SendInput;
            if inputs.is_empty() {
                return true;
            }
            // SAFETY: synthetic key presses, sized as Windows takes them.
            let sent = unsafe { SendInput(inputs, size_of::<Input>() as i32) };
            sent as usize == inputs.len()
        }
        #[cfg(not(windows))]
        {
            let _ = inputs;
            false
        }
    }

    /// Pastes `text` with the clipboard and Ctrl+V, then puts back what was on it, unless something newer is.
    pub fn paste(text: &str) -> bool {
        let previous = imp::read();
        wait_for_modifiers();
        let mut set = false;
        for attempt in 1..=10u64 {
            if imp::set_text_passing(text) {
                set = true;
                break;
            }
            // Another app holds the clipboard.
            std::thread::sleep(Duration::from_millis(50 * attempt));
        }
        if !set {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
        const VK_CONTROL: u16 = 0x11;
        const VK_V: u16 = 0x56;
        let pressed = send(&[key(VK_CONTROL, 0, false), key(VK_V, 0, false), key(VK_V, 0, true), key(VK_CONTROL, 0, true)]);
        let text = text.to_owned();
        std::thread::spawn(move || {
            std::thread::sleep(super::RESTORE_AFTER);
            if !matches!(imp::read(), Copied::Text(now) if now == text) {
                return;
            }
            match previous {
                Copied::Text(previous) => {
                    imp::set_text(&previous);
                }
                Copied::Files(paths) => {
                    imp::set_files(&paths);
                }
                Copied::Image(png) => {
                    imp::set_image(&png);
                }
                // Nothing to put back that Isle can: the words stay.
                _ => {}
            }
        });
        pressed
    }

    /// Types `text` a character at a time, never touching the clipboard.
    pub fn type_text(text: &str) -> bool {
        wait_for_modifiers();
        const VK_RETURN: u16 = 0x0D;
        for ch in text.chars() {
            let keys: Vec<Input> = if ch == '\n' {
                vec![key(VK_RETURN, 0, false), key(VK_RETURN, 0, true)]
            } else {
                let mut units = [0u16; 2];
                ch.encode_utf16(&mut units).iter().flat_map(|&unit| [key(0, unit, false), key(0, unit, true)]).collect()
            };
            if !send(&keys) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn fillers_and_commands_are_cleaned() {
        assert_eq!(clean("um, so I think uh we should go", true), "So I think we should go");
        assert_eq!(clean("first point. new line. second point", true), "First point\nSecond point");
        assert_eq!(clean("hello new paragraph world", true), "Hello\n\nWorld");
        assert_eq!(clean("on Friday U after lunch", true), "On Friday after lunch");
        assert_eq!(clean("we met Dr. smith. then left", true), "We met Dr. smith. Then left");
        assert_eq!(clean("I live in the U.S. now", true), "I live in the U.S. now");
        assert_eq!(clean("hmm", true), "");
        // What Parakeet heard in LocalFlow's sample recording.
        assert_eq!(
            clean("Um, hey team, the build is green New line Let us ship the release on Friday U after lunch", true),
            "Hey team, the build is green\nLet us ship the release on Friday after lunch"
        );
    }
}
