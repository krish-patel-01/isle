// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! Isle's settings: the GSettings keys of the GNOME extension (gnome-extension/schemas) that mean something
//! on Windows, with the same names and defaults, kept in `settings.json`. Two are not kept there but read
//! from where they live: `start-with-windows` (the Run key in the registry) and `agent-alerts-claude`
//! (Isle's hooks in Claude Code's settings).

use serde_json::{Map, Value, json};

use crate::{agents, store};

const FILE: &str = "settings.json";

/// Every key with its default, as the schema has them.
pub fn defaults() -> Map<String, Value> {
    let Value::Object(map) = json!({
        "island-theme": "black",
        "glass-tint": 0,
        "clock-format": "system",
        "lyrics-online": true,
        "open-on": "hover",
        "hover-delay": 120,
        "auto-hide": "off",
        "auto-hide-delay": 3,
        "alerts-over-fullscreen": false,
        "island-monitor": "primary",
        "notch-depth": 30,
        "event-alerts": true,
        "event-alert-minutes": 5,
        "world-clocks": true,
        "world-clock-cities": [],
        "hidden-pages": [],
        "app-usage": false,
        "notification-alerts": true,
        "track-peek": false,
        "call-activity": true,
        "call-preview": true,
        "clipboard-history": true,
        "clipboard-ignored-apps": [],
        "usage-enabled": true,
        "usage-claude": true,
        "usage-codex": true,
        "usage-warn": 80,
        "usage-critical": 95,
        "chimes": true,
    }) else {
        unreachable!()
    };
    map
}

/// The settings as saved, over the defaults.
pub struct Settings {
    values: Map<String, Value>,
}

impl Settings {
    pub fn load() -> Self {
        let saved: Map<String, Value> = store::load(FILE);
        let mut values = defaults();
        for (key, value) in saved {
            if values.contains_key(&key) {
                values.insert(key, value);
            }
        }
        Self { values }
    }

    pub fn get(&self, key: &str) -> Value {
        self.values.get(key).cloned().unwrap_or(Value::Null)
    }

    pub fn bool(&self, key: &str) -> bool {
        self.values.get(key).and_then(Value::as_bool).unwrap_or(false)
    }

    pub fn int(&self, key: &str) -> i64 {
        self.values.get(key).and_then(Value::as_i64).unwrap_or(0)
    }

    /// Every key, the ones kept elsewhere read now.
    pub fn all(&self) -> Value {
        let mut all = self.values.clone();
        all.insert("start-with-windows".into(), json!(autostart::enabled()));
        all.insert("agent-alerts-claude".into(), json!(agents::claude_hooked()));
        // Not settings: what Settings lists (the plans signed in to, Claude Code being there) and how Windows
        // shows the time, for the clock's "As Windows".
        let (claude, codex) = crate::usage::found();
        all.insert("found-claude".into(), json!(claude));
        all.insert("found-codex".into(), json!(codex));
        all.insert("found-claude-code".into(), json!(crate::usage::claude_config_dir().is_dir()));
        all.insert("system-12h".into(), json!(autostart::system_uses_12h()));
        all.insert("system-week-start".into(), json!(autostart::system_week_start()));
        Value::Object(all)
    }

    /// Sets `key`; false when it is unknown, the wrong kind, or the same as before.
    pub fn set(&mut self, key: &str, value: Value) -> anyhow::Result<bool> {
        match key {
            "start-with-windows" => {
                autostart::set(value.as_bool().unwrap_or(false))?;
                return Ok(true);
            }
            "agent-alerts-claude" => {
                agents::set_claude_hooked(value.as_bool().unwrap_or(false))?;
                return Ok(true);
            }
            _ => {}
        }
        let Some(default) = defaults().remove(key) else {
            anyhow::bail!("no setting {key}");
        };
        let same_kind = std::mem::discriminant(&default) == std::mem::discriminant(&value)
            && (default.is_i64() == value.is_i64());
        if !same_kind {
            anyhow::bail!("{key} takes a value like {default}");
        }
        if self.values.get(key) == Some(&value) {
            return Ok(false);
        }
        self.values.insert(key.to_owned(), value);
        // The second usage warning never comes before the first (as prefs.js keeps them).
        let (warn, critical) = (self.int("usage-warn"), self.int("usage-critical"));
        if critical < warn {
            let other = if key == "usage-warn" { "usage-critical" } else { "usage-warn" };
            self.values.insert(other.into(), json!(if key == "usage-warn" { warn } else { critical }));
        }
        self.save();
        Ok(true)
    }

    /// Back to how Isle came, but for what lives elsewhere (Claude Code's hooks, starting with Windows).
    pub fn reset(&mut self) {
        self.values = defaults();
        self.save();
    }

    fn save(&self) {
        // Only what differs from the defaults, so a later default reaches those who never changed it.
        let defaults = defaults();
        let changed: Map<String, Value> = self
            .values
            .iter()
            .filter(|(key, value)| defaults.get(*key) != Some(value))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        store::save(FILE, &changed);
    }
}

/// Starting with Windows: Isle's line under the current user's Run key.
pub mod autostart {
    const NAME: &str = "Isle";

    #[cfg(windows)]
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The command Windows runs at sign-in: this executable, quoted.
    fn command() -> String {
        let exe = std::env::current_exe().unwrap_or_default();
        let exe = exe.to_string_lossy();
        format!("\"{}\"", exe.strip_prefix(r"\\?\").unwrap_or(&exe))
    }

    #[cfg(windows)]
    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    /// Whether Windows starts this Isle at sign-in.
    #[cfg(windows)]
    pub fn enabled() -> bool {
        read(RUN, NAME).is_some_and(|value| value.eq_ignore_ascii_case(&command()))
    }

    /// Whether Windows shows the time with AM and PM (Settings → Time & language → Region).
    #[cfg(windows)]
    pub fn system_uses_12h() -> bool {
        read(r"Control Panel\International", "sShortTime").is_some_and(|format| format.contains('h'))
    }

    /// The day weeks start on, 0 for Sunday to 6 for Saturday, as Windows' region settings have it.
    #[cfg(windows)]
    pub fn system_week_start() -> u32 {
        // Windows counts from Monday (0).
        read(r"Control Panel\International", "iFirstDayOfWeek")
            .and_then(|day| day.trim().parse::<u32>().ok())
            .map_or(1, |day| (day + 1) % 7)
    }

    /// A string value under the current user's key `key`.
    #[cfg(windows)]
    fn read(key: &str, name: &str) -> Option<String> {
        use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
        use windows::core::PCWSTR;
        let (key, name) = (wide(key), wide(name));
        let mut buffer = [0u16; 1024];
        let mut size = (buffer.len() * 2) as u32;
        // SAFETY: reads a string value into a buffer of the size given.
        let found = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(key.as_ptr()),
                PCWSTR(name.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if found.is_err() {
            return None;
        }
        let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..len]))
    }

    #[cfg(windows)]
    pub fn set(on: bool) -> anyhow::Result<()> {
        use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW};
        use windows::core::PCWSTR;
        let (key, name) = (wide(RUN), wide(NAME));
        // SAFETY: writes or removes one string value under the current user's Run key.
        let result = unsafe {
            if on {
                let value = wide(&command());
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    PCWSTR(key.as_ptr()),
                    PCWSTR(name.as_ptr()),
                    REG_SZ.0,
                    Some(value.as_ptr().cast()),
                    (value.len() * 2) as u32,
                )
            } else {
                RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), PCWSTR(name.as_ptr()))
            }
        };
        // Removing what isn't there is fine.
        if result.is_err() && (on || result != windows::Win32::Foundation::ERROR_FILE_NOT_FOUND) {
            anyhow::bail!("the registry said {result:?}");
        }
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn enabled() -> bool {
        false
    }

    #[cfg(not(windows))]
    pub fn system_uses_12h() -> bool {
        false
    }

    #[cfg(not(windows))]
    pub fn system_week_start() -> u32 {
        1
    }

    #[cfg(not(windows))]
    pub fn set(_on: bool) -> anyhow::Result<()> {
        anyhow::bail!("only on Windows")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_keys_of_the_right_kind_are_set() {
        let mut settings = Settings { values: defaults() };
        assert!(settings.set("nonsense", json!(1)).is_err());
        assert!(settings.set("hover-delay", json!("fast")).is_err());
        assert!(settings.set("hover-delay", json!(1.5)).is_err());
        assert!(!settings.set("hover-delay", json!(120)).unwrap());
    }
}
