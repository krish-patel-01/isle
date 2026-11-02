// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! The focus timer: isle-core's Pomodoro, saved across restarts, as the Linux daemon's Pomodoro1.

use std::sync::Mutex;

use chrono::Local;
use isle_core::pomodoro::{FocusStats, Phase, Pomodoro, PomodoroConfig, TimerState};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::store;

const FILE: &str = "pomodoro.json";

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    config: PomodoroConfig,
    state: TimerState,
    stats: FocusStats,
}

pub struct Timer {
    pomodoro: Pomodoro,
    stats: FocusStats,
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

impl Timer {
    pub fn load() -> Mutex<Self> {
        let saved: Saved = store::load(FILE);
        Mutex::new(Self {
            pomodoro: Pomodoro::restore(saved.config, saved.state),
            stats: saved.stats,
        })
    }

    fn save(&self) {
        store::save(
            FILE,
            &Saved {
                config: self.pomodoro.config().clone(),
                state: self.pomodoro.state(),
                stats: self.stats.clone(),
            },
        );
    }

    /// The `State` property of Pomodoro1.
    pub fn state(&self) -> Value {
        let view = self.pomodoro.view(now_ms());
        let (today_sessions, today_focus_ms) = self.stats.on(&today());
        let mut value = serde_json::to_value(view).unwrap_or(Value::Null);
        if let Value::Object(map) = &mut value {
            map.insert("today_sessions".into(), json!(today_sessions));
            map.insert("today_focus_ms".into(), json!(today_focus_ms));
        }
        value
    }

    pub fn config(&self) -> Value {
        serde_json::to_value(self.pomodoro.config()).unwrap_or(Value::Null)
    }

    /// Runs one Pomodoro1 method; returns whether the state changed.
    pub fn call(&mut self, method: &str, arg: &Value) -> bool {
        let now = now_ms();
        match method {
            "Start" => self.pomodoro.start(now),
            "StartPhase" => {
                let phase: Phase = serde_json::from_value(arg.clone()).unwrap_or_default();
                if phase == Phase::Idle {
                    self.pomodoro.stop();
                } else {
                    self.pomodoro.start_phase(phase, now);
                }
            }
            "Pause" => self.pomodoro.pause(now),
            "Stop" => self.pomodoro.stop(),
            "Skip" => {
                self.pomodoro.skip(now);
            }
            "SetConfig" => {
                let Ok(config) = serde_json::from_value::<PomodoroConfig>(arg.clone()) else {
                    return false;
                };
                self.pomodoro.set_config(config);
            }
            _ => return false,
        }
        self.save();
        true
    }

    /// Moves on a phase that ran out; returns `(finished, next)` when one did.
    pub fn tick(&mut self) -> Option<(Phase, Phase)> {
        let duration = self.pomodoro.state().duration_ms;
        let transition = self.pomodoro.tick(now_ms())?;
        if transition.finished == Phase::Focus {
            self.stats.record(&today(), duration);
        }
        self.save();
        Some((transition.finished, transition.next))
    }
}
