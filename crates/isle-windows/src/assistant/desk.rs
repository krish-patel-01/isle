// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! The assistant's desktop actions on Windows (voice.js's on GNOME): apps and windows, the sound, Wi-Fi and
//! Bluetooth, the dark style, Settings' pages, files and folders, music, the screenshot tool and locking. Each says
//! what it did, which goes back to the AI, so it never says done when it was not.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

use super::tools::expand;

/// An installed app as the Start menu lists it.
#[derive(Debug, Clone, Deserialize)]
pub struct StartApp {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "AppID")]
    pub id: String,
}

/// The Start menu's apps, read again after this.
const APPS_FRESH: Duration = Duration::from_secs(10 * 60);
static APPS: Mutex<Option<(Instant, Vec<StartApp>)>> = Mutex::new(None);

/// The installed apps, from the Start menu (Get-StartApps).
pub fn start_apps() -> Vec<StartApp> {
    if let Some((at, apps)) = APPS.lock().unwrap().as_ref()
        && at.elapsed() < APPS_FRESH
    {
        return apps.clone();
    }
    let mut command = std::process::Command::new("powershell.exe");
    command.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", "Get-StartApps | ConvertTo-Json -Compress"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let apps: Vec<StartApp> = command
        .output()
        .ok()
        .and_then(|output| serde_json::from_slice(&output.stdout).ok())
        .unwrap_or_default();
    let apps: Vec<StartApp> = apps
        .into_iter()
        .filter(|app| {
            let name = app.name.to_lowercase();
            !["uninstall", "readme", "help", "documentation", "release notes", "website"].iter().any(|word| name.contains(word))
        })
        .collect();
    if !apps.is_empty() {
        *APPS.lock().unwrap() = Some((Instant::now(), apps.clone()));
    }
    apps
}

/// How well `name` fits what was asked: whole name, its start, a word of it, anywhere in it.
fn fit(name: &str, asked: &str) -> u32 {
    let (name, asked) = (name.to_lowercase(), asked.to_lowercase().trim().to_owned());
    if asked.is_empty() {
        return 0;
    }
    if name == asked {
        4
    } else if name.starts_with(&asked) {
        3
    } else if name.split(|c: char| !c.is_alphanumeric()).any(|word| word == asked) {
        2
    } else if name.contains(&asked) || asked.contains(&name) && name.len() > 3 {
        1
    } else {
        0
    }
}

/// The installed app that fits `asked` best.
pub fn find_app(asked: &str) -> Option<StartApp> {
    let apps = start_apps();
    apps.iter().map(|app| (fit(&app.name, asked), app)).filter(|(score, _)| *score > 0).max_by_key(|(score, app)| (*score, std::cmp::Reverse(app.name.len()))).map(|(_, app)| app.clone())
}

/// Installed apps whose names hold any of `terms`.
pub fn find_apps(terms: &[String]) -> Value {
    let apps = start_apps();
    let found: Vec<&str> = apps
        .iter()
        .filter(|app| terms.iter().any(|term| app.name.to_lowercase().contains(&term.to_lowercase())))
        .map(|app| app.name.as_str())
        .take(30)
        .collect();
    json!({"ok": true, "apps": found})
}

/// The names of the installed apps, for the AI to know (shortened).
pub fn app_names() -> String {
    let mut names: Vec<String> = start_apps().into_iter().map(|app| app.name).collect();
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    names.truncate(250);
    names.join(", ")
}

fn ok(text: impl Into<String>) -> Value {
    json!({"ok": true, "result": text.into()})
}

fn failed(text: impl Into<String>) -> Value {
    json!({"ok": false, "error": text.into()})
}

/// Settings' pages by the names the AI uses.
fn settings_page(panel: &str) -> &'static str {
    match panel.to_lowercase().as_str() {
        "wifi" | "wi-fi" => "network-wifi",
        "bluetooth" | "devices" => "bluetooth",
        "display" | "screen" => "display",
        "sound" | "audio" => "sound",
        "power" | "battery" => "powersleep",
        "background" | "wallpaper" => "personalization-background",
        "network" | "internet" => "network",
        "keyboard" | "typing" => "typing",
        "mouse" | "touchpad" => "mousetouchpad",
        "printers" | "printer" => "printers",
        "notifications" => "notifications",
        "privacy" => "privacy",
        "applications" | "apps" => "appsfeatures",
        "update" | "windows update" => "windowsupdate",
        "colors" | "dark mode" | "theme" => "colors",
        "storage" | "disk" => "storagesense",
        "default apps" => "defaultapps",
        "time" | "date" | "language" => "dateandtime",
        _ => "about",
    }
}

/// Opens a link, a file or a folder in its app.
fn shell_open(target: &str) {
    crate::screen::open(target);
}

/// Runs a desktop action now; the result goes back to the AI. `front` is the window in front as the request began.
pub fn act(action: &Value, front: isize, isle: &super::IsleCall) -> Value {
    imp::init();
    let text = |key: &str| action[key].as_str().unwrap_or_default().trim().to_owned();
    match action["type"].as_str().unwrap_or_default() {
        "open_url" => {
            let url = text("url");
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return failed("only http and https links open");
            }
            shell_open(&url);
            ok(format!("opened {url}"))
        }
        "open_app" => {
            let asked = text("app");
            match find_app(&asked) {
                Some(app) => {
                    imp::launch(&app.id);
                    ok(format!("started {}", app.name))
                }
                None => failed(format!("no installed app called {asked}; find_apps can look by what it does")),
            }
        }
        "close_app" => {
            let asked = text("app");
            let closed = imp::close_app(&asked);
            if closed == 0 { failed(format!("{asked} has no open window")) } else { ok(format!("asked {closed} window(s) of {asked} to close")) }
        }
        "window" => imp::window(&text("command"), &text("title"), &text("app"), front),
        "arrange" => {
            let apps: Vec<String> = action["apps"].as_array().map(|apps| apps.iter().filter_map(|app| app.as_str().map(str::to_owned)).collect()).unwrap_or_default();
            imp::arrange(&text("layout"), &apps)
        }
        "media" => {
            let method = match text("command").as_str() {
                "next" => "Next",
                "previous" => "Previous",
                _ => "PlayPause",
            };
            isle("Media1", method, Value::Null);
            ok(format!("pressed {method}"))
        }
        "play_music" => {
            let (uri, query) = (text("uri"), text("query"));
            // An open.spotify.com link is a spotify: URI the app plays.
            let spotify = uri
                .strip_prefix("https://open.spotify.com/")
                .map(|rest| format!("spotify:{}", rest.split('?').next().unwrap_or_default().replace('/', ":")))
                .or_else(|| uri.starts_with("spotify:").then(|| uri.clone()));
            if let Some(spotify) = spotify.filter(|uri| uri.split(':').count() >= 3) {
                shell_open(&spotify);
                return ok(format!("opened {spotify} in Spotify; it plays once Spotify is up"));
            }
            if query.is_empty() {
                return failed("nothing named to play");
            }
            if find_app("Spotify").is_some() {
                shell_open(&format!("spotify:search:{}", query.replace(' ', "%20")));
                ok(format!("searched Spotify for {query}; the user picks it there (give a uri to play it at once)"))
            } else {
                shell_open(&format!("https://music.youtube.com/search?q={}", query.replace(' ', "+")));
                ok(format!("Spotify isn't installed: searched YouTube Music for {query}"))
            }
        }
        "volume" => imp::volume(action["level"].as_f64(), &text("command")),
        "brightness" => imp::brightness(action["level"].as_f64(), &text("command")),
        "wifi" => imp::radio("wifi", action["on"].as_bool().unwrap_or(true)),
        "bluetooth" => imp::radio("bluetooth", action["on"].as_bool().unwrap_or(true)),
        "dnd" => {
            let on = action["on"].as_bool().unwrap_or(true);
            isle("Notifications1", "SetDnd", json!(on));
            ok(format!("Do Not Disturb {}", if on { "on" } else { "off" }))
        }
        "dark_mode" => imp::dark_mode(action["on"].as_bool().unwrap_or(true)),
        "settings" => {
            let page = settings_page(&text("panel"));
            shell_open(&format!("ms-settings:{page}"));
            ok(format!("opened Settings at {page}"))
        }
        "focus" => {
            let start = text("command") != "stop";
            isle("Pomodoro1", if start { "Start" } else { "Stop" }, Value::Null);
            ok(if start { "focus timer started" } else { "focus timer stopped" })
        }
        "add_task" => {
            isle("Todo1", "Add", json!({"title": text("text")}));
            ok(format!("added the reminder “{}”", text("text")))
        }
        "done_task" => {
            isle("Todo1", "SetDone", json!({"id": action["id"].as_u64().unwrap_or_default(), "done": true}));
            ok("ticked off")
        }
        "remove_task" => {
            isle("Todo1", "Remove", json!({"id": action["id"].as_u64().unwrap_or_default()}));
            ok("removed")
        }
        "add_event" => {
            isle("Calendar1", "Add", json!({"date": text("date"), "text": text("text")}));
            ok(format!("added “{}” on {}", text("text"), text("date")))
        }
        "remove_event" => {
            isle("Calendar1", "Remove", json!({"id": action["id"].as_u64().unwrap_or_default()}));
            ok("removed")
        }
        "open_folder" | "open_file" => {
            let path = expand(&text("path"));
            if !path.exists() {
                return failed(format!("{} does not exist", path.display()));
            }
            shell_open(&path.to_string_lossy());
            ok(format!("opened {}", path.display()))
        }
        "reveal_file" => {
            let path = expand(&text("path"));
            if !path.exists() {
                return failed(format!("{} does not exist", path.display()));
            }
            imp::reveal(&path.to_string_lossy());
            ok(format!("showed {} in File Explorer", path.display()))
        }
        "screenshot" => {
            imp::keys(&[0x5B, 0x10, 0x53]);
            ok("opened the Snipping Tool")
        }
        "lock" => {
            imp::lock();
            ok("locked")
        }
        other => failed(format!("{other} is not an action Isle has on Windows")),
    }
}

#[cfg(windows)]
mod imp {
    use serde_json::{Value, json};
    use windows::Devices::Radios::{Radio, RadioKind, RadioState};
    use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender};
    use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
    use windows::Win32::System::Shutdown::LockWorkStation;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GW_OWNER, GWL_EXSTYLE, GetWindow, GetWindowLongW, GetWindowTextLengthW, GetWindowTextW,
        IsWindowVisible, PostMessageW, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SWP_NOZORDER, SWP_SHOWWINDOW,
        SendMessageTimeoutW, SMTO_ABORTIFHUNG, SetWindowPos, ShowWindow, WM_CLOSE, WM_SETTINGCHANGE, WS_EX_TOOLWINDOW,
        HWND_BROADCAST,
    };
    use windows::core::{BOOL, w};

    use super::{failed, fit, ok};

    pub fn init() {
        // SAFETY: joins this thread to the multithreaded apartment (again is fine).
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    }

    pub fn launch(id: &str) {
        let _ = std::process::Command::new("explorer.exe").arg(format!("shell:AppsFolder\\{id}")).spawn();
    }

    pub fn reveal(path: &str) {
        let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{path}")).spawn();
    }

    pub fn lock() {
        // SAFETY: asks Windows to lock the session.
        let _ = unsafe { LockWorkStation() };
    }

    /// Presses `keys` together, then lets them go.
    pub fn keys(keys: &[u16]) {
        let key = |vk: u16, up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, ..Default::default() },
            },
        };
        let inputs: Vec<INPUT> = keys.iter().map(|&vk| key(vk, false)).chain(keys.iter().rev().map(|&vk| key(vk, true))).collect();
        // SAFETY: synthetic key presses, sized as Windows takes them.
        unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    }

    /// An app's window on the taskbar: its handle, title and app name.
    struct Window {
        hwnd: HWND,
        title: String,
        app: String,
        exe: String,
    }

    fn title_of(hwnd: HWND) -> String {
        // SAFETY: reads a window's title into a buffer of the length it reports.
        unsafe {
            let len = GetWindowTextLengthW(hwnd);
            let mut buffer = vec![0u16; len as usize + 1];
            let read = GetWindowTextW(hwnd, &mut buffer);
            String::from_utf16_lossy(&buffer[..read as usize])
        }
    }

    /// The windows on the taskbar, front first.
    fn windows() -> Vec<Window> {
        unsafe extern "system" fn each(hwnd: HWND, list: LPARAM) -> BOOL {
            // SAFETY: `list` is the Vec handed to EnumWindows below, alive for its call.
            let list = unsafe { &mut *(list.0 as *mut Vec<HWND>) };
            list.push(hwnd);
            BOOL(1)
        }
        let mut all: Vec<HWND> = Vec::new();
        // SAFETY: the callback only pushes into the Vec passed by pointer.
        let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut all as *mut _ as isize)) };
        let island = crate::HANDLE.get().and_then(|_| crate::island_hwnd()).unwrap_or(0);
        all.into_iter()
            .filter(|&hwnd| {
                // SAFETY: plain queries of a window the system listed.
                unsafe {
                    IsWindowVisible(hwnd).as_bool()
                        && GetWindow(hwnd, GW_OWNER).is_err()
                        && GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 == 0
                        && hwnd.0 as isize != island
                }
            })
            .filter_map(|hwnd| {
                let title = title_of(hwnd);
                if title.is_empty() || title == "Program Manager" {
                    return None;
                }
                let path = crate::app_usage::imp::path_of(crate::app_usage::imp::pid_of(hwnd)).unwrap_or_default();
                let exe = std::path::Path::new(&path).file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
                let app = crate::app_usage::imp::name_of(&path);
                Some(Window { hwnd, title, app, exe })
            })
            .collect()
    }

    fn of_app<'a>(windows: &'a [Window], asked: &str) -> Vec<&'a Window> {
        let best = windows.iter().map(|window| fit(&window.app, asked).max(fit(&window.exe, asked))).max().unwrap_or(0);
        if best == 0 {
            return Vec::new();
        }
        windows.iter().filter(|window| fit(&window.app, asked).max(fit(&window.exe, asked)) == best).collect()
    }

    /// The open apps and their windows, for the AI to know.
    pub fn open_windows() -> Vec<Value> {
        windows().into_iter().take(25).map(|window| json!({"app": window.app, "title": window.title})).collect()
    }

    pub fn close_app(asked: &str) -> usize {
        let windows = windows();
        let found = of_app(&windows, asked);
        for window in &found {
            // SAFETY: asks a window to close, as its close button does.
            let _ = unsafe { PostMessageW(Some(window.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        }
        found.len()
    }

    /// The work area of the display `hwnd` is on.
    fn work_area(hwnd: HWND) -> RECT {
        // SAFETY: plain queries.
        unsafe {
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(monitor, &mut info);
            info.rcWork
        }
    }

    /// Puts `hwnd` on the share of its display's work area given in fractions (x, y, width, height).
    fn place(hwnd: HWND, (x, y, width, height): (f64, f64, f64, f64)) {
        let area = work_area(hwnd);
        let (w, h) = (f64::from(area.right - area.left), f64::from(area.bottom - area.top));
        // SAFETY: restores and moves a window the system listed.
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetWindowPos(
                hwnd,
                None,
                area.left + (x * w).round() as i32,
                area.top + (y * h).round() as i32,
                (width * w).round() as i32,
                (height * h).round() as i32,
                SWP_NOZORDER | SWP_SHOWWINDOW,
            );
        }
    }

    fn spot(command: &str) -> Option<(f64, f64, f64, f64)> {
        Some(match command {
            "left" => (0.0, 0.0, 0.5, 1.0),
            "right" => (0.5, 0.0, 0.5, 1.0),
            "top" => (0.0, 0.0, 1.0, 0.5),
            "bottom" => (0.0, 0.5, 1.0, 0.5),
            "top_left" => (0.0, 0.0, 0.5, 0.5),
            "top_right" => (0.5, 0.0, 0.5, 0.5),
            "bottom_left" => (0.0, 0.5, 0.5, 0.5),
            "bottom_right" => (0.5, 0.5, 0.5, 0.5),
            "center" => (0.15, 0.1, 0.7, 0.8),
            _ => return None,
        })
    }

    pub fn window(command: &str, title: &str, app: &str, front: isize) -> Value {
        let windows = windows();
        let target: Option<&Window> = if !title.is_empty() {
            windows.iter().find(|window| window.title.to_lowercase().contains(&title.to_lowercase()))
        } else if !app.is_empty() {
            of_app(&windows, app).into_iter().next()
        } else {
            windows.iter().find(|window| window.hwnd.0 as isize == front).or(windows.first())
        };
        let Some(target) = target else {
            return failed(format!("no open window for {}", if title.is_empty() { app } else { title }));
        };
        let hwnd = target.hwnd;
        // SAFETY: plain calls on a window the system listed.
        unsafe {
            match command {
                "focus" => crate::calls::imp::raise(hwnd.0 as isize),
                "close" => {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
                "minimize" => {
                    let _ = ShowWindow(hwnd, SW_MINIMIZE);
                }
                "maximize" | "fullscreen" => {
                    let _ = ShowWindow(hwnd, SW_MAXIMIZE);
                }
                "unmaximize" | "unfullscreen" => {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                other => match spot(other) {
                    Some(share) => {
                        place(hwnd, share);
                        crate::calls::imp::raise(hwnd.0 as isize);
                    }
                    None => return failed(format!("no window command {other}")),
                },
            }
        }
        ok(format!("{command}: {} ({})", target.title, target.app))
    }

    pub fn arrange(layout: &str, apps: &[String]) -> Value {
        let windows = windows();
        let chosen: Vec<&Window> = if apps.is_empty() {
            windows.iter().take(match layout { "quarters" => 4, "thirds" => 3, "halves" | "rows" => 2, _ => 4 }).collect()
        } else if apps.len() == 1 {
            of_app(&windows, &apps[0])
        } else {
            apps.iter().filter_map(|app| of_app(&windows, app).into_iter().next()).collect()
        };
        if chosen.is_empty() {
            return failed("no windows to arrange");
        }
        let n = chosen.len();
        let (columns, rows) = match layout {
            "halves" => (2, 1),
            "rows" => (1, n.max(1)),
            "thirds" => (3, 1),
            "quarters" => (2, 2),
            _ => {
                let columns = (n as f64).sqrt().ceil() as usize;
                (columns, n.div_ceil(columns))
            }
        };
        for (index, window) in chosen.iter().enumerate().take(columns * rows) {
            let (column, row) = (index % columns, index / columns);
            let (w, h) = (1.0 / columns as f64, 1.0 / rows as f64);
            place(window.hwnd, (column as f64 * w, row as f64 * h, w, h));
        }
        ok(format!("arranged {} window(s): {}", n.min(columns * rows), chosen.iter().map(|window| window.app.as_str()).collect::<Vec<_>>().join(", ")))
    }

    fn speakers() -> Option<IAudioEndpointVolume> {
        // SAFETY: COM calls on this thread's apartment.
        unsafe {
            let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let device = devices.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
            device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok()
        }
    }

    pub fn volume(level: Option<f64>, command: &str) -> Value {
        let Some(speakers) = speakers() else {
            return failed("no speakers");
        };
        // SAFETY: COM calls on the endpoint just made.
        unsafe {
            let now = speakers.GetMasterVolumeLevelScalar().unwrap_or(0.5);
            let set = |value: f32| speakers.SetMasterVolumeLevelScalar(value.clamp(0.0, 1.0), std::ptr::null());
            let done = match (level, command) {
                (Some(level), _) => set((level / 100.0) as f32),
                (None, "up") => set(now + 0.1),
                (None, "down") => set(now - 0.1),
                (None, "mute") => speakers.SetMute(true, std::ptr::null()),
                (None, "unmute") => speakers.SetMute(false, std::ptr::null()),
                _ => return failed("give a level or up, down, mute or unmute"),
            };
            if done.is_err() {
                return failed("Windows didn't change the volume");
            }
            let level = speakers.GetMasterVolumeLevelScalar().unwrap_or(now);
            let muted = speakers.GetMute().map(|muted| muted.as_bool()).unwrap_or(false);
            ok(format!("volume {}%{}", (level * 100.0).round(), if muted { ", muted" } else { "" }))
        }
    }

    pub fn brightness(level: Option<f64>, command: &str) -> Value {
        let script = match (level, command) {
            (Some(level), _) => format!("$v = {}", level.clamp(0.0, 100.0).round()),
            (None, "up") => "$v = [math]::Min(100, (Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightness).CurrentBrightness + 10)".into(),
            (None, "down") => "$v = [math]::Max(0, (Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightness).CurrentBrightness - 10)".into(),
            _ => return failed("give a level or up or down"),
        };
        let script = format!("{script}; Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightnessMethods | Invoke-CimMethod -MethodName WmiSetBrightness -Arguments @{{Timeout=0; Brightness=[byte]$v}} | Out-Null; $v");
        let mut command = std::process::Command::new("powershell.exe");
        command.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", &script]);
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        match command.output() {
            Ok(output) if output.status.success() && output.stderr.is_empty() => {
                ok(format!("brightness {}%", String::from_utf8_lossy(&output.stdout).trim()))
            }
            _ => failed("this display's brightness can't be set from Windows (an external monitor sets its own)"),
        }
    }

    pub fn radio(kind: &str, on: bool) -> Value {
        let wanted = if kind == "wifi" { RadioKind::WiFi } else { RadioKind::Bluetooth };
        let result = (|| -> windows::core::Result<bool> {
            let _ = Radio::RequestAccessAsync()?.get()?;
            let mut done = false;
            for radio in Radio::GetRadiosAsync()?.get()? {
                if radio.Kind()? == wanted {
                    radio.SetStateAsync(if on { RadioState::On } else { RadioState::Off })?.get()?;
                    done = true;
                }
            }
            Ok(done)
        })();
        let name = if kind == "wifi" { "Wi-Fi" } else { "Bluetooth" };
        match result {
            Ok(true) => ok(format!("{name} {}", if on { "on" } else { "off" })),
            Ok(false) => failed(format!("this computer has no {name}")),
            Err(err) => failed(format!("Windows didn't turn {name} {}: {}", if on { "on" } else { "off" }, err.message())),
        }
    }

    pub fn dark_mode(on: bool) -> Value {
        let value = if on { "0" } else { "1" };
        let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
        let set = |name: &str| {
            std::process::Command::new("reg.exe")
                .args(["add", key, "/v", name, "/t", "REG_DWORD", "/d", value, "/f"])
                .output()
                .is_ok_and(|output| output.status.success())
        };
        if !(set("AppsUseLightTheme") & set("SystemUsesLightTheme")) {
            return failed("Windows didn't take the change");
        }
        // SAFETY: tells every window the colours changed, waiting at most a moment for each.
        unsafe {
            let _ = SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                WPARAM(0),
                LPARAM(w!("ImmersiveColorSet").as_ptr() as isize),
                SMTO_ABORTIFHUNG,
                200,
                None,
            );
        }
        ok(if on { "dark mode on" } else { "light mode on" })
    }
}

#[cfg(not(windows))]
mod imp {
    use serde_json::Value;
    pub fn init() {}
    pub fn launch(_id: &str) {}
    pub fn reveal(_path: &str) {}
    pub fn lock() {}
    pub fn keys(_keys: &[u16]) {}
    pub fn open_windows() -> Vec<Value> {
        Vec::new()
    }
    pub fn close_app(_asked: &str) -> usize {
        0
    }
    pub fn window(_command: &str, _title: &str, _app: &str, _front: isize) -> Value {
        super::failed("only on Windows")
    }
    pub fn arrange(_layout: &str, _apps: &[String]) -> Value {
        super::failed("only on Windows")
    }
    pub fn volume(_level: Option<f64>, _command: &str) -> Value {
        super::failed("only on Windows")
    }
    pub fn brightness(_level: Option<f64>, _command: &str) -> Value {
        super::failed("only on Windows")
    }
    pub fn radio(_kind: &str, _on: bool) -> Value {
        super::failed("only on Windows")
    }
    pub fn dark_mode(_on: bool) -> Value {
        super::failed("only on Windows")
    }
}

pub use imp::open_windows;
