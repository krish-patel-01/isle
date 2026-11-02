// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! Copies the GNOME extension's icons and Isle's Inter font into `ui/` (they are not kept twice in
//! the repository), then lets Tauri embed `ui/` in the executable.

use std::fs;
use std::path::Path;

fn copy_dir(from: &Path, to: &Path, extension: &str) {
    println!("cargo:rerun-if-changed={}", from.display());
    fs::create_dir_all(to).expect("could not make a folder in ui/");
    for entry in fs::read_dir(from).expect("could not read the icons").flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == extension) {
            let target = to.join(entry.file_name());
            let fresh = fs::read(&path).ok();
            if fs::read(&target).ok() != fresh {
                fs::copy(&path, &target).expect("could not copy into ui/");
            }
        }
    }
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo = root.join("../..");
    copy_dir(&repo.join("gnome-extension/icons"), &root.join("ui/icons"), "svg");
    copy_dir(&repo.join("data/fonts"), &root.join("ui/fonts"), "ttf");
    tauri_build::build();
}
