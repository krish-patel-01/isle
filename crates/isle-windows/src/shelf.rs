// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! The Shelf's Files tab (isled's shelf.rs, Shelf1): up to six files parked on the island to drag or copy into
//! any app later. It never copies or moves the user's files; what the Shelf keeps itself (a clipboard picture, a
//! file from a phone) lives in `shelf`, and goes when taken off. Saved in `shelf.json`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use isle_core::shelf::{Shelf, ShelfItem};
use log::debug;
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::timer::now_ms;
use crate::{store, uri};

const FILE: &str = "shelf.json";
/// A file that doesn't say whether it is there within this (a share gone offline) is taken to be.
const CHECK_WITHIN: Duration = Duration::from_secs(2);

/// Where copies made for the shelf live (clipboard pictures, files from a phone).
pub fn owned_dir() -> PathBuf {
    store::data_dir().join("shelf")
}

/// Where the tiles' previews are kept, by the MD5 of the file's URI (the phone's page shows them too).
pub fn previews_dir() -> PathBuf {
    store::cache_dir().join("shelf")
}

/// Whether `path` is a file right in the shelf's own folder.
pub fn is_copy(path: &Path) -> bool {
    path.parent() == Some(owned_dir().as_path()) && path.file_name().is_some()
}

pub struct ShelfStore {
    shelf: Mutex<Shelf>,
    /// Counts changes, for phone sharing's page to follow.
    changes: watch::Sender<u64>,
}

/// Why files could not all go on the shelf.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Only so many fit.
    Full,
    /// Not files on this computer.
    NotLocal,
}

impl ShelfStore {
    /// Loads the saved shelf, deleting copies no longer on it.
    pub fn load() -> Self {
        let shelf: Shelf = store::load(FILE);
        if let Ok(entries) = std::fs::read_dir(owned_dir()) {
            for path in entries.flatten().map(|entry| entry.path()) {
                let listed = shelf.items().iter().any(|item| uri::to_path(&item.uri).as_ref() == Some(&path));
                if !listed {
                    remove_file(&path);
                }
            }
        }
        Self { shelf: Mutex::new(shelf), changes: watch::Sender::new(0) }
    }

    /// The `Items` property of Shelf1, with each file's `path` and whether the Shelf keeps it itself (`own`).
    pub fn items(&self) -> Value {
        let shelf = self.shelf.lock().unwrap();
        Value::Array(
            shelf
                .items()
                .iter()
                .map(|item| {
                    let path = uri::to_path(&item.uri);
                    json!({
                        "uri": item.uri,
                        "name": item.name,
                        "added_at_ms": item.added_at_ms,
                        "path": path.as_ref().map(|path| path.to_string_lossy()),
                        "own": path.as_deref().is_some_and(is_copy),
                    })
                })
                .collect(),
        )
    }

    /// The files on the shelf, newest first.
    pub fn list(&self) -> Vec<ShelfItem> {
        self.shelf.lock().unwrap().items().to_vec()
    }

    /// Changes each time the shelf does.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    fn changed(&self, shelf: &Shelf) {
        store::save(FILE, shelf);
        self.changes.send_modify(|count| *count += 1);
    }

    /// Puts files on the shelf by path (those there already move to the front). Those that don't fit are
    /// refused, the rest added; returns whether it changed, or why some were refused.
    pub fn add(&self, paths: &[PathBuf]) -> (bool, Option<Refused>) {
        let mut shelf = self.shelf.lock().unwrap();
        let mut added = false;
        let mut refused = None;
        for path in paths {
            if !path.is_absolute() || !path.exists() {
                refused.get_or_insert(Refused::NotLocal);
                continue;
            }
            let item = ShelfItem {
                uri: uri::from_path(path),
                name: path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
                added_at_ms: now_ms(),
            };
            if shelf.add(item) {
                added = true;
            } else {
                refused = Some(Refused::Full);
            }
        }
        if added {
            self.changed(&shelf);
        }
        (added, refused)
    }

    /// Puts a file the Shelf keeps itself on it; false (and the file deleted) when it is full.
    pub fn add_received(&self, path: &Path) -> bool {
        let (added, _) = self.add(&[path.to_path_buf()]);
        if !added {
            remove_file(path);
        }
        added
    }

    /// Takes one file off the shelf, deleting it only if it is the Shelf's own copy.
    pub fn remove(&self, uri: &str) -> bool {
        let mut shelf = self.shelf.lock().unwrap();
        if !shelf.remove(uri) {
            return false;
        }
        forget(uri);
        self.changed(&shelf);
        true
    }

    /// Empties the shelf, deleting the Shelf's own copies.
    pub fn clear(&self) -> bool {
        let mut shelf = self.shelf.lock().unwrap();
        if shelf.items().is_empty() {
            return false;
        }
        for item in shelf.items() {
            forget(&item.uri);
        }
        shelf.clear();
        self.changed(&shelf);
        true
    }

    /// Drops files that no longer exist (moved or deleted); a file that doesn't answer stays.
    pub fn refresh(&self) -> bool {
        let uris: Vec<String> = self.list().into_iter().map(|item| item.uri).collect();
        let gone: Vec<String> = uris
            .into_iter()
            .filter(|uri| {
                let (answer, check) = std::sync::mpsc::channel();
                let path = uri::to_path(uri);
                std::thread::spawn(move || answer.send(path.is_some_and(|path| path.exists())));
                check.recv_timeout(CHECK_WITHIN) == Ok(false)
            })
            .collect();
        if gone.is_empty() {
            return false;
        }
        let mut shelf = self.shelf.lock().unwrap();
        if shelf.retain_existing(|item| !gone.contains(&item.uri)) {
            self.changed(&shelf);
            return true;
        }
        false
    }

    /// Whether `uri` is on the shelf.
    pub fn has(&self, uri: &str) -> bool {
        self.shelf.lock().unwrap().items().iter().any(|item| item.uri == uri)
    }
}

/// Deletes the Shelf's own copy of a file; the user's own files are never touched.
fn forget(uri: &str) {
    if let Some(path) = uri::to_path(uri)
        && is_copy(&path)
    {
        remove_file(&path);
    }
}

fn remove_file(path: &Path) {
    if let Err(err) = std::fs::remove_file(path) {
        debug!("could not remove {}: {err}", path.display());
    }
}

/// `name`, or "name (n).ext" for the `n`th try (downloads.js `numbered`).
pub fn numbered(name: &str, n: u32) -> String {
    if n <= 1 {
        return name.to_owned();
    }
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} ({n}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} ({n})"),
    }
}

/// Copies a file the Shelf keeps into Downloads under its own name, numbered rather than over a file.
pub fn save_to_downloads(path: &Path) -> std::io::Result<PathBuf> {
    let dir = dirs::download_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Downloads"));
    std::fs::create_dir_all(&dir)?;
    let name = path.file_name().map_or_else(|| "File".to_owned(), |name| name.to_string_lossy().into_owned());
    for n in 1..=999 {
        let target = dir.join(numbered(&name, n));
        // Created new, so an existing file is never written over.
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&target) {
            Ok(mut file) => {
                let mut source = std::fs::File::open(path)?;
                if let Err(err) = std::io::copy(&mut source, &mut file) {
                    drop(file);
                    let _ = std::fs::remove_file(&target);
                    return Err(err);
                }
                return Ok(target);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "too many files of that name"))
}

/// A free name in the Shelf's own folder for a new copy: "Clipboard image 2026-11-04 at 09.12.03.png", " 2"…
pub fn own_path(stem: &str, ext: &str) -> PathBuf {
    let dir = owned_dir();
    let _ = std::fs::create_dir_all(&dir);
    (1..)
        .map(|n| dir.join(if n == 1 { format!("{stem}.{ext}") } else { format!("{stem} {n}.{ext}") }))
        .find(|path| !path.exists())
        .unwrap_or_else(|| dir.join(format!("{stem}.{ext}")))
}

/// The preview of `uri` for its tile: `{image, whole}` (a `data:` URL), else `{icon}`. Made once and kept in
/// `previews_dir`, where the phone's page finds pictures' previews too.
pub fn preview(uri: &str) -> Value {
    let Some(path) = uri::to_path(uri) else {
        return Value::Null;
    };
    let cached = previews_dir().join(format!("{:x}.png", md5::compute(uri)));
    let made = std::fs::read(&cached).ok().map(|png| (png, !crate::thumbs::is_picture(&path))).or_else(|| {
        let preview = crate::thumbs::preview(&path)?;
        let _ = std::fs::create_dir_all(previews_dir());
        let _ = std::fs::write(&cached, &preview.png);
        Some((preview.png, preview.whole))
    });
    match made {
        Some((png, whole)) => json!({"image": crate::data_url(&png), "whole": whole}),
        None => json!({"icon": crate::clipboard::imp::icon(&path.to_string_lossy())}),
    }
}

/// Deletes the previews of files no longer on the shelf.
pub fn tidy_previews(items: &[ShelfItem]) {
    let keep: Vec<String> = items.iter().map(|item| format!("{:x}.png", md5::compute(&item.uri))).collect();
    if let Ok(entries) = std::fs::read_dir(previews_dir()) {
        for entry in entries.flatten() {
            if !keep.iter().any(|name| entry.file_name().to_string_lossy() == *name) {
                remove_file(&entry.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_numbered_before_the_extension() {
        assert_eq!(numbered("Photo.jpg", 1), "Photo.jpg");
        assert_eq!(numbered("Photo.jpg", 2), "Photo (2).jpg");
        assert_eq!(numbered("README", 3), "README (3)");
        assert_eq!(numbered(".env", 2), ".env (2)");
    }
}
