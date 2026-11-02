// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! Isle for Windows. On GNOME the island is drawn by a Shell extension and fed by `isled` over D-Bus;
//! here one process does both: a transparent, always-on-top window at the top of the screen draws the
//! island (`ui/`, the same sizes, springs and styles as the extension), and this side serves it the
//! state the daemon's interfaces would (Media1, Pomodoro1, Todo1), with the same JSON.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[path = "../../isle-daemon/src/http.rs"]
mod http;
mod media;
mod store;
mod tasks;
mod timer;

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use log::warn;
use serde_json::{Value, json};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

use crate::media::lyrics::{self, Lyrics, LyricsStatus, TrackQuery};
use crate::tasks::Tasks;
use crate::timer::Timer;

/// The window the island lives in: room for the open island (600 × 202 CSS px), its 22 px shoulders,
/// a spring's overshoot, and the alerts and assistant that hang lower.
const WINDOW_WIDTH: f64 = 760.0;
const WINDOW_HEIGHT: f64 = 420.0;

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
    })
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
        other => warn!("no interface {other}"),
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
                    .map(|bytes| format!("data:{};base64,{}", mime(&bytes), base64(&bytes)));
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
    let status = if now.available {
        LyricsStatus::Loading
    } else {
        LyricsStatus::None
    };
    let pending = query.status(status);
    *app.lyrics.lock().unwrap() = pending.clone();
    let _ = handle.emit("lyrics", &pending);
    if !now.available {
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
    loop {
        thread::sleep(Duration::from_millis(500));
        let finished = {
            let mut timer = app.timer.lock().unwrap();
            timer.tick().map(|(finished, next)| (finished, next, timer.state()))
        };
        if let Some((finished, next, state)) = finished {
            let _ = handle.emit("pomodoro", state);
            let _ = handle.emit("phaseFinished", json!([finished, next]));
            chime();
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

/// Puts the window at the top of the primary display, centred.
fn place(window: &tauri::WebviewWindow) {
    let Ok(Some(monitor)) = window.primary_monitor() else {
        return;
    };
    let size = monitor.size();
    let origin = monitor.position();
    let width = (WINDOW_WIDTH * monitor.scale_factor()).round() as i32;
    let x = origin.x + (size.width as i32 - width) / 2;
    let _ = window.set_position(PhysicalPosition::new(x, origin.y));
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!("Isle {} starting", env!("CARGO_PKG_VERSION"));
    let (wish_tx, wish_rx) = mpsc::channel();
    let app = Arc::new(App {
        timer: Timer::load(),
        tasks: Tasks::load(),
        media: Mutex::new(serde_json::to_value(media::gsmtc::NowPlaying::default()).unwrap_or(Value::Null)),
        lyrics: Mutex::new(Lyrics::default()),
        media_wishes: Mutex::new(wish_tx),
    });

    tauri::Builder::default()
        .plugin(tauri_single_instance())
        .manage(app.clone())
        .invoke_handler(tauri::generate_handler![
            snapshot,
            call,
            media_art,
            set_interactive,
            set_keyboard
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
            place(&window);
            let _ = window.set_ignore_cursor_events(true);
            let _ = window.show();

            let quit = MenuItem::with_id(tauri_app, "quit", "Quit Isle", true, None::<&str>)?;
            let menu = Menu::with_items(tauri_app, &[&quit])?;
            TrayIconBuilder::new()
                .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?)
                .tooltip("Isle")
                .menu(&menu)
                .on_menu_event(|handle, event| {
                    if event.id().as_ref() == "quit" {
                        handle.exit(0);
                    }
                })
                .build(tauri_app)?;

            let handle = tauri_app.handle().clone();
            let pointer_window = window.clone();
            thread::spawn(move || watch_pointer(pointer_window));
            let (clock_handle, clock_app) = (handle.clone(), app.clone());
            thread::spawn(move || watch_clock(clock_handle, clock_app));
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

/// A second Isle started by hand exits at once and leaves the running one alone, as `isled` does.
fn tauri_single_instance() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("single-instance")
        .setup(|_app, _api| {
            #[cfg(windows)]
            {
                use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
                use windows::Win32::System::Threading::CreateMutexW;
                use windows::core::w;
                // SAFETY: a named mutex the process keeps until it exits.
                unsafe {
                    let _mutex = CreateMutexW(None, true, w!("Local\\io.github.zickrian.isle"));
                    if GetLastError() == ERROR_ALREADY_EXISTS {
                        std::process::exit(0);
                    }
                    std::mem::forget(_mutex);
                }
            }
            Ok(())
        })
        .build()
}
