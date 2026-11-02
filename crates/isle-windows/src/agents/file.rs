// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! Writing another app's settings file safely: the helpers `isle-daemon/src/agents/file.rs` gives the hook
//! editor (`json_hooks.rs`, shared), without Unix permission bits.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The file's text; `None` when there is no such file.
pub fn read(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// `path` with `suffix` added to its name: `settings.json.isle-backup`.
pub fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path.file_name().unwrap_or_default());
    name.push(suffix);
    path.with_file_name(name)
}

/// Copies the file aside as `<name>.isle-backup`, unless one is there already (the first is the user's own).
pub fn backup_once(path: &Path) -> io::Result<()> {
    let backup = sibling(path, ".isle-backup");
    if !path.exists() || backup.symlink_metadata().is_ok() {
        return Ok(());
    }
    fs::copy(path, backup).map(drop)
}

/// Replaces the file with `text` all at once.
pub fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    let target = fs::canonicalize(path)
        .map(strip_verbatim)
        .unwrap_or_else(|_| path.to_path_buf());
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir)?;
    }
    let temp = sibling(&target, ".isle-tmp");
    let _ = fs::remove_file(&temp);
    let written = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, &target)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written
}

/// `\\?\C:\…` (what canonicalize gives on Windows) as `C:\…`.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path,
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A fresh, empty folder for one test.
    pub fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("isle-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_replace_and_back_up_once() {
        let dir = scratch("file");
        let path = dir.join("settings.json");
        fs::write(&path, "first").unwrap();
        backup_once(&path).unwrap();
        write_atomic(&path, "second").unwrap();
        backup_once(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        assert_eq!(fs::read_to_string(sibling(&path, ".isle-backup")).unwrap(), "first");
    }
}
