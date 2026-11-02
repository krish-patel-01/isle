// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! Where Isle keeps its files on Windows: `%LOCALAPPDATA%\Isle` for data, and a `cache` folder in it
//! for what can be fetched again (lyrics, covers). Each kind of data is one JSON file.

use std::fs;
use std::path::{Path, PathBuf};

use log::warn;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Isle's data folder.
pub fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Isle")
}

/// Isle's cache folder; the lyrics module (shared with Linux) asks for it by this name.
pub fn cache_dir() -> PathBuf {
    data_dir().join("cache")
}

/// The JSON file `name` in the data folder, or the default when it is missing or unreadable.
pub fn load<T: DeserializeOwned + Default>(name: &str) -> T {
    let path = data_dir().join(name);
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|err| {
            warn!("{} is unreadable, starting afresh: {err}", path.display());
            T::default()
        }),
        Err(_) => T::default(),
    }
}

/// Writes `value` to the JSON file `name` in the data folder, through a temporary file so a crash
/// never leaves half a file.
pub fn save<T: Serialize>(name: &str, value: &T) {
    let path = data_dir().join(name);
    if let Err(err) = write_atomic(&path, value) {
        warn!("could not save {}: {err}", path.display());
    }
}

fn write_atomic<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    fs::rename(&tmp, path)?;
    Ok(())
}
