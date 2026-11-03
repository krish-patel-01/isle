// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! Isle for Windows. On GNOME the island is drawn by a Shell extension and fed by `isled` over D-Bus;
//! here one process does both: a transparent, always-on-top window at the top of the screen draws the
//! island (`ui/`, the same sizes, springs and styles as the extension), and this side serves it the
//! state the daemon's interfaces would (Media1, Pomodoro1, Todo1), with the same JSON.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agents;
mod calendar;
#[path = "../../isle-daemon/src/http.rs"]
mod http;
mod media;
mod notifications;
mod screen;
mod settings;
mod store;
mod tasks;
mod timer;
mod usage;

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use log::warn;
use serde_json::{Value, json};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::media::lyrics::{self, Lyrics, LyricsStatus, TrackQuery};
use crate::settings::Settings;
use crate::tasks::Tasks;
use crate::timer::Timer;

/// The window the island lives in: room for the open island (600 × 202 CSS px), its 22 px shoulders,
/// a spring's overshoot, and the alerts and assistant that hang lower.
pub(crate) const WINDOW_WIDTH: f64 = 760.0;
pub(crate) const WINDOW_HEIGHT: f64 = 420.0;

/// What the media thread is asked to do.
enum MediaWish {
    Control(String, i64),
    Art(Sender<Option<String>>),
}

struct App {
    timer: Mutex<Timer>,
    tasks: Mutex<Tasks>,
    media: Mutex<Value>,
    lyrics: Mutex<Lyrics>,
    media_wishes: Mutex<Sender<MediaWish>>,
    usage: Mutex<Value>,
    usage_refresh: Arc<usage::Refresh>,
    agents: Arc<agents::Pending>,
    settings: Mutex<Settings>,
    calendar: Mutex<calendar::Own>,
    calendar_wanted: Arc<calendar::Wanted>,
    system_events: Mutex<Value>,
    notifications: Arc<notifications::Center>,
    /// Alerts before events not yet begun, for a page that loads after they were sent.
    events_soon: Mutex<Vec<Value>>,
}

/// Every interface's state at once, as the extension reads them when `isled` appears.
#[tauri::command]
fn snapshot(app: tauri::State<'_, Arc<App>>) -> Value {
    let timer = app.timer.lock().unwrap();
    json!({
        "nowPlaying": app.media.lock().unwrap().clone(),
        "lyrics": serde_json::to_value(&*app.lyrics.lock().unwrap()).unwrap_or(Value::Null),
        "pomodoro": timer.state(),
        "pomodoroConfig": timer.config(),
        "todo": app.tasks.lock().unwrap().items(),
        "usage": app.usage.lock().unwrap().clone(),
        "settings": app.settings.lock().unwrap().all(),
        "events": app.calendar.lock().unwrap().events(),
        "systemEvents": app.system_events.lock().unwrap().clone(),
        "notifications": app.notifications.state(),
        "eventsSoon": app.events_soon.lock().unwrap().clone(),
        "fullscreen": screen::fullscreen_now(),
    })
}

/// Changes one setting (as the Settings window does), and tells every window.
#[tauri::command]
fn settings_set(handle: AppHandle, app: tauri::State<'_, Arc<App>>, key: String, value: Value) -> Result<(), String> {
    let changed = app.settings.lock().unwrap().set(&key, value).map_err(|err| format!("{err:#}"))?;
    if changed {
        settings_changed(&handle, &app, &key);
    }
    Ok(())
}

/// Every setting back to its default; Isle's data stays.
#[tauri::command]
fn settings_reset(handle: AppHandle, app: tauri::State<'_, Arc<App>>) {
    app.settings.lock().unwrap().reset();
    settings_changed(&handle, &app, "");
}

fn settings_changed(handle: &AppHandle, app: &App, key: &str) {
    let _ = handle.emit("settings", app.settings.lock().unwrap().all());
    if key.is_empty() || key.starts_with("usage-") {
        app.usage_refresh.ask();
    }
    if key.is_empty() || key == "island-monitor" {
        screen::wake();
    }
}

/// How the Calendar's field reads a line ({title, start, end}), as it is typed.
#[tauri::command]
fn calendar_parse(text: String) -> Value {
    calendar::parse_line(&text)
}

/// Joins a meeting: opens its Meet, Zoom, Teams or Webex link, and no other.
#[tauri::command]
fn join_meeting(url: String) {
    if calendar::is_meeting(&url) {
        screen::open(&url);
    }
}

/// Opens Isle's pages on the web in the browser; only those.
#[tauri::command]
fn open_link(url: String) {
    if url.starts_with("https://github.com/zickrian/isle") {
        screen::open(&url);
    }
}

/// Opens the Settings window, or brings it forward. Async: WebView2 hangs a window made inside a blocking
/// command.
#[tauri::command]
async fn open_settings(handle: AppHandle) {
    show_settings(&handle);
}

fn show_settings(handle: &AppHandle) {
    if let Some(window) = handle.get_webview_window("settings") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let built = WebviewWindowBuilder::new(handle, "settings", WebviewUrl::App("settings.html".into()))
        .title("Isle Settings")
        .inner_size(560.0, 680.0)
        .min_inner_size(360.0, 400.0)
        .center()
        .build();
    if let Err(err) = built {
        warn!("could not open Settings: {err}");
    }
}

/// Shows or hides the island's window: hidden over a full-screen app, so a game or video runs undisturbed.
#[tauri::command]
fn set_shown(window: tauri::WebviewWindow, shown: bool) {
    let _ = if shown { window.show() } else { window.hide() };
}

/// One daemon method: `interface` as on D-Bus (`Media1`, `Pomodoro1`, `Todo1`), its argument as JSON.
#[tauri::command]
fn call(handle: AppHandle, app: tauri::State<'_, Arc<App>>, interface: String, method: String, arg: Value) {
    match interface.as_str() {
        "Media1" => {
            let position = arg.as_i64().unwrap_or(0);
            let _ = app
                .media_wishes
                .lock()
                .unwrap()
                .send(MediaWish::Control(method, position));
        }
        "Pomodoro1" => {
            let mut timer = app.timer.lock().unwrap();
            if timer.call(&method, &arg) {
                let _ = handle.emit("pomodoro", timer.state());
                let _ = handle.emit("pomodoroConfig", timer.config());
            }
        }
        "Todo1" => {
            let mut tasks = app.tasks.lock().unwrap();
            if tasks.call(&method, &arg) {
                let _ = handle.emit("todo", tasks.items());
            }
        }
        // The Calendar page's span, for Windows' calendars; Isle's own events added and removed.
        "Calendar1" if method == "Range" => {
            let ms = |key: &str| arg[key].as_i64().unwrap_or_default();
            app.calendar_wanted.ask(ms("begin_ms"), ms("end_ms"));
        }
        "Calendar1" => {
            let mut own = app.calendar.lock().unwrap();
            match own.call(&method, &arg) {
                Ok(true) => {
                    let _ = handle.emit("events", own.events());
                }
                Ok(false) => {}
                Err(err) => warn!("could not add the event: {err}"),
            }
        }
        "Notifications1" => {
            if app.notifications.call(&method, &arg) {
                let _ = handle.emit("notifications", app.notifications.state());
            }
        }
        // The Usage page opened: fresher numbers, at most every two minutes.
        "Usage1" if method == "Refresh" => app.usage_refresh.ask(),
        // The user's answer to a permission: "allow", "always" or "deny".
        "Agents1" if method == "Resolve" => {
            let text = |key: &str| arg[key].as_str().unwrap_or_default().to_owned();
            app.agents.resolve(&text("id"), &text("answer"));
        }
        other => warn!("no {method} in {other}"),
    }
}

/// The current track's cover as a `data:` URL, or null.
#[tauri::command]
async fn media_art(app: tauri::State<'_, Arc<App>>) -> Result<Option<String>, ()> {
    log::debug!("cover asked for");
    let (reply, answer) = mpsc::channel();
    let _ = app.media_wishes.lock().unwrap().send(MediaWish::Art(reply));
    Ok(answer.recv_timeout(Duration::from_secs(5)).ok().flatten())
}

/// Lets clicks reach the island (`true`), or pass through the window to what is under it.
#[tauri::command]
fn set_interactive(window: tauri::WebviewWindow, interactive: bool) {
    let _ = window.set_ignore_cursor_events(!interactive);
}

/// Takes the keyboard, for a text field in the island; `false` hands it back to the window behind.
#[tauri::command]
fn set_keyboard(window: tauri::WebviewWindow, wanted: bool) {
    if wanted {
        let _ = window.set_focus();
    } else {
        #[cfg(windows)]
        focus_behind();
    }
}

#[cfg(windows)]
fn focus_behind() {
    use windows::Win32::UI::WindowsAndMessaging::{
        GW_HWNDNEXT, GetForegroundWindow, GetWindow, IsWindowVisible, SetForegroundWindow,
    };
    // SAFETY: plain Win32 calls on window handles the system gave us.
    unsafe {
        let mut next = GetWindow(GetForegroundWindow(), GW_HWNDNEXT);
        while let Ok(hwnd) = next {
            if hwnd.is_invalid() {
                break;
            }
            if IsWindowVisible(hwnd).as_bool() {
                let _ = SetForegroundWindow(hwnd);
                break;
            }
            next = GetWindow(hwnd, GW_HWNDNEXT);
        }
    }
}

/// An image as a `data:` URL, its type read from its first bytes.
pub(crate) fn data_url(bytes: &[u8]) -> String {
    format!("data:{};base64,{}", mime(bytes), base64(bytes))
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn mime(bytes: &[u8]) -> &'static str {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [b'G', b'I', b'F', ..] => "image/gif",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp",
        _ => "image/jpeg",
    }
}

/// Reads the player twice a second, sends `media` when it changes and looks the lyrics up for a new song.
#[cfg(windows)]
fn watch_media(handle: AppHandle, app: Arc<App>, wishes: Receiver<MediaWish>) {
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
    // SAFETY: once, at the start of this thread.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let reader = loop {
        match media::gsmtc::Reader::new() {
            Ok(reader) => break reader,
            Err(err) => {
                warn!("Windows' media controls are not available yet: {err}");
                thread::sleep(Duration::from_secs(10));
            }
        }
    };
    let mut last = media::gsmtc::NowPlaying::default();
    let mut last_song = String::new();
    loop {
        // Wishes first, then a read; a wish wakes the loop at once.
        let timeout = if last.status == "Playing" { 500 } else { 1000 };
        match wishes.recv_timeout(Duration::from_millis(timeout)) {
            Ok(MediaWish::Control(method, position)) => {
                reader.control(&method, position);
                thread::sleep(Duration::from_millis(120));
            }
            Ok(MediaWish::Art(reply)) => {
                let art = reader
                    .art()
                    .map(|bytes| data_url(&bytes));
                let _ = reply.send(art);
                continue;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        let now = reader.now_playing();
        // A new sample alone (Windows moves the position on its own while it plays) is sent only when
        // it disagrees with where the island thinks the song is by more than a second.
        let drift = {
            let expected = last.position_us
                + if last.status == "Playing" {
                    ((now.sampled_at_ms - last.sampled_at_ms) as f64 * 1000.0 * last.rate) as i64
                } else {
                    0
                };
            (now.position_us - expected).abs() > 1_000_000
        };
        if now.same_as(&last) && !drift {
            continue;
        }
        let song = format!("{}\u{1f}{}\u{1f}{}", now.artist, now.title, now.album);
        last = now.clone();
        let value = serde_json::to_value(&now).unwrap_or(Value::Null);
        *app.media.lock().unwrap() = value.clone();
        let _ = handle.emit("nowPlaying", value);
        if song != last_song {
            last_song = song;
            find_lyrics(&handle, &app, &now);
        }
    }
}

/// Looks the song's lyrics up on a thread of its own (LRCLIB can take seconds), sending `lyrics`.
fn find_lyrics(handle: &AppHandle, app: &Arc<App>, now: &media::gsmtc::NowPlaying) {
    let query = TrackQuery {
        title: now.title.clone(),
        artist: now.artist.clone(),
        album: now.album.clone(),
        length_s: now.length_us as f64 / 1e6,
    };
    let online = app.settings.lock().unwrap().bool("lyrics-online");
    let status = if now.available && online {
        LyricsStatus::Loading
    } else {
        LyricsStatus::None
    };
    let pending = query.status(status);
    *app.lyrics.lock().unwrap() = pending.clone();
    let _ = handle.emit("lyrics", &pending);
    if !now.available || !online {
        return;
    }
    let (handle, app) = (handle.clone(), app.clone());
    thread::spawn(move || {
        let mut found = lyrics::fetch(&query);
        // Offline or LRCLIB busy: try again while the song stays on, as isled does.
        let mut wait = 5;
        while found.status.is_failure() && app.lyrics.lock().unwrap().track == query.key() {
            thread::sleep(Duration::from_secs(wait));
            wait = (wait * 2).min(300);
            found = lyrics::fetch(&query);
        }
        let mut current = app.lyrics.lock().unwrap();
        if current.track == found.track {
            *current = found.clone();
            let _ = handle.emit("lyrics", &found);
        }
    });
}

/// The sound of something finished: Windows' own notification sound, as Isle plays the desktop theme's
/// "complete" on GNOME. Muted with the rest of Windows' sounds (Settings → System → Sound).
fn chime() {
    #[cfg(windows)]
    {
        use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC, SND_NODEFAULT};
        use windows::core::w;
        // SAFETY: plays a named system sound asynchronously; the name is a static string.
        unsafe {
            let _ = PlaySoundW(w!("Notification.Default"), None, SND_ALIAS | SND_ASYNC | SND_NODEFAULT);
        }
    }
}

/// Moves the timer on as phases run out, and clears finished tasks at midnight.
fn watch_clock(handle: AppHandle, app: Arc<App>) {
    let mut day = chrono::Local::now().date_naive();
    let mut alarms = calendar::Alarms::default();
    loop {
        thread::sleep(Duration::from_millis(500));
        // Settings' minutes before an event with a time, once: said in the island, with Join for a meeting.
        let lead = {
            let settings = app.settings.lock().unwrap();
            settings.bool("event-alerts").then(|| settings.int("event-alert-minutes").max(0) * 60_000)
        };
        if let Some(lead) = lead {
            let own = app.calendar.lock().unwrap().events();
            let system = app.system_events.lock().unwrap().clone();
            let now = timer::now_ms();
            let mut soon = app.events_soon.lock().unwrap();
            soon.retain(|event| event["start_ms"].as_i64().unwrap_or_default() > now);
            for event in alarms.due(&own, &system, lead, now) {
                log::info!("event soon: {}", event["title"]);
                soon.push(event.clone());
                let _ = handle.emit("eventSoon", event);
            }
        }
        let finished = {
            let mut timer = app.timer.lock().unwrap();
            timer.tick().map(|(finished, next)| (finished, next, timer.state()))
        };
        if let Some((finished, next, state)) = finished {
            let _ = handle.emit("pomodoro", state);
            let _ = handle.emit("phaseFinished", json!([finished, next]));
            if app.settings.lock().unwrap().bool("chimes") {
                chime();
            }
        }
        let today = chrono::Local::now().date_naive();
        if today != day {
            day = today;
            let mut tasks = app.tasks.lock().unwrap();
            if tasks.call("Midnight", &Value::Null) {
                let _ = handle.emit("todo", tasks.items());
            }
        }
    }
}

/// Sends where the pointer is, in the window's CSS pixels, whenever it moves: the window lets clicks
/// through while the pointer is off the island, so the page could not see it otherwise.
fn watch_pointer(window: tauri::WebviewWindow) {
    let mut last = (f64::NAN, f64::NAN);
    loop {
        thread::sleep(Duration::from_millis(16));
        let (Ok(cursor), Ok(origin), Ok(scale)) = (
            window.cursor_position(),
            window.inner_position(),
            window.scale_factor(),
        ) else {
            continue;
        };
        let here = (
            (cursor.x - f64::from(origin.x)) / scale,
            (cursor.y - f64::from(origin.y)) / scale,
        );
        if here != last {
            last = here;
            let _ = window.emit("pointer", json!({"x": here.0, "y": here.1}));
        }
    }
}

fn main() {
    // Run by Claude Code for its hooks: passes them on to the running Isle, and exits.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--agent-hook") {
        agents::run_hook(&args[2..]);
        return;
    }
    // Run by the uninstaller: takes Isle's hooks out of Claude Code and stops starting with Windows.
    if args.get(1).map(String::as_str) == Some("--uninstall") {
        let _ = agents::set_claude_hooked(false);
        let _ = settings::autostart::set(false);
        return;
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!("Isle {} starting", env!("CARGO_PKG_VERSION"));
    let (wish_tx, wish_rx) = mpsc::channel();
    let app = Arc::new(App {
        timer: Timer::load(),
        tasks: Tasks::load(),
        media: Mutex::new(serde_json::to_value(media::gsmtc::NowPlaying::default()).unwrap_or(Value::Null)),
        lyrics: Mutex::new(Lyrics::default()),
        media_wishes: Mutex::new(wish_tx),
        usage: Mutex::new(json!({"providers": []})),
        usage_refresh: Arc::default(),
        agents: Arc::default(),
        settings: Mutex::new(Settings::load()),
        calendar: calendar::Own::load(),
        calendar_wanted: Arc::default(),
        system_events: Mutex::new(Value::Null),
        notifications: Arc::default(),
        events_soon: Mutex::default(),
    });

    tauri::Builder::default()
        // One Isle at a time; starting it again (from the Start menu, say) opens its Settings.
        .plugin(tauri_plugin_single_instance::init(|handle, _args, _cwd| show_settings(handle)))
        .manage(app.clone())
        .invoke_handler(tauri::generate_handler![
            snapshot,
            call,
            media_art,
            set_interactive,
            set_keyboard,
            set_shown,
            settings_set,
            settings_reset,
            open_settings,
            open_link,
            calendar_parse,
            join_meeting
        ])
        .setup(move |tauri_app| {
            let window = WebviewWindowBuilder::new(tauri_app, "island", WebviewUrl::App("index.html".into()))
                .title("Isle")
                .inner_size(WINDOW_WIDTH, WINDOW_HEIGHT)
                .resizable(false)
                .decorations(false)
                .transparent(true)
                .shadow(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .focused(false)
                .visible(false)
                .build()?;
            screen::place(&window, None);
            let _ = window.set_ignore_cursor_events(true);
            let _ = window.show();

            // The tray icon: a click opens Settings, as does its menu.
            let open = MenuItem::with_id(tauri_app, "settings", "Settings", true, None::<&str>)?;
            let quit = MenuItem::with_id(tauri_app, "quit", "Quit Isle", true, None::<&str>)?;
            let menu = Menu::with_items(tauri_app, &[&open, &PredefinedMenuItem::separator(tauri_app)?, &quit])?;
            TrayIconBuilder::new()
                .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?)
                .tooltip("Isle")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|handle, event| match event.id().as_ref() {
                    "quit" => handle.exit(0),
                    "settings" => show_settings(handle),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        show_settings(tray.app_handle());
                    }
                })
                .build(tauri_app)?;
            first_run();

            let handle = tauri_app.handle().clone();
            let pointer_window = window.clone();
            thread::spawn(move || watch_pointer(pointer_window));
            let (screen_window, screen_app) = (window.clone(), app.clone());
            thread::spawn(move || {
                screen::watch(screen_window, move || {
                    screen_app.settings.lock().unwrap().get("island-monitor").as_str() == Some("active")
                })
            });
            let (calendar_handle, calendar_app) = (handle.clone(), app.clone());
            thread::spawn(move || {
                let wanted = calendar_app.calendar_wanted.clone();
                calendar::watch(&wanted, |events| {
                    let value = serde_json::to_value(events).unwrap_or(Value::Null);
                    *calendar_app.system_events.lock().unwrap() = value.clone();
                    let _ = calendar_handle.emit("systemEvents", value);
                });
            });
            let (notes_handle, posted_handle, center) = (handle.clone(), handle.clone(), app.notifications.clone());
            thread::spawn(move || {
                let state = center.clone();
                notifications::watch(
                    &center,
                    || {
                        let _ = notes_handle.emit("notifications", state.state());
                    },
                    |note| {
                        let _ = posted_handle.emit("notificationPosted", note);
                    },
                );
            });
            let (clock_handle, clock_app) = (handle.clone(), app.clone());
            thread::spawn(move || watch_clock(clock_handle, clock_app));
            let (usage_handle, usage_app) = (handle.clone(), app.clone());
            thread::spawn(move || {
                let refresh = usage_app.usage_refresh.clone();
                usage::run(
                    refresh,
                    || {
                        let settings = usage_app.settings.lock().unwrap();
                        usage::Config {
                            enabled: settings.bool("usage-enabled"),
                            claude: settings.bool("usage-claude"),
                            codex: settings.bool("usage-codex"),
                            warn: settings.int("usage-warn") as f64,
                            critical: settings.int("usage-critical") as f64,
                        }
                    },
                    |usage| {
                        let value = serde_json::to_value(usage).unwrap_or(Value::Null);
                        *usage_app.usage.lock().unwrap() = value.clone();
                        let _ = usage_handle.emit("usage", value);
                    },
                    |provider, window, percent, resets_at| {
                        let _ = usage_handle.emit("usageAlert", json!([provider, window, percent, resets_at]));
                    },
                );
            });
            let (event_handle, request_handle, gone_handle) = (handle.clone(), handle.clone(), handle.clone());
            let pending = app.agents.clone();
            thread::spawn(move || {
                agents::serve(
                    pending,
                    move |event| {
                        // Nothing shows while the user is in the AI's own window.
                        if !agents::in_front(&event.pids) {
                            let _ = event_handle.emit("agentEvent", event);
                        }
                    },
                    move |request| {
                        let _ = request_handle.emit("agentRequest", request);
                    },
                    move |id| {
                        let _ = gone_handle.emit("agentGone", id);
                    },
                );
            });
            #[cfg(windows)]
            {
                let media_app = app.clone();
                thread::spawn(move || watch_media(handle, media_app, wish_rx));
            }
            #[cfg(not(windows))]
            drop((handle, wish_rx));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Isle could not start");
}

/// The first time an installed Isle runs, it starts with Windows from then on, as Isle does on GNOME (a
/// switch in Settings turns that off). A build run from the source tree leaves the registry alone.
fn first_run() {
    if cfg!(debug_assertions) {
        return;
    }
    #[derive(Default, serde::Serialize, serde::Deserialize)]
    #[serde(default)]
    struct State {
        autostart_set: bool,
    }
    let mut state: State = store::load("state.json");
    if state.autostart_set {
        return;
    }
    if let Err(err) = settings::autostart::set(true) {
        warn!("could not start Isle with Windows: {err:#}");
        return;
    }
    state.autostart_set = true;
    store::save("state.json", &state);
}
