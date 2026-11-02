// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! Windows' Global System Media Transport Controls: the player Windows itself shows in its volume
//! flyout (Spotify, a browser's YouTube, Media Player, …). Read into the same `NowPlaying` shape the
//! Linux daemon's Media1 sends, so the island's pages work unchanged.

use serde::Serialize;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::DataReader;

/// 100 ns ticks between 1601-01-01 (Windows' epoch) and 1970-01-01, in milliseconds.
const EPOCH_GAP_MS: i64 = 11_644_473_600_000;

/// What plays, as Media1's `NowPlaying` (see docs/dbus-api.md), with `art_key` in place of
/// `art_path`: the cover is asked for apart (`art`), once per track.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct NowPlaying {
    pub available: bool,
    pub identity: String,
    pub desktop_entry: String,
    pub status: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art_key: String,
    pub length_us: i64,
    pub position_us: i64,
    pub sampled_at_ms: i64,
    pub rate: f64,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    pub can_play_pause: bool,
    pub can_seek: bool,
    pub resting: bool,
}

impl NowPlaying {
    /// The same song in the same state: what makes a change worth sending (a new sample alone is not).
    pub fn same_as(&self, other: &Self) -> bool {
        let strip = |n: &Self| Self {
            position_us: 0,
            sampled_at_ms: 0,
            ..n.clone()
        };
        strip(self) == strip(other)
    }
}

pub struct Reader {
    manager: Manager,
}

/// A friendly name for a player's app id: `Spotify.exe` → Spotify, `MSEdge` → Microsoft Edge.
fn identity(app_id: &str) -> (String, String) {
    let base = app_id.rsplit('\\').next().unwrap_or(app_id);
    let base = base.split('!').next_back().unwrap_or(base);
    let lower = base.to_ascii_lowercase();
    let name = match () {
        _ if lower.contains("spotify") => "Spotify",
        _ if lower.contains("msedge") => "Microsoft Edge",
        _ if lower.contains("chrome") => "Google Chrome",
        _ if lower.contains("firefox") => "Firefox",
        _ if lower.contains("brave") => "Brave",
        _ if lower.contains("opera") => "Opera",
        _ if lower.contains("zunemusic") || lower.contains("media") => "Media Player",
        _ if lower.contains("vlc") => "VLC",
        _ if lower.contains("applemusic") || lower.contains("itunes") => "Apple Music",
        _ => "",
    };
    let entry = lower.trim_end_matches(".exe").to_owned();
    if name.is_empty() {
        let plain = base.trim_end_matches(".exe").trim_end_matches(".EXE");
        (plain.to_owned(), entry)
    } else {
        (name.to_owned(), entry)
    }
}

fn browser(entry: &str) -> bool {
    ["msedge", "chrome", "firefox", "brave", "opera", "vivaldi"]
        .iter()
        .any(|name| entry.contains(name))
}

impl Reader {
    pub fn new() -> windows::core::Result<Self> {
        Ok(Self {
            manager: Manager::RequestAsync()?.get()?,
        })
    }

    fn session(&self) -> Option<Session> {
        self.manager.GetCurrentSession().ok()
    }

    /// What the current player plays; `available` false when none.
    pub fn now_playing(&self) -> NowPlaying {
        self.session()
            .and_then(|session| read(&session).ok())
            .unwrap_or_default()
    }

    /// The current track's cover: its bytes, as the player gave them (PNG or JPEG).
    pub fn art(&self) -> Option<Vec<u8>> {
        match self.read_art() {
            Ok(bytes) => bytes,
            Err(err) => {
                log::debug!("no cover from the player: {err}");
                None
            }
        }
    }

    fn read_art(&self) -> windows::core::Result<Option<Vec<u8>>> {
        let Some(session) = self.session() else {
            return Ok(None);
        };
        let props = session.TryGetMediaPropertiesAsync()?.get()?;
        let thumbnail = match props.Thumbnail() {
            Ok(thumbnail) => thumbnail,
            Err(err) => {
                log::debug!("the player gives no cover: {err}");
                return Ok(None);
            }
        };
        let stream = thumbnail.OpenReadAsync()?.get()?;
        let size = u32::try_from(stream.Size()?).unwrap_or(0);
        log::debug!("cover: {size} bytes");
        if size == 0 {
            return Ok(None);
        }
        let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
        reader.LoadAsync(size)?.get()?;
        let mut bytes = vec![0; size as usize];
        reader.ReadBytes(&mut bytes)?;
        Ok(Some(bytes))
    }

    /// Media1's controls: PlayPause, Next, Previous, SetPosition (µs).
    pub fn control(&self, method: &str, position_us: i64) {
        let Some(session) = self.session() else {
            return;
        };
        let result = match method {
            "PlayPause" => session.TryTogglePlayPauseAsync().and_then(|op| op.get()),
            "Next" => session.TrySkipNextAsync().and_then(|op| op.get()),
            "Previous" => session.TrySkipPreviousAsync().and_then(|op| op.get()),
            "SetPosition" => {
                let start = session
                    .GetTimelineProperties()
                    .and_then(|t| t.StartTime())
                    .map_or(0, |t| t.Duration);
                session
                    .TryChangePlaybackPositionAsync(start + position_us * 10)
                    .and_then(|op| op.get())
            }
            _ => return,
        };
        if let Err(err) = result {
            log::info!("the player did not take {method}: {err}");
        }
    }
}

fn read(session: &Session) -> windows::core::Result<NowPlaying> {
    let app_id = session.SourceAppUserModelId()?.to_string();
    let (identity, desktop_entry) = identity(&app_id);
    let props = session.TryGetMediaPropertiesAsync()?.get()?;
    let info = session.GetPlaybackInfo()?;
    let controls = info.Controls()?;
    let timeline = session.GetTimelineProperties()?;

    let status = match info.PlaybackStatus()? {
        Status::Playing => "Playing",
        Status::Paused => "Paused",
        _ => "Stopped",
    };
    let start = timeline.StartTime()?.Duration;
    let end = timeline.EndTime()?.Duration;
    let position = timeline.Position()?.Duration;
    let updated = timeline.LastUpdatedTime()?.UniversalTime;
    let now = chrono::Utc::now().timestamp_millis();
    let sampled_at_ms = if updated > 0 {
        (updated / 10_000 - EPOCH_GAP_MS).min(now)
    } else {
        now
    };
    let rate = info
        .PlaybackRate()
        .and_then(|rate| rate.Value())
        .unwrap_or(1.0);

    let title = props.Title()?.to_string();
    let artist = props.Artist()?.to_string();
    let album = props.AlbumTitle()?.to_string();
    let length_us = ((end - start) / 10).max(0);
    let position_us = ((position - start) / 10).clamp(0, length_us.max(0));
    // A browser's media paused at its end is over: the closed island leaves it out.
    let resting = browser(&desktop_entry)
        && status != "Playing"
        && length_us > 0
        && position_us + 1_500_000 >= length_us;
    Ok(NowPlaying {
        available: !title.is_empty(),
        art_key: format!("{app_id}\u{1f}{artist}\u{1f}{title}\u{1f}{album}"),
        identity,
        desktop_entry,
        status: status.to_owned(),
        title,
        artist,
        album,
        length_us,
        position_us,
        sampled_at_ms,
        rate: if rate > 0.0 { rate } else { 1.0 },
        can_go_next: controls.IsNextEnabled()?,
        can_go_previous: controls.IsPreviousEnabled()?,
        can_play_pause: controls.IsPlayPauseToggleEnabled()?,
        can_seek: controls.IsPlaybackPositionEnabled()? && length_us > 0,
        resting,
    })
}
