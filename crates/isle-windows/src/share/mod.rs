// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! Sharing the Shelf's files with a phone (isled's share/mod.rs, Share1): a QR code on the island opens a page in
//! the phone's browser that lists the shelf's files and sends files to it. The web server, its sessions and the
//! page are isled's own; only this glue and the network's address are Windows'. Nothing listens until Start.

#[path = "../../../isle-daemon/src/share/files.rs"]
mod files;
#[path = "../../../isle-daemon/src/share/http.rs"]
mod http;
mod net;
#[path = "../../../isle-daemon/src/share/server.rs"]
mod server;
// Ending when GNOME Shell leaves the bus, and the firewall hint, are Linux's.
#[allow(dead_code)]
#[path = "../../../isle-daemon/src/share/session.rs"]
mod session;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use isle_core::shelf::ShelfItem;
use qrcode::{Color, EcLevel, QrCode};
use serde_json::{Value, json};
use tokio::sync::watch;

use self::net::Trust;
use self::server::Host;
use self::session::{Config, Ended, Limits, Session, Status};
use crate::shelf::{ShelfStore, owned_dir, previews_dir};

/// The ports sharing listens on, one at random.
const PORTS: (u16, u16) = (47830, 47839);
/// How often the lock, sleep and network are looked at while sharing.
const GUARD_EVERY: Duration = Duration::from_secs(2);

/// What the island hears: the state changed, a file came in, the shelf changed.
pub type Tell = Arc<dyn Fn(&str, Value) + Send + Sync>;

#[derive(Default)]
struct State {
    session: Option<Arc<Session>>,
    trust: Option<Trust>,
    qr: Vec<String>,
    ended: Option<Ended>,
    count: u64,
}

pub struct Share {
    state: Mutex<State>,
    shelf: Arc<ShelfStore>,
    tell: Tell,
}

impl Share {
    pub fn new(shelf: Arc<ShelfStore>, tell: Tell) -> Arc<Self> {
        Arc::new(Self { state: Mutex::default(), shelf, tell })
    }

    /// Starts sharing (or keeps the session already running); the words for the island when it can't.
    pub async fn start(self: &Arc<Self>) -> Result<(), String> {
        if let Some(session) = self.state.lock().unwrap().session.as_ref().filter(|session| session.ended().is_none()) {
            session.touch();
            return Ok(());
        }
        let lan = net::lan().ok_or("Not on a network · connect to Wi-Fi first")?;
        let trust = net::trust(lan.ip);
        let host = WinHost { share: self.clone() };
        let config = Config {
            lan,
            own: net::own_addresses(),
            ports: PORTS,
            dir: owned_dir(),
            previews: previews_dir(),
            limits: Limits::default(),
        };
        let session = server::start(config, host).await.map_err(|err| {
            if err.kind() == std::io::ErrorKind::AddrInUse {
                format!("Ports {}–{} are all in use · try again later", PORTS.0, PORTS.1)
            } else {
                format!("Couldn't start sharing: {err}")
            }
        })?;
        {
            let mut state = self.state.lock().unwrap();
            state.count += 1;
            state.qr = qr_rows(&session.url);
            state.session = Some(session.clone());
            state.trust = Some(trust);
            state.ended = None;
        }
        tokio::spawn(guard(self.clone(), session));
        self.changed();
        Ok(())
    }

    /// Stops sharing: the server closes, transfers under way end.
    pub fn stop(&self) {
        let session = self.state.lock().unwrap().session.take();
        if let Some(session) = session {
            session.end(Ended::Stopped);
            self.forget(Ended::Stopped);
            self.changed();
        }
    }

    /// Allows or turns down the phone `id` asking to connect.
    pub fn answer(&self, id: &str, allow: bool) -> bool {
        let answered = self.session().is_some_and(|session| session.answer(id, allow));
        if answered {
            self.changed();
        }
        answered
    }

    /// Takes the allowed phone `id` off.
    pub fn remove(&self, id: &str) -> bool {
        let removed = self.session().is_some_and(|session| session.remove(id));
        if removed {
            self.changed();
        }
        removed
    }

    /// The session's address with its token, and its QR code: `{url, qr}`; kept out of `state`.
    pub fn code(&self) -> Option<Value> {
        let state = self.state.lock().unwrap();
        let session = state.session.as_ref()?;
        Some(json!({ "url": session.url, "qr": state.qr }))
    }

    /// Share1's `State`: no secret in it.
    pub fn state(&self) -> Value {
        let state = self.state.lock().unwrap();
        let Some(session) = &state.session else {
            return json!({ "active": false, "ended": state.ended });
        };
        json!({
            "active": true,
            "session": state.count,
            "network": state.trust,
            "firewall": Value::Null,
            "asking": session.phones(Status::Asking),
            "phones": session.phones(Status::Allowed),
            "idle_minutes": session.config.limits.idle.as_secs() / 60,
        })
    }

    fn session(&self) -> Option<Arc<Session>> {
        self.state.lock().unwrap().session.clone()
    }

    fn forget(&self, why: Ended) {
        let mut state = self.state.lock().unwrap();
        state.session = None;
        state.qr.clear();
        state.trust = None;
        state.ended = Some(why);
    }

    fn changed(&self) {
        (self.tell)("share", self.state());
    }
}

/// Ends the session when the screen locks, the computer sleeps or the network goes; once it ends, however it
/// ends, the island hears of it.
async fn guard(share: Arc<Share>, session: Arc<Session>) {
    let mut version = session.version();
    let (mut wall, mut tick) = (SystemTime::now(), Instant::now());
    let why = loop {
        if let Some(why) = session.ended() {
            break why;
        }
        tokio::select! {
            () = tokio::time::sleep(GUARD_EVERY) => {
                // Asleep: the wall clock moved on far more than this program's own.
                let slept = wall.elapsed().unwrap_or_default().saturating_sub(tick.elapsed()) > Duration::from_secs(20);
                (wall, tick) = (SystemTime::now(), Instant::now());
                if slept {
                    session.end(Ended::Asleep);
                } else if imp::locked() {
                    session.end(Ended::Locked);
                } else if net::lan().is_none_or(|lan| lan.ip != session.config.lan.ip) {
                    session.end(Ended::Network);
                }
            }
            changed = version.changed() => {
                if changed.is_err() {
                    break Ended::Stopped;
                }
            }
        }
    };
    let current = share.session().is_some_and(|running| Arc::ptr_eq(&running, &session));
    if current {
        share.forget(why);
        share.changed();
    }
}

/// The QR code of `url`, a row of 0s and 1s (1 dark) per line, without its quiet zone.
fn qr_rows(url: &str) -> Vec<String> {
    let Ok(code) = QrCode::with_error_correction_level(url.as_bytes(), EcLevel::L) else {
        return Vec::new();
    };
    code.to_colors()
        .chunks(code.width())
        .map(|row| row.iter().map(|color| if *color == Color::Dark { '1' } else { '0' }).collect())
        .collect()
}

/// The server's view of Isle: the shelf, and the island to tell.
#[derive(Clone)]
struct WinHost {
    share: Arc<Share>,
}

impl Host for WinHost {
    async fn items(&self) -> Vec<ShelfItem> {
        self.share.shelf.list()
    }

    async fn shelve(&self, path: PathBuf) -> bool {
        let added = self.share.shelf.add_received(&path);
        if added {
            (self.share.tell)("shelf", self.share.shelf.items());
        }
        added
    }

    async fn shelf_changes(&self) -> watch::Receiver<u64> {
        self.share.shelf.changes()
    }

    fn changed(&self) {
        self.share.changed();
    }

    fn received(&self, name: String) {
        (self.share.tell)("shareReceived", Value::String(name));
    }
}

#[cfg(windows)]
mod imp {
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_SWITCHDESKTOP, OpenInputDesktop, SwitchDesktop,
    };

    /// Whether the screen is locked: the input desktop is then Winlogon's, which can't be switched to.
    pub fn locked() -> bool {
        // SAFETY: opens the input desktop for the least access that tells, and closes it.
        unsafe {
            match OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_SWITCHDESKTOP) {
                Ok(desktop) => {
                    let locked = SwitchDesktop(desktop).is_err();
                    let _ = CloseDesktop(desktop);
                    locked
                }
                Err(_) => true,
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn locked() -> bool {
        false
    }
}
