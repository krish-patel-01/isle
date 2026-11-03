// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! Other apps' notifications, as GNOME's message tray gives them to the extension on Linux: read from
//! Windows' notification centre (the list behind the clock in the taskbar), newest first. The island shows
//! each new one, and the Notifications page lists them; putting one away there takes it out of Windows' list
//! too. Windows still shows its own banners; nothing lets another app stop those.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

/// Windows' list is read this often: nothing tells an app outside the Store that it changed.
const READ_EVERY: Duration = Duration::from_millis(1000);
/// The page keeps at most this many.
const MAX: usize = 50;

/// One notification, as the page and the alert draw it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Note {
    pub id: u32,
    /// The app's name, and its id to open it by.
    pub app: String,
    pub app_id: String,
    pub title: String,
    pub body: String,
    pub time_ms: i64,
    /// The app's icon as a `data:` URL, or "".
    pub icon: String,
    /// Not seen yet: a dot on the page's tab until the page opens.
    pub unseen: bool,
}

/// The notifications on show, and Do Not Disturb.
#[derive(Default)]
pub struct Center {
    notes: Mutex<Vec<Note>>,
    dnd: Mutex<bool>,
    /// Asked of the reading thread: dismiss one (Some(id)) or all (None).
    removals: Mutex<Vec<Option<u32>>>,
}

impl Center {
    /// The `notifications` state: `{dnd, items}`.
    pub fn state(&self) -> Value {
        serde_json::json!({"dnd": *self.dnd.lock().unwrap(), "items": *self.notes.lock().unwrap()})
    }

    /// Notifications1's methods; true when the state changed.
    pub fn call(&self, method: &str, arg: &Value) -> bool {
        match method {
            "Dismiss" => {
                let Some(id) = arg["id"].as_u64().and_then(|id| u32::try_from(id).ok()) else {
                    return false;
                };
                self.removals.lock().unwrap().push(Some(id));
                self.notes.lock().unwrap().retain(|note| note.id != id);
                true
            }
            "ClearAll" => {
                self.removals.lock().unwrap().push(None);
                self.notes.lock().unwrap().clear();
                true
            }
            "AcknowledgeAll" => {
                let mut notes = self.notes.lock().unwrap();
                let any = notes.iter().any(|note| note.unseen);
                notes.iter_mut().for_each(|note| note.unseen = false);
                any
            }
            "SetDnd" => {
                *self.dnd.lock().unwrap() = arg.as_bool().unwrap_or(false);
                true
            }
            "Open" => {
                let id = arg["id"].as_u64().unwrap_or_default();
                let app = self.notes.lock().unwrap().iter().find(|note| u64::from(note.id) == id).map(|note| note.app_id.clone());
                if let Some(app) = app.filter(|app| !app.is_empty()) {
                    imp::open_app(&app);
                }
                false
            }
            _ => false,
        }
    }
}

/// Reads Windows' notifications for as long as Isle runs: `changed` with each change of the list, `posted`
/// with each one new since Isle started.
pub fn watch(center: &Center, changed: impl Fn(), posted: impl Fn(&Note)) {
    #[cfg(windows)]
    {
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
        // SAFETY: once, at the start of this thread.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    }
    let Some(reader) = imp::Reader::new() else {
        log::info!("Windows does not let Isle read notifications (Settings → Privacy → Notifications)");
        return;
    };
    let mut icons: HashMap<String, String> = HashMap::new();
    let mut first = true;
    loop {
        for removal in std::mem::take(&mut *center.removals.lock().unwrap()) {
            reader.remove(removal);
        }
        if let Some(read) = reader.read(&mut icons) {
            let mut fresh = Vec::new();
            let differs = {
                let mut notes = center.notes.lock().unwrap();
                let before: Vec<u32> = notes.iter().map(|note| note.id).collect();
                let mut next: Vec<Note> = read
                    .into_iter()
                    .map(|mut note| {
                        // One already listed keeps whether it was seen; one there as Isle started counts as seen.
                        note.unseen = notes.iter().find(|old| old.id == note.id).map_or(!first, |old| old.unseen);
                        if !first && !before.contains(&note.id) {
                            fresh.push(note.clone());
                        }
                        note
                    })
                    .collect();
                next.sort_by(|a, b| b.time_ms.cmp(&a.time_ms));
                next.truncate(MAX);
                let differs = next != *notes;
                *notes = next;
                differs
            };
            if differs {
                changed();
            }
            // Oldest first, so the newest ends up on top.
            for note in fresh.iter().rev() {
                posted(note);
            }
            first = false;
        }
        std::thread::sleep(READ_EVERY);
    }
}

#[cfg(windows)]
mod imp {
    use std::collections::HashMap;

    use windows::Foundation::Size;
    use windows::Storage::Streams::DataReader;
    use windows::UI::Notifications::Management::{UserNotificationListener, UserNotificationListenerAccessStatus};
    use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds, UserNotification};

    use super::Note;

    /// 100 ns ticks between 1601 (Windows' epoch) and 1970.
    const EPOCH_TICKS: i64 = 116_444_736_000_000_000;

    pub struct Reader {
        listener: UserNotificationListener,
    }

    impl Reader {
        pub fn new() -> Option<Self> {
            let listener = UserNotificationListener::Current().ok()?;
            let status = listener.RequestAccessAsync().ok()?.get().ok()?;
            (status == UserNotificationListenerAccessStatus::Allowed).then_some(Self { listener })
        }

        pub fn read(&self, icons: &mut HashMap<String, String>) -> Option<Vec<Note>> {
            let list = self.listener.GetNotificationsAsync(NotificationKinds::Toast).ok()?.get().ok()?;
            Some(list.into_iter().filter_map(|item| note(&item, icons).ok()).collect())
        }

        pub fn remove(&self, id: Option<u32>) {
            let _ = match id {
                Some(id) => self.listener.RemoveNotification(id),
                None => self.listener.ClearNotifications(),
            };
        }
    }

    fn note(item: &UserNotification, icons: &mut HashMap<String, String>) -> windows::core::Result<Note> {
        let info = item.AppInfo()?;
        let app_id = info.AppUserModelId().map(|id| id.to_string()).unwrap_or_default();
        let display = info.DisplayInfo()?;
        let app = display.DisplayName().map(|name| name.to_string()).unwrap_or_default();
        let binding = item.Notification()?.Visual()?.GetBinding(&KnownNotificationBindings::ToastGeneric()?)?;
        let texts: Vec<String> = binding
            .GetTextElements()?
            .into_iter()
            .filter_map(|text| text.Text().ok().map(|t| t.to_string()))
            .filter(|text| !text.trim().is_empty())
            .collect();
        let icon = icons
            .entry(app_id.clone())
            .or_insert_with(|| {
                let logo = || -> windows::core::Result<Vec<u8>> {
                    let stream = display.GetLogo(Size { Width: 64.0, Height: 64.0 })?.OpenReadAsync()?.get()?;
                    let size = u32::try_from(stream.Size()?).unwrap_or(0);
                    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
                    reader.LoadAsync(size)?.get()?;
                    let mut bytes = vec![0; size as usize];
                    reader.ReadBytes(&mut bytes)?;
                    Ok(bytes)
                };
                logo().ok().filter(|bytes| !bytes.is_empty()).map(|bytes| crate::data_url(&bytes)).unwrap_or_default()
            })
            .clone();
        let mut texts = texts.into_iter();
        Ok(Note {
            id: item.Id()?,
            title: texts.next().unwrap_or_else(|| app.clone()),
            body: texts.collect::<Vec<_>>().join("\n"),
            app,
            app_id,
            time_ms: (item.CreationTime()?.UniversalTime - EPOCH_TICKS) / 10_000,
            icon,
            unseen: false,
        })
    }

    /// Opens the app a notification came from, or brings it forward.
    pub fn open_app(app_id: &str) {
        // An app id holds no quotes or spaces that would reach the shell as more than one argument.
        if app_id.chars().any(|c| c.is_whitespace() || c == '"') {
            return;
        }
        let _ = std::process::Command::new("explorer.exe").arg(format!("shell:AppsFolder\\{app_id}")).spawn();
    }
}

#[cfg(not(windows))]
mod imp {
    use std::collections::HashMap;

    use super::Note;

    pub struct Reader;

    impl Reader {
        pub fn new() -> Option<Self> {
            None
        }

        pub fn read(&self, _icons: &mut HashMap<String, String>) -> Option<Vec<Note>> {
            None
        }

        pub fn remove(&self, _id: Option<u32>) {}
    }

    pub fn open_app(_app_id: &str) {}
}
