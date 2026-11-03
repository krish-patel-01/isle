// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! The Calendar page's two sources, as the daemon's Calendar1 and GNOME Shell's event source on Linux:
//! Isle's own events (isle-core's calendar, typed in one line, kept in `calendar.json`), and the events of
//! the calendars Windows keeps (its appointment store, which the Calendar app and synced accounts fill),
//! read for the span the page looks at plus today and tomorrow, for the alerts before an event.

use std::sync::{Condvar, Mutex};
use std::time::Duration;

use chrono::Local;
use isle_core::calendar::{AddError, Calendar, format_time, parse};
use serde::Serialize;
use serde_json::{Value, json};

use crate::store;

const FILE: &str = "calendar.json";
/// Windows' calendars are read again this often, as nothing tells Isle they changed.
const READ_EVERY: Duration = Duration::from_secs(60);
const DAY_MS: i64 = 24 * 3600 * 1000;

/// Isle's own events.
pub struct Own {
    calendar: Calendar,
}

fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

impl Own {
    pub fn load() -> Mutex<Self> {
        Mutex::new(Self { calendar: store::load(FILE) })
    }

    /// The `Events` property: every event, by day and start time.
    pub fn events(&self) -> Value {
        serde_json::to_value(self.calendar.events()).unwrap_or(Value::Null)
    }

    /// Calendar1's `Add` ({date, text}) and `Remove` ({id}); true when the events changed.
    pub fn call(&mut self, method: &str, arg: &Value) -> Result<bool, String> {
        match method {
            "Add" => {
                let (date, text) = (arg["date"].as_str().unwrap_or_default(), arg["text"].as_str().unwrap_or_default());
                self.calendar.add(date, text).map_err(|err| match err {
                    AddError::BadDate => format!("`{date}` is not a date (YYYY-MM-DD)"),
                    AddError::NoTitle => "an event needs a title".into(),
                })?;
                self.calendar.forget_old(&today());
            }
            "Remove" => {
                if !self.calendar.remove(arg["id"].as_u64().unwrap_or_default()) {
                    return Ok(false);
                }
            }
            _ => return Ok(false),
        }
        store::save(FILE, &self.calendar);
        Ok(true)
    }
}

/// How `Add` reads a line of text, without adding it: `{title, start, end}`, times as "HH:MM" or null (all
/// day; no end); null when no title is left.
pub fn parse_line(text: &str) -> Value {
    parse(text).map_or(Value::Null, |parsed| {
        json!({
            "title": parsed.title,
            "start": parsed.start.map(format_time),
            "end": parsed.end.map(format_time),
        })
    })
}

/// One occurrence of an event in Windows' calendars, as the page draws it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SystemEvent {
    /// The calendar, the event and its start, a line each: one occurrence, as GNOME's ids.
    pub id: String,
    pub title: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub all_day: bool,
    /// The calendar's colour, "#rrggbb".
    pub color: String,
    /// A Meet, Zoom, Teams or Webex link to join it by, or "".
    pub link: String,
}

/// What Windows' calendars hold for the span asked for, sent as `systemEvents`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SystemEvents {
    /// Whether there is any calendar at all.
    pub has_calendars: bool,
    pub begin_ms: i64,
    pub end_ms: i64,
    pub events: Vec<SystemEvent>,
}

/// The span the page looks at, and a wake-up for the reader when it changes.
#[derive(Default)]
pub struct Wanted {
    range: Mutex<Option<(i64, i64)>>,
    wake: Condvar,
}

impl Wanted {
    /// The page shows [begin, end) now (`Range`).
    pub fn ask(&self, begin_ms: i64, end_ms: i64) {
        *self.range.lock().unwrap() = Some((begin_ms, end_ms.max(begin_ms)));
        self.wake.notify_one();
    }

    fn wait(&self) {
        let range = self.range.lock().unwrap();
        let before = *range;
        let _ = self.wake.wait_timeout_while(range, READ_EVERY, |now| *now == before).unwrap();
    }
}

/// Reads Windows' calendars for as long as Isle runs, calling `publish` with each change.
pub fn watch(wanted: &Wanted, publish: impl Fn(&SystemEvents)) {
    #[cfg(windows)]
    {
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
        // SAFETY: once, at the start of this thread.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    }
    let mut last = SystemEvents::default();
    loop {
        // Today and tomorrow always (the alerts), with what the page looks at.
        let midnight = Local::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .and_then(|t| t.and_local_timezone(Local).earliest())
            .map_or(0, |t| t.timestamp_millis());
        let (mut begin, mut end) = (midnight, midnight + 2 * DAY_MS);
        if let Some((b, e)) = *wanted.range.lock().unwrap() {
            // A span no page asks for (a year) is cut, so a read stays quick.
            begin = begin.min(b).max(midnight - 120 * DAY_MS);
            end = end.max(e).min(midnight + 120 * DAY_MS);
        }
        match imp::read(begin, end) {
            Ok(read) => {
                if read != last {
                    publish(&read);
                    last = read;
                }
            }
            Err(err) => log::debug!("could not read Windows' calendars: {err}"),
        }
        wanted.wait();
    }
}

/// The alerts for events: each event with a time, of Isle's own or Windows', once Settings' minutes before it
/// starts, and once as it starts (within a minute of it: a computer asleep then has missed it). Kept here rather than in the page, whose timers Windows may slow while it is out of sight.
#[derive(Default)]
pub struct Alarms {
    /// The alerts already given, by id, start, and whether it was the one as it starts.
    alarmed: std::collections::HashSet<(String, i64, bool)>,
}

impl Alarms {
    /// The events due an alert now, as `{id, title, start_ms, end_ms, link}`; `own` is the `events` state,
    /// `system` the `systemEvents` one.
    pub fn due(&mut self, own: &Value, system: &Value, lead_ms: i64, now_ms: i64) -> Vec<Value> {
        let mut timed: Vec<Value> = Vec::new();
        for event in own.as_array().into_iter().flatten() {
            let (Some(date), Some(start)) = (event["date"].as_str(), event["start"].as_str()) else {
                continue;
            };
            let at = |time: &str| {
                chrono::NaiveDateTime::parse_from_str(&format!("{date} {time}"), "%Y-%m-%d %H:%M")
                    .ok()
                    .and_then(|t| t.and_local_timezone(Local).earliest())
                    .map(|t| t.timestamp_millis())
            };
            let Some(start_ms) = at(start) else {
                continue;
            };
            timed.push(json!({
                "id": format!("isle:{}", event["id"]),
                "title": event["title"],
                "start_ms": start_ms,
                "end_ms": event["end"].as_str().and_then(at),
                "link": "",
            }));
        }
        for event in system["events"].as_array().into_iter().flatten() {
            if !event["all_day"].as_bool().unwrap_or(false) {
                timed.push(json!({
                    "id": event["id"],
                    "title": event["title"],
                    "start_ms": event["start_ms"],
                    "end_ms": event["end_ms"],
                    "link": event["link"],
                }));
            }
        }
        // Forgotten an hour after they began.
        self.alarmed.retain(|(_, start, _)| *start > now_ms - 3_600_000);
        let mut due = Vec::new();
        for event in timed {
            let start = event["start_ms"].as_i64().unwrap_or_default();
            let id = event["id"].as_str().unwrap_or_default().to_owned();
            let soon = start > now_ms && start - lead_ms <= now_ms;
            let now = start <= now_ms && now_ms - start < 60_000;
            if (soon || now) && self.alarmed.insert((id, start, now)) {
                due.push(event);
            }
        }
        due
    }
}

/// Each `https://` address in a text (plain, or in HTML a calendar put in a description), as isled's
/// meeting.rs finds them.
fn links(text: &str) -> impl Iterator<Item = String> + '_ {
    text.match_indices("https://").map(|(at, _)| {
        let rest = &text[at..];
        let end = rest
            .find(|c: char| c.is_whitespace() || c.is_control() || "<>\"'`|\\".contains(c))
            .unwrap_or(rest.len());
        rest[..end]
            .trim_end_matches(['.', ',', ';', ':', ')', ']', '}', '!', '?'])
            .replace("&amp;", "&")
    })
}

/// Whether a link joins a meeting on a service Isle knows (meeting.rs `is_meeting`).
pub fn is_meeting(link: &str) -> bool {
    let Some(rest) = link.strip_prefix("https://") else {
        return false;
    };
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (host, path) = rest.split_at(host_end);
    // No `user@host`, which could pass one host off as another.
    if host.contains('@') {
        return false;
    }
    let host = host.split(':').next().unwrap_or_default().to_ascii_lowercase();
    let on = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    let has_path = |prefixes: &[&str]| prefixes.iter().any(|prefix| path.starts_with(prefix));
    match host.as_str() {
        "meet.google.com" => path.len() > 1,
        "teams.microsoft.com" => has_path(&["/l/meetup-join/", "/meet/"]),
        "teams.live.com" => path.len() > 1,
        _ if on("zoom.us") => has_path(&["/j/", "/my/", "/w/"]),
        _ if on("webex.com") => host != "webex.com" && path.len() > 1,
        _ => false,
    }
}

/// The meeting link of an event: its own online-meeting link, else the first in its place or notes.
fn meeting_link(own: &str, location: &str, details: &str) -> String {
    [own, location, details]
        .into_iter()
        .flat_map(links)
        .find(|link| is_meeting(link))
        .unwrap_or_default()
}

#[cfg(windows)]
mod imp {
    use std::collections::HashMap;

    use windows::ApplicationModel::Appointments::{
        AppointmentManager, AppointmentProperties, AppointmentStoreAccessType, FindAppointmentsOptions,
    };
    use windows::Foundation::{DateTime, TimeSpan};

    use super::{SystemEvent, SystemEvents, meeting_link};

    /// 100 ns ticks between 1601 (Windows' epoch) and 1970.
    const EPOCH_TICKS: i64 = 116_444_736_000_000_000;

    fn to_ms(time: DateTime) -> i64 {
        (time.UniversalTime - EPOCH_TICKS) / 10_000
    }

    pub fn read(begin_ms: i64, end_ms: i64) -> windows::core::Result<SystemEvents> {
        let store = AppointmentManager::RequestStoreAsync(AppointmentStoreAccessType::AllCalendarsReadOnly)?.get()?;
        let mut colors = HashMap::new();
        for calendar in store.FindAppointmentCalendarsAsync()?.get()? {
            if calendar.IsHidden().unwrap_or(false) {
                continue;
            }
            let c = calendar.DisplayColor()?;
            colors.insert(calendar.LocalId()?.to_string(), format!("#{:02x}{:02x}{:02x}", c.R, c.G, c.B));
        }
        let options = FindAppointmentsOptions::new()?;
        let fetch = options.FetchProperties()?;
        for property in [
            AppointmentProperties::Subject()?,
            AppointmentProperties::StartTime()?,
            AppointmentProperties::Duration()?,
            AppointmentProperties::AllDay()?,
            AppointmentProperties::Location()?,
            AppointmentProperties::Details()?,
            AppointmentProperties::OnlineMeetingLink()?,
        ] {
            fetch.Append(&property)?;
        }
        let start = DateTime { UniversalTime: begin_ms * 10_000 + EPOCH_TICKS };
        let length = TimeSpan { Duration: (end_ms - begin_ms) * 10_000 };
        let mut events = Vec::new();
        for appointment in store.FindAppointmentsAsyncWithOptions(start, length, &options)?.get()? {
            let calendar = appointment.CalendarId()?.to_string();
            // In a hidden calendar (turned off in the Calendar app): left out, as there.
            let Some(color) = colors.get(&calendar) else {
                continue;
            };
            let start_ms = to_ms(appointment.StartTime()?);
            let end_ms = start_ms + appointment.Duration()?.Duration / 10_000;
            let text = |value: windows::core::Result<windows::core::HSTRING>| value.map(|v| v.to_string()).unwrap_or_default();
            let title = text(appointment.Subject());
            events.push(SystemEvent {
                id: format!("{calendar}\n{}\n{start_ms}", text(appointment.LocalId())),
                title: if title.trim().is_empty() { "Untitled".into() } else { title },
                start_ms,
                end_ms: end_ms.max(start_ms),
                all_day: appointment.AllDay().unwrap_or(false),
                color: color.clone(),
                link: meeting_link(
                    &text(appointment.OnlineMeetingLink()),
                    &text(appointment.Location()),
                    &text(appointment.Details()),
                ),
            });
        }
        events.sort_by_key(|event| (event.start_ms, event.id.clone()));
        Ok(SystemEvents { has_calendars: !colors.is_empty(), begin_ms, end_ms, events })
    }
}

#[cfg(not(windows))]
mod imp {
    use super::SystemEvents;

    pub fn read(begin_ms: i64, end_ms: i64) -> Result<SystemEvents, String> {
        Ok(SystemEvents { has_calendars: false, begin_ms, end_ms, events: Vec::new() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_meeting_link_is_found_where_the_calendar_put_it() {
        assert_eq!(meeting_link("", "Room 4", "Join: https://meet.google.com/abc-defg-hij."),
            "https://meet.google.com/abc-defg-hij");
        assert_eq!(meeting_link("https://teams.microsoft.com/l/meetup-join/x", "", ""),
            "https://teams.microsoft.com/l/meetup-join/x");
        assert_eq!(meeting_link("", "https://docs.google.com/x", "https://zoom.us/"), "");
    }

    #[test]
    fn an_event_is_alerted_before_and_as_it_starts() {
        let mut alarms = Alarms::default();
        let system = json!({"events": [{"id": "a", "title": "Standup", "start_ms": 1_000_000, "end_ms": 2_000_000,
            "all_day": false, "link": ""}]});
        assert!(alarms.due(&json!([]), &system, 300_000, 600_000).is_empty());
        assert_eq!(alarms.due(&json!([]), &system, 300_000, 710_000).len(), 1);
        assert!(alarms.due(&json!([]), &system, 300_000, 720_000).is_empty());
        // Once more as it starts, and no more.
        assert_eq!(alarms.due(&json!([]), &system, 300_000, 1_000_001).len(), 1);
        assert!(alarms.due(&json!([]), &system, 300_000, 1_030_000).is_empty());
        // Missed by more than a minute (asleep): not said late.
        let mut late = Alarms::default();
        assert!(late.due(&json!([]), &system, 300_000, 1_070_000).is_empty());
    }

    #[test]
    fn a_line_reads_as_the_daemon_reads_it() {
        assert_eq!(parse_line("Dinner 20:00")["start"], "20:00");
        assert_eq!(parse_line("Holiday")["start"], Value::Null);
        assert_eq!(parse_line("20:00"), Value::Null);
    }
}
