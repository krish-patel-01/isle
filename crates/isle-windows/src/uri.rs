// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! `file://` URIs of Windows paths (isled's uri.rs, for drive letters and shares): `C:\a b.txt` is
//! `file:///C:/a%20b.txt`, `\\nas\photos\a.jpg` is `file://nas/photos/a.jpg`.

use std::path::{Path, PathBuf};

/// The `file://` URI of an absolute path.
pub fn from_path(path: &Path) -> String {
    let slashed = path.to_string_lossy().replace('\\', "/");
    // A share keeps its server as the URI's host.
    let (host, rest) = match slashed.strip_prefix("//") {
        Some(share) => share.split_once('/').map_or((share, ""), |(host, rest)| (host, rest)),
        None => ("", slashed.as_str()),
    };
    let mut uri = format!("file://{host}");
    if !rest.starts_with('/') {
        uri.push('/');
    }
    for byte in rest.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/:!$&'()*+,;=@".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

/// The path a `file://` URI names; `None` for any other URI.
pub fn to_path(uri: &str) -> Option<PathBuf> {
    let scheme = uri.get(..5)?;
    if !scheme.eq_ignore_ascii_case("file:") {
        return None;
    }
    let rest = uri[5..].split(['?', '#']).next().unwrap_or_default();
    let rest = rest.strip_prefix("//")?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let decoded = String::from_utf8(decode(path)).ok()?;
    if decoded.contains('\0') || decoded.is_empty() {
        return None;
    }
    let path = decoded.replace('/', "\\");
    Some(PathBuf::from(if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
        path
    } else {
        format!("\\\\{host}\\{path}")
    }))
}

/// Decodes `%XX` escapes into bytes; invalid escapes are kept as they are.
fn decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'%')
            .then(|| bytes.get(i + 1..i + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
        match escaped {
            Some(byte) => {
                decoded.push(byte);
                i += 3;
            }
            None => {
                decoded.push(bytes[i]);
                i += 1;
            }
        }
    }
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_uris_go_both_ways() {
        let path = Path::new(r"C:\Users\Ana Lee\Pictures\café #1.png");
        let uri = from_path(path);
        assert_eq!(uri, "file:///C:/Users/Ana%20Lee/Pictures/caf%C3%A9%20%231.png");
        assert_eq!(to_path(&uri).unwrap(), path);
        let share = Path::new(r"\\nas\photos\a.jpg");
        assert_eq!(from_path(share), "file://nas/photos/a.jpg");
        assert_eq!(to_path(&from_path(share)).unwrap(), share);
        assert!(to_path("https://example.com/a").is_none());
    }
}
