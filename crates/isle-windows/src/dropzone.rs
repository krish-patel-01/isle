// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! Files dropped on the island (the shelf helper's part on GNOME, crates/isle-shelf). Over the island, Windows
//! finds the web view's drawing surfaces, which belong to Edge's own processes and turn drops away; so while
//! something is dragged onto the island from elsewhere, a window of Isle's own, all but invisible, lies over the
//! island to take the files. It tells the island as Tauri's drops would ("fileDrag": enter, over, drop, leave,
//! with the files' paths and where, in the island window's pixels), and goes when the button is let go.

use std::sync::Mutex;

/// The island's shape on screen (physical pixels: left, top, right, bottom), as the island last said.
static AREA: Mutex<Option<[i32; 4]>> = Mutex::new(None);
/// How far around the closed island a drag already counts as on it.
const SLACK: i32 = 30;

/// What `poll` saw last: whether the button was down, whether that press began on the island, and whether the
/// catcher is out.
#[derive(Default)]
pub struct Watch {
    held: bool,
    started_inside: bool,
    shown: bool,
    placed: Option<[i32; 4]>,
}

/// The island's shape: `rect` (x, y, width, height) in CSS px of the window at `origin` (physical) and `scale`.
pub fn set_area(rect: [f64; 4], origin: (i32, i32), scale: f64) {
    let [x, y, width, height] = rect.map(|value| (value * scale).round() as i32);
    let area = [origin.0 + x, origin.1 + y, origin.0 + x + width, origin.1 + y + height];
    log::debug!("drop area {area:?}");
    *AREA.lock().unwrap() = Some(area);
}

fn inside(area: [i32; 4], (x, y): (i32, i32), slack: i32) -> bool {
    x >= area[0] - slack && x <= area[2] + slack && y >= area[1] - slack && y <= area[3] + slack
}

impl Watch {
    /// Called often (the pointer's thread): shows the catcher once a press made elsewhere comes onto the island,
    /// and takes it away when the button is let go. Returns what to do: Some(area) to show it there, Some(None)
    /// to hide it, None for nothing.
    pub fn poll(&mut self) -> Option<Option<[i32; 4]>> {
        let area = (*AREA.lock().unwrap())?;
        let (held, cursor) = imp::pointer()?;
        if held && !self.held {
            self.started_inside = inside(area, cursor, 0);
            log::debug!("press at {cursor:?}, on the island {}", self.started_inside);
        }
        self.held = held;
        if held && !self.started_inside && !self.shown && inside(area, cursor, SLACK) {
            log::debug!("a drag reached the island at {cursor:?}");
            self.shown = true;
            self.placed = Some(area);
            return Some(Some(area));
        }
        if self.shown && !held {
            log::debug!("the button was let go");
            self.shown = false;
            self.placed = None;
            return Some(None);
        }
        // The island grew under the drag (it opened): the catcher follows.
        if self.shown && self.placed != Some(area) {
            self.placed = Some(area);
            return Some(Some(area));
        }
        None
    }
}

#[cfg(windows)]
pub mod imp {
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicIsize, Ordering};

    use serde_json::json;
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, POINTL, WPARAM};
    use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Ole::{
        CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl, RegisterDragDrop,
        ReleaseStgMedium,
    };
    use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, GetCursorPos, HWND_TOPMOST, LWA_ALPHA, RegisterClassW, SW_HIDE,
        SWP_NOACTIVATE, SWP_SHOWWINDOW, SetLayeredWindowAttributes, SetWindowPos, ShowWindow, WNDCLASSW,
        WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    };
    use windows::core::{Ref, implement, w};

    /// The catcher window, made once on the app's main thread.
    static CATCHER: AtomicIsize = AtomicIsize::new(0);

    thread_local! {
        /// Its drop target, alive as long as the window.
        static TARGET: RefCell<Option<IDropTarget>> = const { RefCell::new(None) };
    }

    /// Whether the left button is down, and where the pointer is (physical pixels).
    pub fn pointer() -> Option<(bool, (i32, i32))> {
        let mut point = POINT::default();
        // SAFETY: plain queries of the pointer.
        unsafe {
            GetCursorPos(&mut point).ok()?;
            Some(((GetAsyncKeyState(i32::from(VK_LBUTTON.0)) as u16 & 0x8000) != 0, (point.x, point.y)))
        }
    }

    unsafe extern "system" fn proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        // SAFETY: the default handling of every message.
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    /// Makes the catcher, hidden; on the main thread, where OLE runs. `tell(kind, paths, (x, y))` hears the
    /// drag, with the screen point. Owned by the island's window (`island`), so it always lies above it.
    pub fn create(island: isize, tell: Box<dyn Fn(&str, Vec<String>, (i32, i32))>) {
        // SAFETY: a window class and a hidden, all but transparent popup of it, and its drop target, on this
        // thread.
        unsafe {
            let instance = GetModuleHandleW(None).unwrap_or_default();
            let class = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: instance.into(),
                lpszClassName: w!("IsleDropZone"),
                ..Default::default()
            };
            RegisterClassW(&class);
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                w!("IsleDropZone"),
                w!("Isle drop zone"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                Some(HWND(island as *mut _)),
                None,
                Some(instance.into()),
                None,
            ) else {
                log::warn!("could not make the island's drop zone");
                return;
            };
            // One step above invisible: the system still finds it under the pointer.
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 1, LWA_ALPHA);
            let target: IDropTarget = Catcher { tell, valid: false.into() }.into();
            if let Err(err) = RegisterDragDrop(hwnd, &target) {
                log::warn!("could not take drops on the island: {err}");
                return;
            }
            TARGET.with(|slot| *slot.borrow_mut() = Some(target));
            CATCHER.store(hwnd.0 as isize, Ordering::Relaxed);
        }
    }

    /// Lays the catcher over `area` (physical: left, top, right, bottom), or hides it. On the main thread.
    pub fn place(area: Option<[i32; 4]>) {
        let hwnd = HWND(CATCHER.load(Ordering::Relaxed) as *mut _);
        if hwnd.is_invalid() {
            return;
        }
        log::debug!("drop zone at {area:?}");
        // SAFETY: moves or hides our own window.
        unsafe {
            match area {
                Some([left, top, right, bottom]) => {
                    let _ = SetWindowPos(
                        hwnd,
                        Some(HWND_TOPMOST),
                        left,
                        top,
                        right - left,
                        bottom - top,
                        SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                }
                None => {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
            }
        }
    }

    #[implement(IDropTarget)]
    struct Catcher {
        tell: Box<dyn Fn(&str, Vec<String>, (i32, i32))>,
        /// The drag brings files.
        valid: std::cell::Cell<bool>,
    }

    /// The paths of the files `data` carries; none when it carries no files.
    fn paths(data: Ref<'_, IDataObject>) -> Option<Vec<String>> {
        let format = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        // SAFETY: the dropped files' list, read whole and released.
        unsafe {
            let mut medium = match data.as_ref()?.GetData(&format) {
                Ok(medium) => medium,
                Err(err) => {
                    log::debug!("the drag carries no files: {err}");
                    return None;
                }
            };
            let drop = HDROP(medium.u.hGlobal.0);
            let count = DragQueryFileW(drop, u32::MAX, None);
            let paths = (0..count)
                .map(|index| {
                    let len = DragQueryFileW(drop, index, None) as usize;
                    let mut name = vec![0u16; len + 1];
                    DragQueryFileW(drop, index, Some(&mut name));
                    PathBuf::from(String::from_utf16_lossy(&name[..len])).to_string_lossy().into_owned()
                })
                .collect();
            ReleaseStgMedium(&mut medium);
            Some(paths)
        }
    }

    impl IDropTarget_Impl for Catcher_Impl {
        fn DragEnter(
            &self,
            data: Ref<'_, IDataObject>,
            _keys: MODIFIERKEYS_FLAGS,
            point: &POINTL,
            effect: *mut DROPEFFECT,
        ) -> windows::core::Result<()> {
            log::debug!("drag enters the drop zone");
            let paths = paths(data).filter(|paths| !paths.is_empty());
            self.valid.set(paths.is_some());
            if let Some(paths) = paths {
                (self.tell)("enter", paths, (point.x, point.y));
            }
            // SAFETY: the effect the caller gave room for.
            unsafe { *effect = if self.valid.get() { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
            Ok(())
        }

        fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, point: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
            if self.valid.get() {
                (self.tell)("over", Vec::new(), (point.x, point.y));
            }
            // SAFETY: as above.
            unsafe { *effect = if self.valid.get() { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
            Ok(())
        }

        fn DragLeave(&self) -> windows::core::Result<()> {
            if self.valid.replace(false) {
                (self.tell)("leave", Vec::new(), (0, 0));
            }
            Ok(())
        }

        fn Drop(
            &self,
            data: Ref<'_, IDataObject>,
            _keys: MODIFIERKEYS_FLAGS,
            point: &POINTL,
            effect: *mut DROPEFFECT,
        ) -> windows::core::Result<()> {
            if self.valid.replace(false) {
                (self.tell)("drop", paths(data).unwrap_or_default(), (point.x, point.y));
            }
            // SAFETY: as above.
            unsafe { *effect = DROPEFFECT_COPY };
            place(None);
            Ok(())
        }
    }

    /// The drag as the island hears it: Tauri's drag-drop payload, the point in the island window's pixels.
    pub fn payload(kind: &str, paths: Vec<String>, point: (i32, i32), origin: (i32, i32)) -> serde_json::Value {
        json!({"type": kind, "paths": paths, "position": {"x": point.0 - origin.0, "y": point.1 - origin.1}})
    }
}

#[cfg(not(windows))]
pub mod imp {
    pub fn pointer() -> Option<(bool, (i32, i32))> {
        None
    }
    pub fn create(_island: isize, _tell: Box<dyn Fn(&str, Vec<String>, (i32, i32))>) {}
    pub fn place(_area: Option<[i32; 4]>) {}
    pub fn payload(kind: &str, paths: Vec<String>, point: (i32, i32), origin: (i32, i32)) -> serde_json::Value {
        serde_json::json!({"type": kind, "paths": paths, "position": {"x": point.0 - origin.0, "y": point.1 - origin.1}})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_area_is_the_islands_rect_on_screen() {
        set_area([280.0, 0.0, 200.0, 32.0], (100, 0), 1.5);
        assert_eq!(*AREA.lock().unwrap(), Some([520, 0, 820, 48]));
        assert!(inside([520, 0, 820, 48], (500, 60), SLACK));
        assert!(!inside([520, 0, 820, 48], (500, 60), 0));
    }
}
