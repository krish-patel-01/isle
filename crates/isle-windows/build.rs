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

/// Whether the newest Visual C++ toolset's static C++ library lacks the STL helpers sherpa-onnx's prebuilt
/// libraries call (see src/msvc_stl.rs): then Isle brings its own.
fn old_msvc() -> bool {
    let mut newest: Option<std::path::PathBuf> = None;
    for root in [r"C:\Program Files\Microsoft Visual Studio", r"C:\Program Files (x86)\Microsoft Visual Studio"] {
        let Ok(years) = fs::read_dir(root) else { continue };
        for edition in years.flatten().filter_map(|year| fs::read_dir(year.path()).ok()).flatten().flatten() {
            let Ok(versions) = fs::read_dir(edition.path().join(r"VC\Tools\MSVC")) else { continue };
            for version in versions.flatten() {
                let lib = version.path().join("lib").join("x64").join("libcpmt.lib");
                if lib.is_file() && newest.as_ref().is_none_or(|best| version.path() > best.parent().unwrap().parent().unwrap().parent().unwrap().to_path_buf()) {
                    newest = Some(lib);
                }
            }
        }
    }
    let Some(lib) = newest else { return false };
    let Ok(bytes) = fs::read(&lib) else { return false };
    !bytes.windows(14).any(|window| window == b"__std_remove_8")
}

fn main() {
    println!("cargo:rustc-check-cfg=cfg(old_msvc)");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") && old_msvc() {
        println!("cargo:rustc-cfg=old_msvc");
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo = root.join("../..");
    copy_dir(&repo.join("gnome-extension/icons"), &root.join("ui/icons"), "svg");
    // An open tab shows its icon's `-fill` form; the Assistant's sparkle is solid already, so it is its own.
    let icons = root.join("ui/icons");
    fs::copy(icons.join("isle-sparkle-symbolic.svg"), icons.join("isle-sparkle-symbolic-fill.svg"))
        .expect("could not copy the sparkle icon");
    copy_dir(&repo.join("data/fonts"), &root.join("ui/fonts"), "ttf");
    tauri_build::build();
}
