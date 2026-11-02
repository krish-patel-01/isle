// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! Music: what plays, read from Windows' media controls (in place of MPRIS on Linux), and its
//! lyrics from LRCLIB through the same module the Linux daemon uses.

pub mod art {
    /// FNV-1a: a stable hash for cache file names (as `isle-daemon/src/media/art.rs`).
    pub fn fnv1a(text: &str) -> u64 {
        text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        })
    }
}

#[path = "../../../isle-daemon/src/media/lyrics.rs"]
#[allow(dead_code)]
pub mod lyrics;

#[cfg(windows)]
pub mod gsmtc;
