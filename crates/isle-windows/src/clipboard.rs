// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! Clipboard history: what GNOME Shell watches (gnome-extension/src/clipboard.js) and isled keeps (its
//! clipboard.rs, Clipboard1), in one place. A hidden window hears each copy; the history is isle-core's, saved
//! in `clipboard.json`, with images in `clips` (`<hash>.png` as copied and `<hash>.card.png`, the card's
//! thumbnail), served to the island as `isleclip://`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use image::{DynamicImage, ImageFormat};
use isle_core::clipboard::{Added, Clip, ClipboardHistory, PinError, content_hash};
use log::{debug, warn};
use serde::Serialize;
use serde_json::Value;

use crate::store;
use crate::timer::now_ms;

const FILE: &str = "clipboard.json";
/// The island's scheme for the cards' thumbnails (WebView2 reaches it as `http://isleclip.localhost/`).
pub const SCHEME: &str = "isleclip";
/// Larger copies are not kept, as on GNOME.
const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
/// Thumbnails are the card's picture at twice its pixels (clipRules.js `CARD_PICTURE`).
const THUMB_WIDTH: u32 = 272;
const THUMB_HEIGHT: u32 = 216;
/// A password manager clears what it copied this soon after; the item then leaves the history too.
const SECRET_CLEARED_MS: i64 = 90 * 1000;
/// Password managers whose copies are never kept, by program name, besides those added in Settings
/// (clipboard-ignored-apps). Most also mark their copies as private, which leaves them out anyway.
pub const IGNORED_APPS: &[&str] = &[
    "keepass", "keepassxc", "bitwarden", "1password", "enpass", "dashlane", "nordpass", "roboform",
    "lastpass", "proton pass", "protonpass", "keeper",
];

/// What a copy held, as read from the clipboard.
pub enum Copied {
    Text(String),
    /// Paths of the files copied.
    Files(Vec<String>),
    /// A PNG image.
    Image(Vec<u8>),
    /// Marked private by the app that copied it (a password manager).
    Secret,
    /// Emptied.
    Empty,
    /// Something the history does not keep.
    Other,
}

/// What to put back on the clipboard for an item.
pub enum Content {
    Text(String),
    Files(Vec<String>),
    /// The PNG file.
    Image(PathBuf),
}

pub struct Clipboard {
    inner: Mutex<Inner>,
}

struct Inner {
    history: ClipboardHistory,
    clips: ClipFiles,
    /// The text last added and when, until something else is copied: a password manager clearing it soon after
    /// takes it out again.
    last_text: Option<(u64, i64)>,
    /// Program names by path, for the cards and the search.
    names: HashMap<String, String>,
}

impl Clipboard {
    /// Loads the history and tidies it against the image files on disk.
    pub fn load() -> Self {
        let mut history: ClipboardHistory = store::load(FILE);
        let clips = ClipFiles { dir: store::data_dir().join("clips") };
        // An image item is useless without its image, which may have been deleted meanwhile.
        let before = history.items().len();
        history.retain(|item| item.clip.image_hash().is_none_or(|hash| clips.image(hash).is_file()));
        history.prune();
        if history.items().len() != before {
            store::save(FILE, &history);
        }
        clips.remove_orphans(&history.image_hashes().collect());
        for hash in history.image_hashes() {
            if !clips.thumb(hash).is_file()
                && let Err(err) = clips.remake_thumb(hash)
            {
                warn!("could not make a clipboard thumbnail again: {err}");
            }
        }
        Self {
            inner: Mutex::new(Inner { history, clips, last_text: None, names: HashMap::new() }),
        }
    }

    /// The `Items` property of Clipboard1, with each card's app name and a thumbnail the island can load.
    pub fn items(&self) -> Value {
        let mut inner = self.inner.lock().unwrap();
        let Inner { history, names, .. } = &mut *inner;
        let view: Vec<ItemView> = history
            .items()
            .iter()
            .map(|item| {
                let image = match &item.clip {
                    Clip::Image { hash, width, height } => Some((hash, *width, *height)),
                    _ => None,
                };
                let thumb_size = image.map(|(_, width, height)| ThumbPlan::new(width, height).size());
                let app_name = if item.app_id.is_empty() {
                    String::new()
                } else {
                    names.entry(item.app_id.clone()).or_insert_with(|| imp::name_of(&item.app_id)).clone()
                };
                ItemView {
                    id: item.id,
                    kind: item.clip.kind(),
                    preview: item.clip.preview(),
                    uris: item.clip.uris(),
                    paths: item.clip.uris().iter().map(|uri| path_of_uri(uri)).collect(),
                    thumb: image.map(|(hash, ..)| format!("http://{SCHEME}.localhost/{hash}.card.png")),
                    thumb_width: thumb_size.map(|(width, _)| width),
                    thumb_height: thumb_size.map(|(_, height)| height),
                    width: image.map(|(_, width, _)| width),
                    height: image.map(|(.., height)| height),
                    app_id: &item.app_id,
                    app_name,
                    at_ms: item.at_ms,
                    pinned: item.pinned,
                }
            })
            .collect();
        serde_json::to_value(view).unwrap_or(Value::Null)
    }

    /// Keeps a copy from the app at `app_id` (its program's path, or ""); returns whether the history changed.
    pub fn add(&self, copied: Copied, app_id: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let clip = match copied {
            Copied::Text(text) => match Clip::from_text(&text) {
                Ok(clip) => clip,
                Err(_) => return false,
            },
            Copied::Files(paths) => match Clip::from_uris(paths.iter().map(|path| uri_of_path(path)).collect()) {
                Ok(clip) => clip,
                Err(_) => return false,
            },
            Copied::Image(png) => {
                if png.len() > MAX_IMAGE_BYTES {
                    return false;
                }
                let hash = content_hash(&png);
                let known = inner.history.image_size(&hash).filter(|_| inner.clips.has(&hash));
                let (width, height) = match known {
                    Some(size) => size,
                    None => match inner.clips.store(&hash, &png) {
                        Ok(size) => size,
                        Err(err) => {
                            warn!("could not keep a copied image: {err}");
                            return false;
                        }
                    },
                };
                Clip::Image { hash, width, height }
            }
            Copied::Secret => {
                inner.last_text = None;
                return false;
            }
            Copied::Empty => return inner.forget_secret(),
            Copied::Other => return false,
        };
        let text = clip.text().is_some();
        let added = inner.history.add(clip, app_id, now_ms());
        inner.last_text = text.then(|| (added.id, now_ms()));
        inner.added(added);
        true
    }

    /// Runs one Clipboard1 method that changes the list; returns whether it changed.
    pub fn call(&self, method: &str, arg: &Value) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let id = arg.get("id").and_then(Value::as_u64).unwrap_or(0);
        let unreferenced = match method {
            "Touch" => {
                if !inner.history.touch(id, now_ms()) {
                    return false;
                }
                Vec::new()
            }
            "SetPinned" => {
                let pinned = arg.get("pinned").and_then(Value::as_bool).unwrap_or(false);
                match inner.history.set_pinned(id, pinned) {
                    Ok(()) => Vec::new(),
                    Err(PinError::NotFound | PinError::Full) => return false,
                }
            }
            "Remove" => match inner.history.remove(id) {
                Some(unreferenced) => unreferenced,
                None => return false,
            },
            "Clear" => inner.history.clear(),
            _ => return false,
        };
        store::save(FILE, &inner.history);
        inner.clips.remove(&unreferenced);
        true
    }

    /// The items whose whole text (or files' names) holds `query`, ignoring case, in the list's order.
    pub fn search(&self, query: &str) -> Vec<u64> {
        let query = query.to_lowercase();
        let inner = self.inner.lock().unwrap();
        inner.history.items().iter().filter(|item| item.clip.contains(&query)).map(|item| item.id).collect()
    }

    /// What to put back on the clipboard for item `id`.
    pub fn content(&self, id: u64) -> Option<Content> {
        let inner = self.inner.lock().unwrap();
        let item = inner.history.get(id)?;
        Some(match &item.clip {
            Clip::Text { text } | Clip::Link { text } => Content::Text(text.clone()),
            Clip::Files { uris } => Content::Files(uris.iter().map(|uri| path_of_uri(uri)).collect()),
            Clip::Image { hash, .. } => Content::Image(inner.clips.image(hash)),
        })
    }
}

impl Inner {
    /// Saves an addition, then deletes the image files it pushed out of the history.
    fn added(&self, added: Added) {
        store::save(FILE, &self.history);
        self.clips.remove(&added.unreferenced);
    }

    /// The clipboard emptied soon after text was copied: a password manager clearing a password, which then
    /// leaves the history too, unless pinned.
    fn forget_secret(&mut self) -> bool {
        let Some((id, at)) = self.last_text.take() else {
            return false;
        };
        if now_ms() - at > SECRET_CLEARED_MS || self.history.get(id).is_none_or(|item| item.pinned) {
            return false;
        }
        if let Some(unreferenced) = self.history.remove(id) {
            store::save(FILE, &self.history);
            self.clips.remove(&unreferenced);
        }
        true
    }
}

/// An `Items` entry, as Clipboard1's, with `paths` (the files' own paths), `app_name` and `thumb` as a URL.
#[derive(Serialize)]
struct ItemView<'a> {
    id: u64,
    kind: &'static str,
    preview: String,
    uris: &'a [String],
    paths: Vec<String>,
    thumb: Option<String>,
    thumb_width: Option<u32>,
    thumb_height: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
    app_id: &'a str,
    app_name: String,
    at_ms: i64,
    pinned: bool,
}

/// A thumbnail's bytes for the `isleclip` scheme: only `<hash>.card.png` files from the clips folder.
pub fn thumb_file(name: &str) -> Option<Vec<u8>> {
    let hash = name.strip_suffix(".card.png")?;
    if hash.is_empty() || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    fs::read(store::data_dir().join("clips").join(name)).ok()
}

/// A path as a `file://` URI.
pub fn uri_of_path(path: &str) -> String {
    crate::uri::from_path(Path::new(path))
}

/// A `file://` URI's path on Windows.
pub fn path_of_uri(uri: &str) -> String {
    crate::uri::to_path(uri).map(|path| path.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Whether the program at `path` is one whose copies are left out: a known password manager, or one of
/// `ignored` (program names, lower case).
pub fn is_ignored(path: &str, ignored: &[String]) -> bool {
    let stem = Path::new(path).file_stem().map(|stem| stem.to_string_lossy().to_lowercase()).unwrap_or_default();
    !stem.is_empty() && (IGNORED_APPS.contains(&stem.as_str()) || ignored.iter().any(|name| *name == stem))
}

/// The folder of stored images: `<hash>.png`, the image as copied, and `<hash>.card.png`, its thumbnail.
struct ClipFiles {
    dir: PathBuf,
}

impl ClipFiles {
    fn image(&self, hash: &str) -> PathBuf {
        self.dir.join(format!("{hash}.png"))
    }

    fn thumb(&self, hash: &str) -> PathBuf {
        self.dir.join(format!("{hash}.card.png"))
    }

    fn has(&self, hash: &str) -> bool {
        self.image(hash).is_file() && self.thumb(hash).is_file()
    }

    /// Stores a PNG and its thumbnail, returning the image's size.
    fn store(&self, hash: &str, png: &[u8]) -> anyhow::Result<(u32, u32)> {
        let image = image::load_from_memory_with_format(png, ImageFormat::Png)?;
        fs::create_dir_all(&self.dir)?;
        write_replacing(&self.image(hash), png)?;
        self.write_thumb(hash, &image, png)?;
        Ok((image.width(), image.height()))
    }

    fn remake_thumb(&self, hash: &str) -> anyhow::Result<()> {
        let png = fs::read(self.image(hash))?;
        let image = image::load_from_memory_with_format(&png, ImageFormat::Png)?;
        self.write_thumb(hash, &image, &png)
    }

    /// Writes the thumbnail `ThumbPlan` makes of `image`, whose PNG is `png`.
    fn write_thumb(&self, hash: &str, image: &DynamicImage, png: &[u8]) -> anyhow::Result<()> {
        let (width, height) = (image.width(), image.height());
        let plan = ThumbPlan::new(width, height);
        let size = plan.size();
        if size == (width, height) {
            // The whole image as it is: scaling it up would only blur it.
            write_replacing(&self.thumb(hash), png)?;
        } else {
            let mut thumb = image.crop_imm(plan.x, plan.y, plan.width, plan.height);
            if (plan.width, plan.height) != size {
                thumb = thumb.thumbnail_exact(size.0, size.1);
            }
            let mut bytes = Cursor::new(Vec::new());
            thumb.write_to(&mut bytes, ImageFormat::Png)?;
            write_replacing(&self.thumb(hash), bytes.get_ref())?;
        }
        Ok(())
    }

    /// Deletes the files of images no item refers to any more.
    fn remove(&self, hashes: &[String]) {
        for hash in hashes {
            remove_file(&self.image(hash));
            remove_file(&self.thumb(hash));
        }
    }

    /// Deletes files not in `referenced`: images dropped while Isle was not running, and half-written ones.
    fn remove_orphans(&self, referenced: &HashSet<&str>) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let hash = name
                .to_str()
                .and_then(|name| name.strip_suffix(".card.png").or_else(|| name.strip_suffix(".png")));
            if !hash.is_some_and(|hash| referenced.contains(hash)) {
                remove_file(&entry.path());
            }
        }
    }
}

/// What of an image becomes its thumbnail (isled's `ThumbPlan`): its middle cut to the card's shape and
/// scaled down, or, for an image too small or thin to fill that, its middle at its own pixels.
#[derive(Debug, PartialEq, Eq)]
struct ThumbPlan {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    scaled: bool,
}

impl ThumbPlan {
    fn new(width: u32, height: u32) -> Self {
        let (cut_width, cut_height, scaled) = if width >= THUMB_WIDTH && height >= THUMB_HEIGHT {
            let (w, h) = (u64::from(width), u64::from(height));
            let (tw, th) = (u64::from(THUMB_WIDTH), u64::from(THUMB_HEIGHT));
            if w * th > h * tw {
                (((h * tw + th / 2) / th).min(w) as u32, height, true)
            } else {
                (width, ((w * th + tw / 2) / tw).min(h) as u32, true)
            }
        } else {
            (width.min(THUMB_WIDTH), height.min(THUMB_HEIGHT), false)
        };
        Self { x: (width - cut_width) / 2, y: (height - cut_height) / 2, width: cut_width, height: cut_height, scaled }
    }

    fn size(&self) -> (u32, u32) {
        if self.scaled { (THUMB_WIDTH, THUMB_HEIGHT) } else { (self.width, self.height) }
    }
}

/// Writes through a temporary file renamed into place, so an interrupted write never leaves half an image.
fn write_replacing(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut temp = path.as_os_str().to_owned();
    temp.push(format!(".{n}.tmp"));
    fs::write(&temp, bytes)?;
    fs::rename(&temp, path)
}

fn remove_file(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => debug!("could not remove {}: {err}", path.display()),
    }
}

/// A Windows bitmap (CF_DIB: its header and pixels, without the file's header) as a PNG.
pub fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    let u32_at = |at: usize| dib.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let header = u32_at(0)? as usize;
    let bits = dib.get(14..16).map(|b| u16::from_le_bytes([b[0], b[1]]))?;
    let compression = u32_at(16)?;
    let used = u32_at(32).unwrap_or(0) as usize;
    // BI_BITFIELDS after a plain 40-byte header: three masks follow it.
    let masks = if compression == 3 && header == 40 { 12 } else { 0 };
    let colors = if bits <= 8 { if used == 0 { 1 << bits } else { used } } else { used } * 4;
    let offset = 14 + header + masks + colors;
    let mut file = Vec::with_capacity(14 + dib.len());
    file.extend_from_slice(b"BM");
    file.extend_from_slice(&((14 + dib.len()) as u32).to_le_bytes());
    file.extend_from_slice(&0u32.to_le_bytes());
    file.extend_from_slice(&(offset as u32).to_le_bytes());
    file.extend_from_slice(dib);
    let image = image::load_from_memory_with_format(&file, ImageFormat::Bmp).ok()?;
    let mut png = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image.to_rgba8()).write_to(&mut png, ImageFormat::Png).ok()?;
    Some(png.into_inner())
}

/// A PNG as a Windows bitmap (CF_DIB): 32-bit, bottom row first.
pub fn png_to_dib(png: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory_with_format(png, ImageFormat::Png).ok()?.to_rgba8();
    let (width, height) = image.dimensions();
    let mut dib = Vec::with_capacity(40 + (width * height * 4) as usize);
    for value in [40u32, width, height] {
        dib.extend_from_slice(&value.to_le_bytes());
    }
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&32u16.to_le_bytes());
    for value in [0u32, width * height * 4, 0, 0, 0, 0] {
        dib.extend_from_slice(&value.to_le_bytes());
    }
    for row in image.rows().rev() {
        for pixel in row {
            let [r, g, b, a] = pixel.0;
            dib.extend_from_slice(&[b, g, r, a]);
        }
    }
    Some(dib)
}

#[cfg(windows)]
pub mod imp {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::thread;
    use std::time::Duration;

    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::DataExchange::{
        AddClipboardFormatListener, CloseClipboard, CountClipboardFormats, EmptyClipboard, GetClipboardData,
        GetClipboardOwner, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
        VK_CONTROL,
    };
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetForegroundWindow, GetMessageW, HWND_MESSAGE, MSG,
        RegisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLIPBOARDUPDATE, WNDCLASSW,
    };
    use windows::core::{PCWSTR, w};

    use super::Copied;

    const CF_UNICODETEXT: u32 = 13;
    const CF_HDROP: u32 = 15;
    const CF_DIB: u32 = 8;
    /// Larger text is not kept (Clip refuses it too); read no further.
    const MAX_TEXT_BYTES: usize = 512 * 1024;
    /// A bitmap past this many bytes is not read (an 8K screenshot is about 130 MB).
    const MAX_DIB_BYTES: usize = 160 * 1024 * 1024;

    /// The hidden window that hears copies; Isle's own copies are made with it, which tells them apart.
    static OWN: AtomicIsize = AtomicIsize::new(0);

    thread_local! {
        static ON_COPY: RefCell<Option<Box<dyn FnMut()>>> = RefCell::new(None);
    }

    pub use crate::app_usage::imp::{front_app, icon, name_of};

    fn format(name: PCWSTR) -> u32 {
        // SAFETY: registers (or looks up) a clipboard format by a constant name.
        unsafe { RegisterClipboardFormatW(name) }
    }

    unsafe extern "system" fn proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if message == WM_CLIPBOARDUPDATE {
            ON_COPY.with(|on_copy| {
                if let Some(on_copy) = on_copy.borrow_mut().as_mut() {
                    on_copy();
                }
            });
            return LRESULT(0);
        }
        // SAFETY: the default handling of every other message.
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    /// Calls `on_copy` after each copy another program makes, for ever (run on a thread of its own).
    pub fn listen(on_copy: impl FnMut() + 'static) {
        ON_COPY.with(|slot| *slot.borrow_mut() = Some(Box::new(on_copy)));
        // SAFETY: a message-only window of a class registered here, and its message loop, all on this thread.
        unsafe {
            let instance = GetModuleHandleW(None).unwrap_or_default();
            let class = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: instance.into(),
                lpszClassName: w!("IsleClipboard"),
                ..Default::default()
            };
            RegisterClassW(&class);
            let Ok(hwnd) = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("IsleClipboard"),
                w!("Isle clipboard"),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(instance.into()),
                None,
            ) else {
                log::warn!("could not watch the clipboard");
                return;
            };
            OWN.store(hwnd.0 as isize, Ordering::Relaxed);
            if AddClipboardFormatListener(hwnd).is_err() {
                log::warn!("could not watch the clipboard");
                return;
            }
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).as_bool() {
                DispatchMessageW(&message);
            }
        }
    }

    fn own() -> HWND {
        HWND(OWN.load(Ordering::Relaxed) as *mut _)
    }

    /// Whether Isle made the last copy itself (putting an item back).
    pub fn own_copy() -> bool {
        // SAFETY: a plain query.
        unsafe { GetClipboardOwner() }.is_ok_and(|owner| owner == own())
    }

    /// Opens the clipboard, waiting a little for a program that holds it.
    fn open() -> bool {
        for _ in 0..20 {
            // SAFETY: opened for our hidden window; every caller closes it.
            if unsafe { OpenClipboard(Some(own())) }.is_ok() {
                return true;
            }
            thread::sleep(Duration::from_millis(15));
        }
        false
    }

    fn available(format: u32) -> bool {
        // SAFETY: a plain query while the clipboard is open.
        unsafe { IsClipboardFormatAvailable(format) }.is_ok()
    }

    /// The bytes of `format`, at most `max` of them (more: none).
    fn bytes(format: u32, max: usize) -> Option<Vec<u8>> {
        // SAFETY: reads the clipboard's memory for `format` while it is locked, within the size it reports.
        unsafe {
            let handle = GetClipboardData(format).ok()?;
            let global = HGLOBAL(handle.0);
            let size = GlobalSize(global);
            if size == 0 || size > max {
                return None;
            }
            let data = GlobalLock(global);
            if data.is_null() {
                return None;
            }
            let bytes = std::slice::from_raw_parts(data as *const u8, size).to_vec();
            let _ = GlobalUnlock(global);
            Some(bytes)
        }
    }

    fn text() -> Option<String> {
        let bytes = bytes(CF_UNICODETEXT, MAX_TEXT_BYTES * 2 + 2)?;
        let wide: Vec<u16> = bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).take_while(|&c| c != 0).collect();
        Some(String::from_utf16_lossy(&wide))
    }

    fn files() -> Vec<String> {
        // SAFETY: reads the dropped files' list the clipboard holds, each name into a buffer of its length.
        unsafe {
            let Ok(handle) = GetClipboardData(CF_HDROP) else {
                return Vec::new();
            };
            let drop = HDROP(handle.0);
            let count = DragQueryFileW(drop, u32::MAX, None);
            (0..count)
                .filter_map(|index| {
                    let len = DragQueryFileW(drop, index, None) as usize;
                    let mut name = vec![0u16; len + 1];
                    let read = DragQueryFileW(drop, index, Some(&mut name)) as usize;
                    (read > 0).then(|| String::from_utf16_lossy(&name[..read]))
                })
                .collect()
        }
    }

    /// What is on the clipboard now: files first; then text (a spreadsheet's cells come with a picture of
    /// them too), unless it is only the address of a picture copied with it; else an image.
    pub fn read() -> Copied {
        // The copying program may still be adding formats; reading at once would keep it from the clipboard.
        thread::sleep(Duration::from_millis(30));
        if !open() {
            return Copied::Other;
        }
        let copied = read_open();
        // SAFETY: closes what `open` opened.
        let _ = unsafe { CloseClipboard() };
        match copied {
            Raw::Dib(dib) => super::dib_to_png(&dib).map_or(Copied::Other, Copied::Image),
            Raw::Copied(copied) => copied,
        }
    }

    enum Raw {
        Copied(Copied),
        Dib(Vec<u8>),
    }

    fn read_open() -> Raw {
        // SAFETY: a plain query while the clipboard is open.
        if unsafe { CountClipboardFormats() } == 0 {
            return Raw::Copied(Copied::Empty);
        }
        // Password managers mark what they copy as private, as Windows' own clipboard history asks.
        let exclude = format(w!("ExcludeClipboardContentFromMonitorProcessing"));
        let ignore = format(w!("Clipboard Viewer Ignore"));
        let history = format(w!("CanIncludeInClipboardHistory"));
        let kept_out = available(history) && bytes(history, 64).is_some_and(|b| b.get(..4) == Some(&[0, 0, 0, 0]));
        if available(exclude) || available(ignore) || kept_out {
            return Raw::Copied(Copied::Secret);
        }
        if available(CF_HDROP) {
            let files = files();
            if !files.is_empty() {
                return Raw::Copied(Copied::Files(files));
            }
        }
        let png = format(w!("PNG"));
        let picture = available(png) || available(CF_DIB);
        if available(CF_UNICODETEXT)
            && let Some(text) = text()
        {
            let trimmed = text.trim();
            let address = picture
                && !trimmed.contains(char::is_whitespace)
                && ["http:", "https:", "data:", "file:"].iter().any(|scheme| trimmed.starts_with(scheme));
            if !trimmed.replace('\0', "").is_empty() && !address {
                return Raw::Copied(Copied::Text(text));
            }
        }
        if available(png)
            && let Some(bytes) = bytes(png, super::MAX_IMAGE_BYTES)
        {
            return Raw::Copied(Copied::Image(bytes));
        }
        if available(CF_DIB)
            && let Some(dib) = bytes(CF_DIB, MAX_DIB_BYTES)
        {
            return Raw::Dib(dib);
        }
        Raw::Copied(Copied::Other)
    }

    /// Hands `bytes` to the open clipboard as `format`.
    fn put(format: u32, bytes: &[u8]) {
        // SAFETY: fills fresh movable memory of the right size; the clipboard owns it once set, else it is freed.
        unsafe {
            let Ok(global) = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) else {
                return;
            };
            let data = GlobalLock(global);
            if data.is_null() {
                let _ = GlobalFree(Some(global));
                return;
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), data as *mut u8, bytes.len());
            let _ = GlobalUnlock(global);
            if SetClipboardData(format, Some(HANDLE(global.0))).is_err() {
                let _ = GlobalFree(Some(global));
            }
        }
    }

    fn wide(text: &str) -> Vec<u8> {
        text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
    }

    /// Puts text back on the clipboard.
    pub fn set_text(text: &str) -> bool {
        set(|| put(CF_UNICODETEXT, &wide(text)))
    }

    /// Puts text on the clipboard for a moment, to be pasted (dictation): kept out of Windows' clipboard history
    /// and its cloud clipboard.
    pub fn set_text_passing(text: &str) -> bool {
        set(|| {
            put(CF_UNICODETEXT, &wide(text));
            put(format(w!("ExcludeClipboardContentFromMonitorProcessing")), &[0]);
            put(format(w!("CanIncludeInClipboardHistory")), &0u32.to_le_bytes());
            put(format(w!("CanUploadToCloudClipboard")), &0u32.to_le_bytes());
        })
    }

    /// Puts files back on the clipboard, to be pasted as copies.
    pub fn set_files(paths: &[String]) -> bool {
        // DROPFILES: where the names start, a point, whether it is in the non-client area, wide names.
        let mut drop = Vec::new();
        for value in [20u32, 0, 0, 0, 1] {
            drop.extend_from_slice(&value.to_le_bytes());
        }
        for path in paths {
            drop.extend(wide(path));
        }
        drop.extend_from_slice(&[0, 0]);
        set(|| {
            put(CF_HDROP, &drop);
            // DROPEFFECT_COPY: a paste copies the files rather than moving them.
            put(format(w!("Preferred DropEffect")), &1u32.to_le_bytes());
        })
    }

    /// Puts an image back on the clipboard, as a PNG and a bitmap for the apps that only read those.
    pub fn set_image(png: &[u8]) -> bool {
        let dib = super::png_to_dib(png);
        set(|| {
            put(format(w!("PNG")), png);
            if let Some(dib) = &dib {
                put(CF_DIB, dib);
            }
        })
    }

    fn set(fill: impl FnOnce()) -> bool {
        if !open() {
            return false;
        }
        // SAFETY: empties the clipboard we opened, making our hidden window its owner, and closes it.
        unsafe {
            let _ = EmptyClipboard();
            fill();
            let _ = CloseClipboard();
        }
        true
    }

    /// The window in front.
    pub fn foreground() -> isize {
        // SAFETY: a plain query.
        unsafe { GetForegroundWindow() }.0 as isize
    }

    /// Presses Ctrl+V in the window in front, unless that is the island (`island`) itself.
    pub fn press_paste(island: isize) {
        // SAFETY: a plain query, then synthetic key presses.
        unsafe {
            if GetForegroundWindow().0 as isize == island {
                return;
            }
            let key = |vk: VIRTUAL_KEY, up: bool| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: vk,
                        dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                        ..Default::default()
                    },
                },
            };
            const VK_V: VIRTUAL_KEY = VIRTUAL_KEY(0x56);
            let inputs = [key(VK_CONTROL, false), key(VK_V, false), key(VK_V, true), key(VK_CONTROL, true)];
            SendInput(&inputs, size_of::<INPUT>() as i32);
        }
    }
}

#[cfg(not(windows))]
pub mod imp {
    use super::Copied;

    pub fn listen(_on_copy: impl FnMut() + 'static) {}
    pub fn own_copy() -> bool {
        false
    }
    pub fn read() -> Copied {
        Copied::Other
    }
    pub fn set_text(_text: &str) -> bool {
        false
    }
    pub fn set_files(_paths: &[String]) -> bool {
        false
    }
    pub fn set_text_passing(_text: &str) -> bool {
        false
    }
    pub fn set_image(_png: &[u8]) -> bool {
        false
    }
    pub fn press_paste(_island: isize) {}
    pub fn foreground() -> isize {
        0
    }
    pub fn front_app() -> Option<(String, String)> {
        None
    }
    pub fn icon(_path: &str) -> String {
        String::new()
    }
    pub fn name_of(path: &str) -> String {
        path.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_uris_go_both_ways() {
        let path = r"C:\Users\Ana Lee\Pictures\café #1.png";
        let uri = uri_of_path(path);
        assert_eq!(uri, "file:///C:/Users/Ana%20Lee/Pictures/caf%C3%A9%20%231.png");
        assert_eq!(path_of_uri(&uri), path);
        let share = r"\\nas\photos\a.jpg";
        assert_eq!(uri_of_path(share), "file://nas/photos/a.jpg");
        assert_eq!(path_of_uri(&uri_of_path(share)), share);
    }

    #[test]
    fn a_files_card_shows_their_names() {
        let clip = Clip::from_uris(vec![uri_of_path(r"C:\a b\report final.pdf")]).unwrap();
        assert_eq!(clip.preview(), "report final.pdf");
    }

    #[test]
    fn password_managers_are_left_out() {
        let added = ["notepad".to_owned()];
        assert!(is_ignored(r"c:\program files\keepassxc\keepassxc.exe", &[]));
        assert!(is_ignored(r"C:\Program Files\Bitwarden\Bitwarden.exe", &[]));
        assert!(!is_ignored(r"c:\windows\notepad.exe", &[]));
        assert!(is_ignored(r"c:\windows\notepad.exe", &added));
        assert!(!is_ignored("", &added));
    }

    #[test]
    fn thumbnails_are_cut_as_on_gnome() {
        let plan = ThumbPlan::new(1920, 1080);
        assert_eq!((plan.x, plan.y, plan.width, plan.height, plan.size()), (280, 0, 1360, 1080, (272, 216)));
        assert_eq!(ThumbPlan::new(32, 32).size(), (32, 32));
    }

    #[test]
    fn bitmaps_and_pngs_convert_both_ways() {
        let image = image::RgbaImage::from_fn(5, 3, |x, y| image::Rgba([x as u8 * 40, y as u8 * 80, 7, 255]));
        let mut png = Cursor::new(Vec::new());
        image.write_to(&mut png, ImageFormat::Png).unwrap();
        let dib = png_to_dib(png.get_ref()).unwrap();
        let back = dib_to_png(&dib).unwrap();
        let decoded = image::load_from_memory(&back).unwrap().to_rgba8();
        assert_eq!(decoded, image);
    }

    #[test]
    fn only_card_thumbnails_are_served() {
        assert!(thumb_file("../settings.json").is_none());
        assert!(thumb_file("abc.png").is_none());
        assert!(thumb_file("..\\x.card.png").is_none());
    }
}
