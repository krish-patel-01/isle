// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! The Reminders page's list: isle-core's TodoList, saved, as the Linux daemon's Todo1.

use std::sync::Mutex;

use chrono::{Local, TimeZone};
use isle_core::todo::TodoList;
use serde_json::Value;

use crate::store;
use crate::timer::now_ms;

const FILE: &str = "todo.json";

pub struct Tasks {
    list: TodoList,
}

/// Midnight today, in epoch milliseconds.
fn day_start() -> i64 {
    let today = Local::now().date_naive();
    Local
        .from_local_datetime(&today.and_hms_opt(0, 0, 0).unwrap_or_default())
        .earliest()
        .map_or(0, |time| time.timestamp_millis())
}

impl Tasks {
    pub fn load() -> Mutex<Self> {
        let mut list: TodoList = store::load(FILE);
        // Finished tasks go at midnight.
        if list.drop_done_before(day_start()) {
            store::save(FILE, &list);
        }
        Mutex::new(Self { list })
    }

    /// The `Items` property of Todo1.
    pub fn items(&self) -> Value {
        serde_json::to_value(self.list.items()).unwrap_or(Value::Null)
    }

    /// Runs one Todo1 method; returns whether the list changed.
    pub fn call(&mut self, method: &str, arg: &Value) -> bool {
        let id = arg.get("id").and_then(Value::as_u64).unwrap_or(0);
        let title = arg.get("title").and_then(Value::as_str).unwrap_or("");
        let changed = match method {
            "Add" => self.list.add(title).is_some(),
            "SetDone" => {
                let done = arg.get("done").and_then(Value::as_bool).unwrap_or(false);
                self.list.set_done(id, done, now_ms())
            }
            "Rename" => self.list.rename(id, title),
            "Remove" => self.list.remove(id),
            "ClearDone" => {
                self.list.clear_done();
                true
            }
            "Midnight" => self.list.drop_done_before(day_start()),
            _ => false,
        };
        if changed {
            store::save(FILE, &self.list);
        }
        changed
    }
}
