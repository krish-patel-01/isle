// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! Where the island is, and what is in front of it: the window sits at the top centre of the main display,
//! or of the display in use (Settings: island-monitor); a full-screen app (a video, a game, a slideshow) on
//! that display hides it, as GNOME hides the island under a full-screen window.

use std::sync::{Condvar, Mutex};
use std::time::Duration;

use tauri::{Emitter, LogicalSize, PhysicalPosition};

use crate::{WINDOW_HEIGHT, WINDOW_WIDTH};

/// How often the window in front is looked at.
const EVERY: Duration = Duration::from_millis(400);

static WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

/// Looks again now (a setting changed).
pub fn wake() {
    *WAKE.0.lock().unwrap() = true;
    WAKE.1.notify_one();
}

fn sleep() {
    let woken = WAKE.0.lock().unwrap();
    let (mut woken, _) = WAKE.1.wait_timeout_while(woken, EVERY, |w| !*w).unwrap();
    *woken = false;
}

/// A display, in physical pixels.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Display {
    pub x: i32,
    pub y: i32,
    pub width: i32,
}

/// Puts the window at the top of `display` (the main one when `None`), centred.
pub fn place(window: &tauri::WebviewWindow, display: Option<Display>) {
    let monitors = window.available_monitors().unwrap_or_default();
    let monitor = match display {
        Some(display) => monitors
            .into_iter()
            .find(|m| m.position().x == display.x && m.position().y == display.y),
        None => window.primary_monitor().ok().flatten(),
    };
    let Some(monitor) = monitor else {
        return;
    };
    let scale = monitor.scale_factor();
    let width = (WINDOW_WIDTH * scale).round() as i32;
    let x = monitor.position().x + (monitor.size().width as i32 - width) / 2;
    let _ = window.set_position(PhysicalPosition::new(x, monitor.position().y));
    // Moved to a display of another scale, it keeps its size in CSS pixels.
    let _ = window.set_size(LogicalSize::new(WINDOW_WIDTH, WINDOW_HEIGHT));
}

/// Watches the window in front for as long as Isle runs: sends `fullscreen` as a full-screen app comes and
/// goes on the island's display, `windowAtTop` as an app's window reaches up under the island (for auto-hide
/// beside windows), and follows the display in use when `follow()` says so.
pub fn watch(window: tauri::WebviewWindow, follow: impl Fn() -> bool) {
    let mut fullscreen = false;
    let mut at_top = false;
    loop {
        sleep();
        let now = imp::window_at_top(&window);
        if now != at_top {
            at_top = now;
            let _ = window.emit("windowAtTop", at_top);
        }
        // Isle itself in front (Settings, the island typed in) leaves everything as it was.
        let Some(front) = imp::front() else {
            continue;
        };
        let target = if follow() { Some(front.display) } else { imp::main_display() };
        if target.is_some() && target != imp::island_display(&window) {
            place(&window, target);
        }
        let now = front.fullscreen && Some(front.display) == imp::island_display(&window);
        if now != fullscreen {
            fullscreen = now;
            let _ = window.emit("fullscreen", fullscreen);
        }
    }
}

/// Whether a full-screen app is in front now, for the island's first look.
pub fn fullscreen_now() -> bool {
    imp::front().is_some_and(|f| f.fullscreen)
}

/// Opens `url` in the browser.
pub fn open(url: &str) {
    imp::open(url);
}

/// The window in front: its display, and whether it covers it whole.
#[derive(Clone, Copy, Debug)]
struct Front {
    display: Display,
    fullscreen: bool,
}

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
        MonitorFromWindow,
    };
    use windows::Win32::UI::Shell::{
        QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState, ShellExecuteW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GWL_EXSTYLE, GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowRect,
        GetWindowThreadProcessId, IsIconic, IsWindowVisible, IsZoomed, SW_SHOWNORMAL, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
    };
    use windows::core::{BOOL, HSTRING, PCWSTR, w};

    use super::{Display, Front};

    fn display_of(monitor: HMONITOR) -> Option<(Display, RECT)> {
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        // SAFETY: fills a MONITORINFO of the size it is told.
        unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool().then(|| {
            let r = info.rcMonitor;
            (Display { x: r.left, y: r.top, width: r.right - r.left }, r)
        })
    }

    pub fn main_display() -> Option<Display> {
        // SAFETY: plain lookups of the display at the origin, which is the main one.
        display_of(unsafe { MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY) }).map(|(d, _)| d)
    }

    pub fn island_display(window: &tauri::WebviewWindow) -> Option<Display> {
        let hwnd = HWND(window.hwnd().ok()?.0 as _);
        // SAFETY: the display nearest a window of ours.
        display_of(unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) }).map(|(d, _)| d)
    }

    /// How close to the display's top a window counts as reaching it (shell.js REACH_PX), in CSS px.
    const REACH_PX: i32 = 5;

    fn class_of(hwnd: HWND) -> String {
        let mut class = [0u16; 64];
        // SAFETY: fills a buffer of the size given.
        let len = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
        String::from_utf16_lossy(&class[..len])
    }

    fn is_shell(class: &str) -> bool {
        matches!(class, "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")
    }

    /// Whether an app's window on the island's display reaches up under it: maximized, snapped or moved there
    /// (shell.js `windowAtTop`).
    pub fn window_at_top(window: &tauri::WebviewWindow) -> bool {
        let Ok(island) = window.hwnd() else {
            return false;
        };
        let island = HWND(island.0 as _);
        let mut bounds = RECT::default();
        // SAFETY: plain Win32 queries of our own window.
        let (display, scale) = unsafe {
            if GetWindowRect(island, &mut bounds).is_err() {
                return false;
            }
            let Some((_, display)) = display_of(MonitorFromWindow(island, MONITOR_DEFAULTTONEAREST)) else {
                return false;
            };
            (display, GetDpiForWindow(island).max(96) as i32)
        };
        struct Search {
            island: RECT,
            display: RECT,
            reach: i32,
            pid: u32,
            found: bool,
        }
        unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> BOOL {
            // SAFETY: `data` is the Search below, alive for the whole enumeration.
            let search = unsafe { &mut *(data.0 as *mut Search) };
            // SAFETY: plain Win32 queries of a top-level window the system handed us.
            unsafe {
                if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                    return true.into();
                }
                if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
                    return true.into();
                }
                let mut pid = 0;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                if pid == search.pid || is_shell(&class_of(hwnd)) {
                    return true.into();
                }
                // Hidden by Windows itself (a suspended app, another desktop).
                let mut cloaked = 0u32;
                let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&raw mut cloaked).cast(), 4);
                if cloaked != 0 {
                    return true.into();
                }
                let mut rect = RECT::default();
                if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, (&raw mut rect).cast(), size_of::<RECT>() as u32)
                    .is_err()
                {
                    return true.into();
                }
                let on_display = rect.left < search.display.right && rect.right > search.display.left
                    && rect.top < search.display.bottom && rect.bottom > search.display.top;
                if on_display && rect.top < search.display.top + search.reach
                    && rect.left < search.island.right && rect.right > search.island.left
                {
                    search.found = true;
                    return false.into();
                }
            }
            true.into()
        }
        let mut search = Search {
            island: bounds,
            display,
            reach: REACH_PX * scale / 96,
            pid: std::process::id(),
            found: false,
        };
        // SAFETY: the callback only reads windows and writes `search`, which outlives the call.
        let _ = unsafe { EnumWindows(Some(each), LPARAM(&raw mut search as isize)) };
        search.found
    }

    pub fn front() -> Option<Front> {
        // SAFETY: plain Win32 queries of the window in front.
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.is_invalid() {
                return None;
            }
            // Isle itself (the island taking the keyboard, Settings) says nothing about the app behind it.
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == std::process::id() {
                return None;
            }
            let (display, bounds) = display_of(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST))?;
            // The desktop and the taskbar cover the display too.
            let shell = is_shell(&class_of(hwnd));
            // What it draws, without the invisible resize borders. A maximized window is never full screen, even
            // covering the display with the taskbar set to hide: as on GNOME, only a video, a game or F11 is.
            let mut rect = RECT::default();
            let drawn = DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, (&raw mut rect).cast(), size_of::<RECT>() as u32)
                .is_ok()
                || GetWindowRect(hwnd, &mut rect).is_ok();
            let covers = drawn
                && rect.left <= bounds.left
                && rect.top <= bounds.top
                && rect.right >= bounds.right
                && rect.bottom >= bounds.bottom;
            let maximized = IsZoomed(hwnd).as_bool();
            // An overlay that clicks go through (a recording's border, a helper's highlight) is no app.
            let overlay = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & (WS_EX_TRANSPARENT.0 | WS_EX_TOOLWINDOW.0) != 0;
            let mut fullscreen = !shell && !overlay && covers && !maximized && !IsIconic(hwnd).as_bool();
            // A game in exclusive full screen, or a presentation, as Windows itself tells.
            if let Ok(state) = SHQueryUserNotificationState() {
                fullscreen |= !shell && (state == QUNS_RUNNING_D3D_FULL_SCREEN || state == QUNS_PRESENTATION_MODE);
            }
            log::debug!("in front: {} covers {covers}, maximized {maximized}, overlay {overlay}, full screen {fullscreen}", class_of(hwnd));
            Some(Front { display, fullscreen })
        }
    }

    pub fn open(url: &str) {
        // SAFETY: hands a URL to the shell, which opens the default browser.
        unsafe {
            ShellExecuteW(None, w!("open"), &HSTRING::from(url), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{Display, Front};

    pub fn main_display() -> Option<Display> {
        None
    }

    pub fn island_display(_window: &tauri::WebviewWindow) -> Option<Display> {
        None
    }

    pub fn window_at_top(_window: &tauri::WebviewWindow) -> bool {
        false
    }

    pub fn front() -> Option<Front> {
        None
    }

    pub fn open(_url: &str) {}
}
