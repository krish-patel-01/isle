// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! App usage, as the extension times it on GNOME (appUsage.js) and isled keeps it (AppUsage1): the app whose
//! window is in front is timed, while the Apps page is turned on in Settings; five minutes without input pause
//! it, unless something keeps the screen awake (a video). The days are isle-core's, kept 30 days in
//! `app-usage.json`. An app is its program: its path is its id, its description its name.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{Days, Local};
use isle_core::app_usage::{AppTotal, AppUsage, KEEP_DAYS};
use serde_json::{Value, json};

use crate::store;

const FILE: &str = "app-usage.json";
/// How often the window in front is looked at.
const EVERY: Duration = Duration::from_secs(1);
/// Five minutes without input pause the clock, unless the screen is kept awake.
const IDLE: Duration = Duration::from_secs(5 * 60);
/// What is timed is saved this often.
const SAVE_EVERY: Duration = Duration::from_secs(60);

/// The days kept, and each app's icon once looked up.
pub struct Usage {
    usage: Mutex<AppUsage>,
    icons: Mutex<HashMap<String, String>>,
}

/// The last `count` days, oldest first, today last.
fn days_back(count: u32) -> Vec<String> {
    let today = Local::now().date_naive();
    (0..count)
        .rev()
        .filter_map(|back| today.checked_sub_days(Days::new(u64::from(back))))
        .map(|day| day.format("%Y-%m-%d").to_string())
        .collect()
}

impl Usage {
    pub fn load() -> Self {
        let mut usage: AppUsage = store::load(FILE);
        if days_back(KEEP_DAYS).first().is_some_and(|oldest| usage.prune(oldest)) {
            store::save(FILE, &usage);
        }
        Self { usage: Mutex::new(usage), icons: Mutex::default() }
    }

    /// Today's apps, the last seven days', and the last 30, most used first, each with its icon.
    pub fn stats(&self) -> Value {
        let days = days_back(KEEP_DAYS);
        let usage = self.usage.lock().unwrap();
        let today = days.last().map_or("", String::as_str);
        let week = &days[days.len().saturating_sub(7)..];
        let list = |totals: Vec<AppTotal>| -> Value {
            let mut icons = self.icons.lock().unwrap();
            Value::Array(
                totals
                    .into_iter()
                    .map(|app| {
                        let icon = icons.entry(app.id.clone()).or_insert_with(|| imp::icon(&app.id)).clone();
                        json!({"id": app.id, "name": app.name, "ms": app.ms, "icon": icon})
                    })
                    .collect(),
            )
        };
        json!({
            "today": list(usage.totals([today])),
            "week": list(usage.totals(week.iter().map(String::as_str))),
            "month": list(usage.totals(days.iter().map(String::as_str))),
        })
    }

    /// Forgets every day, as Settings' Clear asks.
    pub fn clear(&self) {
        let mut usage = self.usage.lock().unwrap();
        usage.clear();
        store::save(FILE, &*usage);
    }

    fn record(&self, day: &str, id: &str, name: &str, ms: i64) {
        self.usage.lock().unwrap().record(day, id, name, ms);
    }

    fn save(&self) {
        let mut usage = self.usage.lock().unwrap();
        if let Some(oldest) = days_back(KEEP_DAYS).first() {
            usage.prune(oldest);
        }
        store::save(FILE, &*usage);
    }
}

/// Times the app in front for as long as Isle runs, while `enabled()` says so.
pub fn watch(usage: &Usage, enabled: impl Fn() -> bool) {
    let mut last = Instant::now();
    let mut saved = Instant::now();
    let mut unsaved = false;
    loop {
        std::thread::sleep(EVERY);
        let now = Instant::now();
        // The time since the last look, at most a little more than a look's (a computer that slept counts none).
        let ms = now.duration_since(last).min(EVERY * 3).as_millis() as i64;
        last = now;
        if !enabled() {
            continue;
        }
        let away = imp::idle() >= IDLE && !imp::kept_awake();
        if !away && let Some((id, name)) = imp::front_app() {
            usage.record(&Local::now().format("%Y-%m-%d").to_string(), &id, &name, ms);
            unsaved = true;
        }
        if unsaved && now.duration_since(saved) >= SAVE_EVERY {
            usage.save();
            saved = now;
            unsaved = false;
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::time::Duration;

    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, MAX_PATH};
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, ReleaseDC,
    };
    use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
    use windows::Win32::System::Power::{CallNtPowerInformation, SystemExecutionState};
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    use windows::Win32::UI::Shell::{SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGetFileInfoW};
    use windows::Win32::UI::WindowsAndMessaging::{
        DestroyIcon, EnumChildWindows, GetClassNameW, GetForegroundWindow, GetIconInfo, GetWindowThreadProcessId,
        ICONINFO,
    };
    use windows::core::{BOOL, HSTRING, PCWSTR, PWSTR, w};

    /// How long since the last key or pointer movement.
    pub fn idle() -> Duration {
        let mut info = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        // SAFETY: fills a struct of the size it is told.
        if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
            return Duration::ZERO;
        }
        // SAFETY: a plain clock read.
        let now = unsafe { GetTickCount() };
        Duration::from_millis(u64::from(now.wrapping_sub(info.dwTime)))
    }

    /// Whether an app keeps the screen awake (a video player), as the session's idle inhibitor on GNOME.
    pub fn kept_awake() -> bool {
        const ES_DISPLAY_REQUIRED: u32 = 0x2;
        let mut state = 0u32;
        // SAFETY: asks for the system's execution state into a u32, as documented.
        let asked = unsafe {
            CallNtPowerInformation(SystemExecutionState, None, 0, Some((&raw mut state).cast()), size_of::<u32>() as u32)
        };
        asked.is_ok() && state & ES_DISPLAY_REQUIRED != 0
    }

    fn class_of(hwnd: HWND) -> String {
        let mut class = [0u16; 64];
        // SAFETY: fills a buffer of the size given.
        let len = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
        String::from_utf16_lossy(&class[..len])
    }

    fn pid_of(hwnd: HWND) -> u32 {
        let mut pid = 0;
        // SAFETY: a plain query of a window the system handed us.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        pid
    }

    /// The program a process runs, as a full path.
    fn path_of(pid: u32) -> Option<String> {
        // SAFETY: opens a process for the least access there is, reads its image name, closes it.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buffer = [0u16; MAX_PATH as usize * 4];
            let mut len = buffer.len() as u32;
            let read = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut len);
            let _ = CloseHandle(process);
            read.ok()?;
            Some(String::from_utf16_lossy(&buffer[..len as usize]))
        }
    }

    /// A program's name as it calls itself (its file description), else its file name.
    fn name_of(path: &str) -> String {
        let stem = std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let wide = HSTRING::from(path);
        // SAFETY: reads a file's version resource into a buffer of the size it reported, then a string from it.
        let described = unsafe {
            let size = GetFileVersionInfoSizeW(&wide, None);
            if size == 0 {
                return stem;
            }
            let mut data = vec![0u8; size as usize];
            if GetFileVersionInfoW(&wide, None, size, data.as_mut_ptr().cast()).is_err() {
                return stem;
            }
            // The first language the file has.
            let mut table: *mut c_void = std::ptr::null_mut();
            let mut len = 0u32;
            if !VerQueryValueW(data.as_ptr().cast(), w!("\\VarFileInfo\\Translation"), &mut table, &mut len).as_bool() || len < 4 {
                return stem;
            }
            let pair = *(table as *const [u16; 2]);
            let key = HSTRING::from(format!("\\StringFileInfo\\{:04x}{:04x}\\FileDescription", pair[0], pair[1]));
            let mut text: *mut c_void = std::ptr::null_mut();
            if !VerQueryValueW(data.as_ptr().cast(), &key, &mut text, &mut len).as_bool() || len == 0 {
                return stem;
            }
            PCWSTR(text as *const u16).to_string().unwrap_or_default()
        };
        let described = described.trim().to_owned();
        if described.is_empty() { stem } else { described }
    }

    /// The app in front: (its program's path, its name); none for the desktop, the taskbar or Isle itself.
    pub fn front_app() -> Option<(String, String)> {
        // SAFETY: a plain query of the window in front.
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_invalid() {
            return None;
        }
        if matches!(class_of(hwnd).as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
            return None;
        }
        let mut pid = pid_of(hwnd);
        // A Store app's window belongs to its frame host; the app is the process of the window inside.
        if class_of(hwnd) == "ApplicationFrameWindow" {
            unsafe extern "system" fn inner(child: HWND, found: LPARAM) -> BOOL {
                // SAFETY: `found` is the u32 below, alive for the whole enumeration.
                unsafe {
                    if super::imp::class_of(child) == "Windows.UI.Core.CoreWindow" {
                        *(found.0 as *mut u32) = super::imp::pid_of(child);
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
            if found != 0 {
                pid = found;
            }
        }
        if pid == std::process::id() {
            return None;
        }
        let path = path_of(pid)?;
        let name = name_of(&path);
        Some((path.to_lowercase(), name))
    }

    /// A program's icon as a PNG `data:` URL, or "".
    pub fn icon(path: &str) -> String {
        let wide = HSTRING::from(path);
        let mut info = SHFILEINFOW::default();
        // SAFETY: asks the shell for the file's icon, reads its colour and mask bitmaps, frees all of it.
        unsafe {
            if SHGetFileInfoW(&wide, Default::default(), Some(&mut info), size_of::<SHFILEINFOW>() as u32, SHGFI_ICON | SHGFI_LARGEICON) == 0
                || info.hIcon.is_invalid()
            {
                return String::new();
            }
            let mut parts = ICONINFO::default();
            let got = GetIconInfo(info.hIcon, &mut parts).is_ok();
            let mut png = String::new();
            if got {
                let dc = GetDC(None);
                let mut header = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, ..Default::default() },
                    ..Default::default()
                };
                // First its size, then its pixels, top row first, as 32-bit BGRA.
                if GetDIBits(dc, parts.hbmColor, 0, 0, None, &mut header, DIB_RGB_COLORS) != 0 {
                    let (width, height) = (header.bmiHeader.biWidth, header.bmiHeader.biHeight.abs());
                    header.bmiHeader.biBitCount = 32;
                    header.bmiHeader.biCompression = BI_RGB.0;
                    header.bmiHeader.biHeight = -height;
                    let mut pixels = vec![0u8; (width * height * 4) as usize];
                    if GetDIBits(dc, parts.hbmColor, 0, height as u32, Some(pixels.as_mut_ptr().cast()), &mut header, DIB_RGB_COLORS) != 0 {
                        // BGRA to RGBA; an icon without alpha is opaque.
                        let opaque = pixels.chunks(4).all(|p| p[3] == 0);
                        for p in pixels.chunks_mut(4) {
                            p.swap(0, 2);
                            if opaque {
                                p[3] = 255;
                            }
                        }
                        png = crate::data_url(&super::png(width as u32, height as u32, &pixels));
                    }
                }
                ReleaseDC(None, dc);
                let _ = DeleteObject(parts.hbmColor.into());
                let _ = DeleteObject(parts.hbmMask.into());
            }
            let _ = DestroyIcon(info.hIcon);
            png
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::time::Duration;

    pub fn idle() -> Duration {
        Duration::ZERO
    }

    pub fn kept_awake() -> bool {
        false
    }

    pub fn front_app() -> Option<(String, String)> {
        None
    }

    pub fn icon(_path: &str) -> String {
        String::new()
    }
}

/// An RGBA image as a PNG, its pixels stored uncompressed (an icon is small).
fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    // Each row starts with filter 0 (none).
    let mut raw = Vec::with_capacity(rgba.len() + height as usize);
    for row in rgba.chunks((width * 4) as usize) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    // zlib with stored deflate blocks of at most 65535 bytes, and the Adler-32 sum.
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_png_has_its_signature_header_and_end() {
        let image = png(2, 1, &[255, 0, 0, 255, 0, 255, 0, 128]);
        assert_eq!(&image[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&image[12..16], b"IHDR");
        assert_eq!(&image[image.len() - 8..image.len() - 4], b"IEND");
        // The well-known CRC of an empty IEND chunk.
        assert_eq!(&image[image.len() - 4..], &[0xAE, 0x42, 0x60, 0x82]);
    }

    #[test]
    fn days_back_ends_today() {
        let days = days_back(7);
        assert_eq!(days.len(), 7);
        assert_eq!(days.last().unwrap(), &Local::now().format("%Y-%m-%d").to_string());
    }
}
