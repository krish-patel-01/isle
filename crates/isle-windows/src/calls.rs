// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! A call in any app (WhatsApp, Teams, Zoom, Meet in a browser…), as the extension finds one on GNOME (call.js,
//! callServices.js): a call app, or a browser on a call's page, holding the microphone; the window it runs in, and
//! the microphone's mute. Where GNOME asks the sound server who records, Windows keeps the same in its privacy
//! settings (CapabilityAccessManager): each app that used the microphone, and whether it still does.
//!
//! The open island shows the call's window live, as GNOME's clone of it: a DWM thumbnail, drawn by the compositor
//! from what is on screen already, so nothing is captured.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

/// How often the microphone's users are looked at.
const EVERY: Duration = Duration::from_millis(700);
/// The microphone must be held this long to count: sites open it briefly to ask for permission.
const START_DELAY: Duration = Duration::from_secs(2);
/// A call outlasts the microphone closing this long, as apps reopen it to switch devices.
const END_DELAY: Duration = Duration::from_secs(4);
/// A window's title names the call's service only this early on, while it is the call's tab.
const SERVICE_GRACE: Duration = Duration::from_secs(15);

/// Call services named in window titles, for calls in a browser: (what to look for, its name).
const SERVICES: &[(&[Find], &str)] = &[
    (&[Find::Has("google meet"), Find::Word("meet")], "Google Meet"),
    (&[Find::Has("whatsapp")], "WhatsApp"),
    (&[Find::Has("telegram")], "Telegram"),
    (&[Find::Has("discord")], "Discord"),
    (&[Find::Has("zoom")], "Zoom"),
    (&[Find::Has("microsoft teams"), Find::Word("teams")], "Microsoft Teams"),
    (&[Find::Has("jitsi")], "Jitsi Meet"),
    (&[Find::Has("slack")], "Slack"),
    (&[Find::Has("messenger")], "Messenger"),
    (&[Find::Has("signal")], "Signal"),
    (&[Find::Has("skype")], "Skype"),
    (&[Find::Has("webex")], "Webex"),
];

/// Words in a title showing a call under way, not only the service (with a Meet code, see `in_call`).
const IN_CALL_WORDS: &[&str] = &["call", "calling", "meeting", "huddle", "panggilan", "rapat"];

/// Apps made for calls, by their names: holding the microphone, they are in one (or recording a voice note).
/// Anything else that holds it (dictation, a game's voice chat, a recorder) is the microphone in use, not a call.
const CALL_APPS: &[Find] = &[
    Find::Has("discord"), Find::Has("vesktop"), Find::Has("webcord"), Find::Has("legcord"), Find::Has("armcord"),
    Find::Has("zoom"), Find::Has("slack"), Find::Has("teams"), Find::Has("skype"), Find::Has("telegram"),
    Find::Has("signal"), Find::Has("whatsapp"), Find::Has("zapzap"), Find::Has("whatsie"), Find::Has("wasistlos"),
    Find::Word("element"), Find::Has("mumble"), Find::Has("teamspeak"), Find::Has("jitsi"), Find::Has("webex"),
    Find::Has("viber"), Find::Has("jami"), Find::Has("linphone"), Find::Has("ferdium"), Find::Has("rambox"),
    Find::Has("beeper"), Find::Has("mattermost"), Find::Has("rocket.chat"), Find::Word("wire"),
    Find::Has("nextcloud"), Find::Has("caprine"), Find::Word("dino"), Find::Has("nheko"), Find::Has("neochat"),
    Find::Has("zoiper"),
    // Windows' own: calls from a phone through Phone Link.
    Find::Has("yourphone"), Find::Has("phone link"), Find::Has("phoneexperiencehost"),
];

/// Browsers, whose tabs hold web calls (Meet, Zoom, Teams on the web…).
const BROWSERS: &[Find] = &[
    Find::Has("chrom"), Find::Has("firefox"), Find::Has("brave"), Find::Has("msedge"), Find::Has("vivaldi"),
    Find::Has("opera"), Find::Has("librewolf"), Find::Word("zen"), Find::Has("comet"), Find::Has("waterfox"),
];

/// Apps that record rather than call, matched in their names.
const RECORDERS: &[&str] = &["obs", "audacity", "tenacity", "recorder", "kdenlive", "ardour"];

/// How a name is looked for in lower-case text: anywhere in it, or as a word of its own.
#[derive(Clone, Copy)]
enum Find {
    Has(&'static str),
    Word(&'static str),
}

impl Find {
    fn in_text(self, text: &str) -> bool {
        match self {
            Self::Has(needle) => text.contains(needle),
            Self::Word(word) => has_word(text, word),
        }
    }
}

fn is_word_char(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// Whether `word` stands in `text` as a word of its own.
fn has_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(at, _)| {
        !is_word_char(text[..at].chars().next_back()) && !is_word_char(text[at + word.len()..].chars().next())
    })
}

fn any(finds: &[Find], text: &str) -> bool {
    let text = text.to_lowercase();
    finds.iter().any(|find| find.in_text(&text))
}

/// The call service a window title names, or None.
pub fn service_of(title: &str) -> Option<&'static str> {
    let title = title.to_lowercase();
    SERVICES.iter().find(|(finds, _)| finds.iter().any(|find| find.in_text(&title))).map(|(_, name)| *name)
}

/// Whether a title shows a call under way: a Meet code ("kgm-mwyq-mge") or the words.
fn in_call(title: &str) -> bool {
    let bytes = title.as_bytes();
    let code = (0..bytes.len().saturating_sub(11)).any(|at| {
        let part = &bytes[at..at + 12];
        let shape = part.iter().enumerate().all(|(i, &b)| if i == 3 || i == 8 { b == b'-' } else { b.is_ascii_lowercase() });
        let before = at.checked_sub(1).map(|i| bytes[i]);
        let after = bytes.get(at + 12).copied();
        let edge = |b: Option<u8>| !b.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_');
        shape && edge(before) && edge(after)
    });
    let lower = title.to_lowercase();
    code || IN_CALL_WORDS.iter().any(|word| has_word(&lower, word))
}

/// Whether an app going by any of `names` is made for calls.
pub fn is_call_app(names: &[&str]) -> bool {
    names.iter().any(|name| any(CALL_APPS, name))
}

/// Whether a program is a browser (WebView2, which any app may run, is not one).
pub fn is_browser(path: &str) -> bool {
    let lower = path.to_lowercase();
    !lower.contains("msedgewebview2") && any(BROWSERS, &file_stem(&lower))
}

fn is_recorder(names: &[&str]) -> bool {
    names.iter().any(|name| {
        // By the runs of letters in it: "obs64", "loom-recorder-production", "WindowsSoundRecorder".
        let name = name.to_lowercase();
        name.contains("recorder")
            || name.split(|c: char| !c.is_alphabetic()).any(|run| RECORDERS.contains(&run))
    })
}

/// Whether a browser's `windows` ((title, in front as the microphone opened)) show a web call: one on a call
/// service's page that shows a call under way, or was in front as the microphone opened. WhatsApp Web open
/// behind is none.
pub fn web_call_shown(windows: &[(&str, bool)]) -> bool {
    windows.iter().any(|(title, focused)| service_of(title).is_some() && (*focused || in_call(title)))
}

/// Of `windows` ((title, in front), most recently used first), the one a call that just began is in, or None
/// when none names a service: a title showing a call under way comes first, then the window in front, then the
/// most recently used.
pub fn call_window_index(windows: &[(&str, bool)]) -> Option<usize> {
    let mut best = None;
    let mut best_score = 0;
    for (index, (title, focused)) in windows.iter().enumerate() {
        if service_of(title).is_none() {
            continue;
        }
        let score = 1 + if in_call(title) { 4 } else { 0 } + if *focused { 2 } else { 0 };
        if score > best_score {
            best = Some(index);
            best_score = score;
        }
    }
    best
}

fn file_stem(path: &str) -> String {
    std::path::Path::new(path).file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Who holds the microphone, as Windows' privacy settings know it: a Store app by its package family, any other
/// program by its path (lower case).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Holder {
    Package(String),
    Program(String),
}

/// A top-level window someone could be calling in.
#[derive(Clone, Debug)]
pub struct Win {
    pub hwnd: isize,
    pub title: String,
    /// Its process's program, lower case.
    pub path: String,
    pub family: Option<String>,
    pub focused: bool,
}

impl Holder {
    /// Whether `window` is one of this holder's.
    fn owns(&self, window: &Win) -> bool {
        match self {
            Self::Package(family) => window.family.as_deref() == Some(family.as_str()),
            Self::Program(path) => &window.path == path,
        }
    }

    fn path(&self) -> Option<&str> {
        match self {
            Self::Program(path) => Some(path),
            Self::Package(_) => None,
        }
    }
}

/// What the island shows of the call.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CallState {
    pub active: bool,
    /// When the microphone was taken, in ms since the epoch.
    pub since_ms: i64,
    /// The call's service ("Google Meet") once its window named one, else its app's name.
    pub name: String,
    /// The app's icon as a `data:` URL, or "".
    pub icon: String,
    /// The camera is on too.
    pub video: bool,
    /// The microphone is muted, for every app.
    pub muted: bool,
    /// The call's window: there is one to go back to, and its shape (width / height) for the preview.
    pub reachable: bool,
    pub aspect: f64,
}

/// The call under way, and what is known of whoever holds the microphone.
struct Active {
    holder: Holder,
    since_ms: i64,
    began: Instant,
    service: Option<&'static str>,
    hwnd: isize,
    ending: Option<Instant>,
}

/// When each holder was first seen, and the window in front then.
struct Seen {
    at: Instant,
    focused: isize,
    since_ms: i64,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |time| time.as_millis() as i64)
}

pub struct Calls {
    state: Mutex<CallState>,
    hwnd: Mutex<isize>,
    /// The live preview: its DWM thumbnail and the window it shows.
    thumbnail: Mutex<Option<(isize, isize)>>,
}

impl Default for Calls {
    fn default() -> Self {
        Self { state: Mutex::default(), hwnd: Mutex::new(0), thumbnail: Mutex::new(None) }
    }
}

impl Calls {
    pub fn state(&self) -> Value {
        serde_json::to_value(&*self.state.lock().unwrap()).unwrap_or(Value::Null)
    }

    /// Mutes or unmutes the microphone from the call's button; the new state.
    pub fn toggle_muted(&self) -> Value {
        let muted = !self.state.lock().unwrap().muted;
        imp::set_muted(muted);
        self.state.lock().unwrap().muted = imp::muted();
        self.state()
    }

    /// Brings the call's window to the front.
    pub fn raise(&self) {
        let hwnd = *self.hwnd.lock().unwrap();
        if hwnd != 0 {
            imp::raise(hwnd);
        }
    }

    /// Shows the call's window live in the island's window `dest` at `rect` (physical pixels: x, y, width,
    /// height), or stops showing it.
    pub fn preview(&self, dest: isize, rect: Option<[i32; 4]>) {
        let source = *self.hwnd.lock().unwrap();
        let mut thumbnail = self.thumbnail.lock().unwrap();
        let wanted = rect.filter(|_| source != 0);
        if let Some((thumb, shown)) = *thumbnail {
            if wanted.is_none() || shown != source {
                imp::unregister(thumb);
                *thumbnail = None;
            }
        }
        let Some(rect) = wanted else { return };
        if thumbnail.is_none() {
            *thumbnail = imp::register(dest, source).map(|thumb| (thumb, source));
        }
        if let Some((thumb, _)) = *thumbnail {
            imp::place(thumb, source, rect);
        }
    }
}

/// Follows the microphone's users, while `enabled` (Settings: call-activity), calling `changed` with each new state.
pub fn watch(calls: &Calls, enabled: impl Fn() -> bool, changed: impl Fn(Value)) {
    let own = std::env::current_exe().map(|path| path.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut seen: HashMap<Holder, Seen> = HashMap::new();
    let mut active: Option<Active> = None;
    let mut names: HashMap<String, String> = HashMap::new();
    let mut icons: HashMap<String, String> = HashMap::new();
    loop {
        let on = enabled();
        let now = Instant::now();
        // Isle's own, a recorder's and WebView2's (any app's, its own name says nothing) never make a call.
        let holders: Vec<(Holder, i64)> = if on {
            imp::microphone_users()
                .into_iter()
                .filter(|(holder, _)| match holder {
                    Holder::Program(path) => {
                        *path != own && !path.contains("msedgewebview2") && !is_recorder(&[&file_stem(path)])
                    }
                    Holder::Package(family) => !is_recorder(&[family]),
                })
                .collect()
        } else {
            Vec::new()
        };
        seen.retain(|holder, _| holders.iter().any(|(held, _)| held == holder));
        let front = imp::foreground();
        for (holder, _) in &holders {
            seen.entry(holder.clone()).or_insert(Seen { at: now, focused: front, since_ms: now_ms() });
        }
        let windows = if holders.is_empty() && active.is_none() { Vec::new() } else { imp::windows() };
        let mut name_of = |path: &str| names.entry(path.to_owned()).or_insert_with(|| imp::name_of(path)).clone();
        let is_call = |holder: &Holder, name_of: &mut dyn FnMut(&str) -> String| -> bool {
            let names: Vec<String> = match holder {
                Holder::Package(family) => vec![family.clone()],
                Holder::Program(path) => vec![file_stem(path), name_of(path)],
            };
            if is_call_app(&names.iter().map(String::as_str).collect::<Vec<_>>()) {
                return true;
            }
            // A browser's: only its own windows count (a Meet tab in Edge is no call in Chrome); in front as the
            // microphone opened, not since: switching to WhatsApp Web mid-dictation is no call.
            let Some(path) = holder.path().filter(|path| is_browser(path)) else { return false };
            let focused = seen.get(holder).map_or(0, |seen| seen.focused);
            let titles: Vec<(&str, bool)> = windows
                .iter()
                .filter(|window| window.path == path)
                .map(|window| (window.title.as_str(), window.hwnd == focused))
                .collect();
            web_call_shown(&titles)
        };

        if let Some(call) = &mut active {
            // Its own microphone while there; else another call's (an app reopening it on a new device).
            let still = holders.iter().any(|(holder, _)| *holder == call.holder);
            let other = if still { None } else { holders.iter().find(|(holder, _)| is_call(holder, &mut name_of)) };
            if still || other.is_some() {
                if let Some((holder, _)) = other {
                    call.holder = holder.clone();
                }
                call.ending = None;
            } else if !on {
                active = None;
            } else if call.ending.get_or_insert(now).elapsed() >= END_DELAY {
                active = None;
            }
        } else if on {
            let begun = holders.iter().find(|(holder, _)| {
                seen.get(holder).is_some_and(|seen| seen.at.elapsed() >= START_DELAY) && is_call(holder, &mut name_of)
            });
            if let Some((holder, _)) = begun {
                let since_ms = seen.get(holder).map_or_else(now_ms, |seen| seen.since_ms);
                active = Some(Active {
                    holder: holder.clone(),
                    since_ms,
                    began: now,
                    service: None,
                    hwnd: 0,
                    ending: None,
                });
            }
        }

        let state = match &mut active {
            Some(call) => {
                choose_window(call, &windows);
                let window = windows.iter().find(|window| window.hwnd == call.hwnd);
                let path = window.map(|window| window.path.clone()).or_else(|| call.holder.path().map(str::to_owned));
                let app_name = path.as_deref().map(&mut name_of);
                let name = call
                    .service
                    .map(str::to_owned)
                    .or(app_name)
                    .unwrap_or_else(|| "Call".to_owned());
                let icon = path
                    .map(|path| icons.entry(path.clone()).or_insert_with(|| crate::app_usage::imp::icon(&path)).clone())
                    .unwrap_or_default();
                let video = !imp::in_use("webcam").into_iter().all(|(holder, _)| holder.path() == Some(own.as_str()));
                CallState {
                    active: true,
                    since_ms: call.since_ms,
                    name,
                    icon,
                    video,
                    muted: imp::muted(),
                    reachable: window.is_some(),
                    aspect: window.map_or(0.0, |window| imp::aspect(window.hwnd)),
                }
            }
            None => CallState::default(),
        };
        *calls.hwnd.lock().unwrap() = active.as_ref().map_or(0, |call| call.hwnd);
        let changed_now = {
            let mut current = calls.state.lock().unwrap();
            let differs = *current != state;
            *current = state;
            differs
        };
        if changed_now {
            changed(calls.state());
        }
        std::thread::sleep(EVERY);
    }
}

/// The call keeps the window it was found in: one titled after a call service, else the app's window in front
/// (or most recently used) when it began. A window that stops showing the call gives way to one that does.
fn choose_window(call: &mut Active, windows: &[Win]) {
    let mine: Vec<&Win> = windows.iter().filter(|window| call.holder.owns(window)).collect();
    // Early on, the window's title may still come to name the service (the call's tab coming to the front).
    if call.service.is_none() && call.began.elapsed() < SERVICE_GRACE {
        if let Some(window) = mine.iter().find(|window| window.hwnd == call.hwnd) {
            call.service = service_of(&window.title);
        }
    }
    let current = mine.iter().find(|window| window.hwnd == call.hwnd);
    if let (Some(current), Some(service)) = (current, call.service) {
        if service_of(&current.title) == Some(service) {
            return;
        }
        // Its tab moved: a window that now names the call's service.
        if let Some(moved) = mine.iter().find(|window| service_of(&window.title) == Some(service)) {
            call.hwnd = moved.hwnd;
        }
        return;
    }
    if current.is_some() {
        return;
    }
    if let Some(service) = call.service {
        if let Some(window) = mine.iter().find(|window| service_of(&window.title) == Some(service)) {
            call.hwnd = window.hwnd;
            return;
        }
    }
    let titles: Vec<(&str, bool)> = mine.iter().map(|window| (window.title.as_str(), window.focused)).collect();
    if let Some(index) = call_window_index(&titles) {
        call.service = service_of(&mine[index].title);
        call.hwnd = mine[index].hwnd;
        return;
    }
    call.hwnd = mine.iter().find(|window| window.focused).or(mine.first()).map_or(0, |window| window.hwnd);
}

#[cfg(windows)]
pub(crate) mod imp {
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Dwm::{
        DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_RECTSOURCE,
        DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE, DWMWA_CLOAKED, DwmGetWindowAttribute, DwmRegisterThumbnail,
        DwmUnregisterThumbnail, DwmUpdateThumbnailProperties,
    };
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{
        ERole, IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eCommunications, eConsole,
    };
    use windows::Win32::Storage::Packaging::Appx::GetPackageFamilyName;
    use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, RRF_RT_REG_QWORD, RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
    };
    use windows::Win32::System::Threading::{
        AttachThreadInput, GetCurrentThreadId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, EnumWindows, GW_OWNER, GWL_EXSTYLE, GetClientRect, GetForegroundWindow, GetWindow,
        GetWindowLongW, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow,
        IsWindowVisible, SW_RESTORE, SetForegroundWindow, ShowWindow, WS_EX_TOOLWINDOW,
    };
    use windows::core::{BOOL, HSTRING, PWSTR};

    use super::{Holder, Win};
    use crate::app_usage::imp::{class_of, path_of, pid_of};

    pub use crate::app_usage::imp::name_of;

    /// From Windows' file time (100 ns since 1601) to ms since 1970.
    fn unix_ms(file_time: u64) -> i64 {
        (file_time / 10_000) as i64 - 11_644_473_600_000
    }

    fn qword(key: HKEY, subkey: &HSTRING, value: &str) -> u64 {
        let mut data = 0u64;
        let mut size = size_of::<u64>() as u32;
        // SAFETY: reads a QWORD into a u64 of the size given.
        let read = unsafe {
            RegGetValueW(key, subkey, &HSTRING::from(value), RRF_RT_REG_QWORD, None, Some((&raw mut data).cast()), Some(&mut size))
        };
        if read.is_ok() { data } else { 0 }
    }

    /// The names of the keys under `key`.
    fn subkeys(key: HKEY) -> Vec<String> {
        let mut names = Vec::new();
        for index in 0.. {
            let mut buffer = [0u16; 512];
            let mut len = buffer.len() as u32;
            // SAFETY: fills a buffer of the size given.
            let read = unsafe { RegEnumKeyExW(key, index, Some(PWSTR(buffer.as_mut_ptr())), &mut len, None, None, None, None) };
            if read.is_err() {
                break;
            }
            names.push(String::from_utf16_lossy(&buffer[..len as usize]));
        }
        names
    }

    /// The apps using `capability` ("microphone", "webcam") now, each with when it began (ms since the epoch).
    pub fn in_use(capability: &str) -> Vec<(Holder, i64)> {
        let base = format!(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\{capability}");
        let mut found = Vec::new();
        let mut look = |path: &str, holder: &dyn Fn(&str) -> Holder| {
            let mut key = HKEY::default();
            // SAFETY: opens a key for reading into `key`, closed below.
            if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(path), None, KEY_READ, &mut key) }.is_err() {
                return;
            }
            for name in subkeys(key) {
                if name == "NonPackaged" {
                    continue;
                }
                let subkey = HSTRING::from(name.as_str());
                let (start, stop) = (qword(key, &subkey, "LastUsedTimeStart"), qword(key, &subkey, "LastUsedTimeStop"));
                // In use: begun, and not stopped since.
                if start != 0 && stop < start {
                    found.push((holder(&name), unix_ms(start)));
                }
            }
            // SAFETY: closes the key opened above.
            let _ = unsafe { RegCloseKey(key) };
        };
        look(&base, &|name| Holder::Package(name.to_owned()));
        look(&format!(r"{base}\NonPackaged"), &|name| Holder::Program(name.replace('#', "\\").to_lowercase()));
        found
    }

    /// Each process's parent and program name, from one snapshot.
    fn processes() -> std::collections::HashMap<u32, (u32, String)> {
        use windows::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
        };
        let mut found = std::collections::HashMap::new();
        // SAFETY: walks a process snapshot with an entry of the size it is told, then closes it.
        unsafe {
            let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return found };
            let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
            while more {
                let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase();
                found.insert(entry.th32ProcessID, (entry.th32ParentProcessID, name));
                more = Process32NextW(snapshot, &mut entry).is_ok();
            }
            let _ = CloseHandle(snapshot);
        }
        found
    }

    /// The apps recording from a microphone now, by the sound system's own sessions (what Windows' microphone
    /// indicator shows): a Store app by its package, any other by its program. WebView2's helper records for the
    /// app it runs in, found up its parents.
    pub fn microphone_users() -> Vec<(Holder, i64)> {
        use windows::Win32::Media::Audio::{
            AudioSessionStateActive, DEVICE_STATE_ACTIVE, IAudioSessionControl2, IAudioSessionManager2,
        };
        use windows::core::Interface;
        let mut pids = Vec::new();
        // SAFETY: COM calls as documented, on interfaces the calls hand back.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let Ok(devices) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else {
                return Vec::new();
            };
            let Ok(endpoints) = devices.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) else { return Vec::new() };
            for index in 0..endpoints.GetCount().unwrap_or(0) {
                let Ok(device) = endpoints.Item(index) else { continue };
                let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
                let Ok(sessions) = manager.GetSessionEnumerator() else { continue };
                for at in 0..sessions.GetCount().unwrap_or(0) {
                    let Ok(session) = sessions.GetSession(at) else { continue };
                    if session.GetState().ok() != Some(AudioSessionStateActive) {
                        continue;
                    }
                    if let Ok(pid) = session.cast::<IAudioSessionControl2>().and_then(|session| session.GetProcessId()) {
                        if pid != 0 && !pids.contains(&pid) {
                            pids.push(pid);
                        }
                    }
                }
            }
        }
        if pids.is_empty() {
            return Vec::new();
        }
        let tree = processes();
        let mut found: Vec<(Holder, i64)> = Vec::new();
        for mut pid in pids {
            for _ in 0..6 {
                match tree.get(&pid) {
                    Some((parent, name)) if name == "msedgewebview2.exe" && *parent != 0 => pid = *parent,
                    _ => break,
                }
            }
            let Some(path) = path_of(pid).map(|path| path.to_lowercase()) else { continue };
            let holder = match family_of(pid) {
                Some(family) if path.contains(r"\windowsapps\") => Holder::Package(family),
                _ => Holder::Program(path),
            };
            if !found.iter().any(|(held, _)| *held == holder) {
                found.push((holder, 0));
            }
        }
        found
    }

    pub fn foreground() -> isize {
        // SAFETY: a plain query.
        unsafe { GetForegroundWindow() }.0 as isize
    }

    /// A process's Store package family, or None for a plain program.
    fn family_of(pid: u32) -> Option<String> {
        // SAFETY: opens the process for the least access there is, reads its package family, closes it.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buffer = [0u16; 256];
            let mut len = buffer.len() as u32;
            let read = GetPackageFamilyName(process, &mut len, Some(PWSTR(buffer.as_mut_ptr())));
            let _ = CloseHandle(process);
            read.is_ok().then(|| String::from_utf16_lossy(&buffer[..len.saturating_sub(1) as usize]))
        }
    }

    /// The windows on the taskbar, front first: shown, unowned, not a tool window, not cloaked.
    pub fn windows() -> Vec<Win> {
        unsafe extern "system" fn collect(hwnd: HWND, found: LPARAM) -> BOOL {
            // SAFETY: `found` is the Vec below, alive for the whole enumeration.
            unsafe { (*(found.0 as *mut Vec<HWND>)).push(hwnd) };
            true.into()
        }
        let mut handles: Vec<HWND> = Vec::new();
        // SAFETY: the callback only pushes into `handles`, which outlives the call.
        unsafe {
            let _ = EnumWindows(Some(collect), LPARAM(&raw mut handles as isize));
        }
        let front = foreground();
        let mut pids = std::collections::HashMap::new();
        handles
            .into_iter()
            .filter_map(|hwnd| {
                // SAFETY: plain queries of windows the system handed us.
                unsafe {
                    if !IsWindowVisible(hwnd).as_bool() || GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.is_invalid()) {
                        return None;
                    }
                    if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
                        return None;
                    }
                    let mut cloaked = 0u32;
                    let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&raw mut cloaked).cast(), size_of::<u32>() as u32);
                    if cloaked != 0 {
                        return None;
                    }
                    let len = GetWindowTextLengthW(hwnd);
                    if len == 0 {
                        return None;
                    }
                    let mut text = vec![0u16; len as usize + 1];
                    let got = GetWindowTextW(hwnd, &mut text) as usize;
                    let title = String::from_utf16_lossy(&text[..got]);
                    let pid = app_pid(hwnd);
                    let (path, family) = pids
                        .entry(pid)
                        .or_insert_with(|| (path_of(pid).unwrap_or_default().to_lowercase(), family_of(pid)))
                        .clone();
                    Some(Win { hwnd: hwnd.0 as isize, title, path, family, focused: hwnd.0 as isize == front })
                }
            })
            .collect()
    }

    /// A window's app's process: a Store app's window belongs to its frame host, the app is the window inside.
    fn app_pid(hwnd: HWND) -> u32 {
        use windows::Win32::UI::WindowsAndMessaging::EnumChildWindows;
        if class_of(hwnd) != "ApplicationFrameWindow" {
            return pid_of(hwnd);
        }
        unsafe extern "system" fn inner(child: HWND, found: LPARAM) -> BOOL {
            // SAFETY: `found` is the u32 below, alive for the whole enumeration.
            unsafe {
                if class_of(child) == "Windows.UI.Core.CoreWindow" {
                    *(found.0 as *mut u32) = pid_of(child);
                    return false.into();
                }
            }
            true.into()
        }
        let mut found = 0u32;
        // SAFETY: the callback only reads child windows and writes `found`, which outlives the call.
        unsafe {
            let _ = EnumChildWindows(Some(hwnd), Some(inner), LPARAM(&raw mut found as isize));
        }
        if found != 0 { found } else { pid_of(hwnd) }
    }

    /// The window's width over its height.
    pub fn aspect(hwnd: isize) -> f64 {
        let (width, height) = size(hwnd);
        if height > 0 { f64::from(width) / f64::from(height) } else { 0.0 }
    }

    /// A window's inside, in pixels; a minimized one's as it was before (its copy shows it so).
    fn size(hwnd: isize) -> (i32, i32) {
        use windows::Win32::UI::WindowsAndMessaging::{GetWindowPlacement, WINDOWPLACEMENT};
        let hwnd = HWND(hwnd as _);
        let mut rect = RECT::default();
        // SAFETY: plain queries of a window's size and placement.
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let mut placement = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
                if GetWindowPlacement(hwnd, &mut placement).is_ok() {
                    let normal = placement.rcNormalPosition;
                    return (normal.right - normal.left, normal.bottom - normal.top);
                }
            }
            let _ = GetClientRect(hwnd, &mut rect);
        }
        (rect.right - rect.left, rect.bottom - rect.top)
    }

    /// Brings a window to the front, restored if minimized.
    pub fn raise(hwnd: isize) {
        let hwnd = HWND(hwnd as _);
        // SAFETY: plain Win32 calls on a window handle the system gave us; the input queues attached are detached
        // again before returning.
        unsafe {
            if !IsWindow(Some(hwnd)).as_bool() {
                return;
            }
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            // Windows lets the app in front hand the front over: Isle borrows its input queue for the moment.
            let front = GetForegroundWindow();
            let theirs = GetWindowThreadProcessId(front, None);
            let ours = GetCurrentThreadId();
            let attached = theirs != 0 && theirs != ours && AttachThreadInput(ours, theirs, true).as_bool();
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);
            if attached {
                let _ = AttachThreadInput(ours, theirs, false);
            }
        }
    }

    /// The default microphone's mute control, for calls (`eCommunications`) or everything else (`eConsole`).
    fn microphone(role: ERole) -> Option<IAudioEndpointVolume> {
        // SAFETY: COM calls as documented; CoInitializeEx only fails when the thread already chose a model,
        // which serves as well.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let device = devices.GetDefaultAudioEndpoint(eCapture, role).ok()?;
            device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok()
        }
    }

    /// Whether the microphone calls use is muted.
    pub fn muted() -> bool {
        // SAFETY: a plain COM query.
        microphone(eCommunications).is_some_and(|volume| unsafe { volume.GetMute() }.is_ok_and(|muted| muted.as_bool()))
    }

    /// Mutes or unmutes the microphone, both the one for calls and the default one (often the same).
    pub fn set_muted(muted: bool) {
        for role in [eCommunications, eConsole] {
            if let Some(volume) = microphone(role) {
                // SAFETY: a plain COM call; no event context is passed.
                let _ = unsafe { volume.SetMute(muted, std::ptr::null()) };
            }
        }
    }

    /// Starts a live copy of window `source` in window `dest`.
    pub fn register(dest: isize, source: isize) -> Option<isize> {
        // SAFETY: both are windows the system handed us; DWM checks them.
        unsafe { DwmRegisterThumbnail(HWND(dest as _), HWND(source as _)) }.ok()
    }

    pub fn unregister(thumb: isize) {
        // SAFETY: a thumbnail registered above.
        let _ = unsafe { DwmUnregisterThumbnail(thumb) };
    }

    /// Shows the live copy at `rect` (x, y, width, height in `dest`'s pixels), cropped to fill it as the
    /// preview on GNOME is: the middle of the window, as wide or as tall as fits.
    pub fn place(thumb: isize, source: isize, [x, y, width, height]: [i32; 4]) {
        let (sw, sh) = size(source);
        let (sw, sh) = (f64::from(sw.max(1)), f64::from(sh.max(1)));
        let scale = (f64::from(width) / sw).max(f64::from(height) / sh);
        let (cw, ch) = (f64::from(width) / scale, f64::from(height) / scale);
        let (cx, cy) = ((sw - cw) / 2.0, (sh - ch) / 2.0);
        let properties = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_RECTSOURCE | DWM_TNP_OPACITY | DWM_TNP_VISIBLE
                | DWM_TNP_SOURCECLIENTAREAONLY,
            rcDestination: RECT { left: x, top: y, right: x + width, bottom: y + height },
            rcSource: RECT {
                left: cx.round() as i32,
                top: cy.round() as i32,
                right: (cx + cw).round() as i32,
                bottom: (cy + ch).round() as i32,
            },
            opacity: 255,
            fVisible: true.into(),
            fSourceClientAreaOnly: true.into(),
        };
        // SAFETY: a thumbnail registered above, with properties that live through the call.
        let _ = unsafe { DwmUpdateThumbnailProperties(thumb, &properties) };
    }
}

#[cfg(not(windows))]
pub(crate) mod imp {
    use super::{Holder, Win};

    pub fn in_use(_capability: &str) -> Vec<(Holder, i64)> {
        Vec::new()
    }
    pub fn microphone_users() -> Vec<(Holder, i64)> {
        Vec::new()
    }
    pub fn foreground() -> isize {
        0
    }
    pub fn windows() -> Vec<Win> {
        Vec::new()
    }
    pub fn name_of(path: &str) -> String {
        super::file_stem(path)
    }
    pub fn aspect(_hwnd: isize) -> f64 {
        0.0
    }
    pub fn raise(_hwnd: isize) {}
    pub fn muted() -> bool {
        false
    }
    pub fn set_muted(_muted: bool) {}
    pub fn register(_dest: isize, _source: isize) -> Option<isize> {
        None
    }
    pub fn unregister(_thumb: isize) {}
    pub fn place(_thumb: isize, _source: isize, _rect: [i32; 4]) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn services_are_named_in_titles() {
        assert_eq!(service_of("Meet – kgm-mwyq-mge - Google Chrome"), Some("Google Meet"));
        assert_eq!(service_of("Chat | Microsoft Teams"), Some("Microsoft Teams"));
        assert_eq!(service_of("Meeting notes - Notepad"), None);
        assert_eq!(service_of("signal.rs - isle - Visual Studio Code"), Some("Signal"));
    }

    #[test]
    fn a_call_under_way_shows_in_its_title() {
        assert!(in_call("Meet – kgm-mwyq-mge"));
        assert!(in_call("Huddle with Sam"));
        assert!(!in_call("Google Meet"));
        assert!(!in_call("abc-defg-hijk"));
    }

    #[test]
    fn call_apps_and_browsers() {
        assert!(is_call_app(&["5319275A.WhatsAppDesktop_cv1g1gvanyjgm"]));
        assert!(is_call_app(&["MSTeams_8wekyb3d8bbwe"]));
        assert!(is_call_app(&["discord", "Discord"]));
        assert!(!is_call_app(&["code", "Visual Studio Code"]));
        assert!(is_browser(r"c:\program files\google\chrome\application\chrome.exe"));
        assert!(is_browser(r"c:\program files (x86)\microsoft\edge\application\msedge.exe"));
        assert!(!is_browser(r"c:\program files (x86)\microsoft\edgewebview\application\1\msedgewebview2.exe"));
        assert!(is_recorder(&["obs64"]));
        assert!(is_recorder(&["Microsoft.WindowsSoundRecorder_8wekyb3d8bbwe"]));
        assert!(!is_recorder(&["chrome"]));
    }

    #[test]
    fn whatsapp_web_behind_is_no_call() {
        assert!(!web_call_shown(&[("WhatsApp - Google Chrome", false)]));
        assert!(web_call_shown(&[("WhatsApp - Google Chrome", true)]));
        assert!(web_call_shown(&[("Meet – kgm-mwyq-mge - Google Chrome", false)]));
    }

    #[test]
    fn a_meet_call_wins_over_whatsapp_in_front() {
        let windows = [("WhatsApp - Google Chrome", true), ("Meet – kgm-mwyq-mge - Google Chrome", false)];
        assert_eq!(call_window_index(&windows), Some(1));
        assert_eq!(call_window_index(&[("Inbox - Google Chrome", true)]), None);
    }
}
