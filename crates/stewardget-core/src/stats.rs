use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::util;

/// Lifetime usage statistics. Stored locally only – never transmitted.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stats {
    /// Total bytes received.
    pub total_bytes: u64,
    pub completed: u64,
    pub failed: u64,
    /// Highest aggregate speed observed, bytes/second.
    pub peak_speed: f64,
    /// Seconds during which at least one download was transferring.
    pub active_secs: f64,
    /// Bytes per day, keyed by local calendar days since 1970-01-01.
    pub daily: BTreeMap<i64, u64>,
    pub first_seen: u64,
}

/// Bytes downloaded on a single day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayTotal {
    pub day: i64,
    pub bytes: u64,
}

impl DayTotal {
    /// `MM-DD` label for charts.
    pub fn label(&self) -> String {
        let (_, m, d) = util::civil_from_days(self.day);
        format!("{m:02}-{d:02}")
    }
}

impl Stats {
    const KEEP_DAYS: i64 = 365;

    pub(crate) fn record_bytes(&mut self, bytes: u64, now: u64) {
        if bytes == 0 {
            return;
        }
        if self.first_seen == 0 {
            self.first_seen = now;
        }
        self.total_bytes += bytes;
        let today = util::day_index(now);
        *self.daily.entry(today).or_default() += bytes;
        // bounded history
        while self.daily.first_key_value().is_some_and(|(d, _)| *d < today - Self::KEEP_DAYS) {
            self.daily.pop_first();
        }
    }

    /// The last `days` days (including today), oldest first, zero-filled.
    pub fn recent_days(&self, days: usize, now: u64) -> Vec<DayTotal> {
        let today = util::day_index(now);
        (0..days as i64)
            .rev()
            .map(|back| {
                let day = today - back;
                DayTotal { day, bytes: self.daily.get(&day).copied().unwrap_or(0) }
            })
            .collect()
    }

    pub fn today(&self, now: u64) -> u64 {
        self.daily.get(&util::day_index(now)).copied().unwrap_or(0)
    }

    /// Mean speed while active, bytes/second.
    pub fn average_speed(&self) -> f64 {
        if self.active_secs > 1.0 { self.total_bytes as f64 / self.active_secs } else { 0.0 }
    }

    pub fn success_rate(&self) -> Option<f64> {
        let total = self.completed + self.failed;
        (total > 0).then(|| self.completed as f64 / total as f64)
    }
}
