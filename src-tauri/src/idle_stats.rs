//! One native collector, independent of timer actions and webview visibility.
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, WebviewWindow};
use tauri_plugin_store::StoreExt;

static MUTATION: Mutex<()> = Mutex::new(());
const DAY: u64 = 86_400;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdlePeriod {
    started_at: u64,
    ended_at: u64,
}

#[derive(Default, Clone)]
struct Collector {
    observing_since: Option<u64>,
    active_start: Option<u64>,
    last_input: Option<u64>,
}

impl Collector {
    fn sample(
        &mut self,
        periods: &mut Vec<IdlePeriod>,
        now: u64,
        seconds: Option<u64>,
        threshold: u64,
    ) {
        let Some(seconds) = seconds else {
            // Disabled or unavailable: never bridge an unobserved gap.
            *self = Self::default();
            return;
        };
        let since = *self.observing_since.get_or_insert(now);
        let input = now.saturating_sub(seconds);
        // Also recognize input followed by another idle interval between polls.
        let returned = self
            .last_input
            .is_some_and(|previous| input > previous.saturating_add(2));
        if returned && self.active_start.take().is_some() {
            if let Some(period) = periods.last_mut() {
                period.ended_at = input.max(period.ended_at).min(now);
            }
        }
        // A threshold change must not split an already recorded idle period.
        if seconds >= threshold || self.active_start.is_some() {
            if self.active_start.is_none() || periods.is_empty() {
                let start = input.max(since);
                self.active_start = Some(start);
                periods.push(IdlePeriod {
                    started_at: start,
                    ended_at: now,
                });
            } else if let Some(period) = periods.last_mut() {
                period.ended_at = now.max(period.started_at);
            }
        }
        self.last_input = Some(input);
    }
}

fn prune(periods: &mut Vec<IdlePeriod>, now: u64, days: u64) {
    let cutoff = now.saturating_sub(days.clamp(1, 365) * DAY);
    periods.retain(|period| period.ended_at > cutoff);
    for period in periods {
        period.started_at = period.started_at.max(cutoff);
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn preferences(app: &AppHandle) -> Result<(bool, u64, u64), String> {
    let store = app.store("settings.json").map_err(|e| e.to_string())?;
    let settings = store.get("settings").unwrap_or_default();
    Ok((
        settings["enableIdleDetection"].as_bool().unwrap_or(false),
        settings["idleThresholdMinutes"]
            .as_u64()
            .unwrap_or(5)
            .clamp(1, 60)
            * 60,
        settings["idleStatsRetentionDays"]
            .as_u64()
            .unwrap_or(30)
            .clamp(1, 365),
    ))
}

fn access(
    app: &AppHandle,
    sample: Option<(&mut Collector, Option<u64>, u64)>,
) -> Result<Vec<IdlePeriod>, String> {
    let _lock = MUTATION.lock().map_err(|e| e.to_string())?;
    let (_, _, days) = preferences(app)?;
    let store = app.store("idle-stats.json").map_err(|e| e.to_string())?;
    let previous = store.get("periods");
    let mut periods: Vec<IdlePeriod> = previous
        .clone()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    let timestamp = now();
    if let Some((collector, seconds, threshold)) = sample {
        collector.sample(&mut periods, timestamp, seconds, threshold);
    }
    prune(&mut periods, timestamp, days);
    let value = serde_json::to_value(&periods).map_err(|e| e.to_string())?;
    if previous.as_ref() != Some(&value) {
        crate::store_persistence::persist_store_value(&store, "periods", Some(value))?;
    }
    Ok(periods)
}

#[tauri::command]
pub fn get_idle_stats(app: AppHandle, window: WebviewWindow) -> Result<Vec<IdlePeriod>, String> {
    if window.label() != "settings" {
        return Err("Idle statistics are only available in settings".into());
    }
    access(&app, None)
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        let mut collector = Collector::default();
        loop {
            let result = preferences(&app).and_then(|(enabled, threshold, _)| {
                let seconds = if enabled {
                    crate::idle::get_idle_seconds().ok()
                } else {
                    None
                };
                // Commit collector state only after the corresponding data is saved.
                let mut next = collector.clone();
                access(&app, Some((&mut next, seconds, threshold)))?;
                collector = next;
                Ok(())
            });
            if let Err(error) = result {
                log::error!("Failed to persist idle statistics: {error}");
            }
            std::thread::sleep(Duration::from_secs(10));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_once_updates_and_closes_at_last_input() {
        let mut c = Collector::default();
        let mut periods = vec![];
        c.sample(&mut periods, 1000, Some(0), 60);
        c.sample(&mut periods, 1060, Some(60), 60);
        c.sample(&mut periods, 1070, Some(70), 60);
        c.sample(&mut periods, 1080, Some(4), 60);
        assert_eq!(
            periods,
            vec![IdlePeriod {
                started_at: 1000,
                ended_at: 1076
            }]
        );
        c.sample(&mut periods, 1140, Some(64), 60);
        assert_eq!(periods.len(), 2);
    }

    #[test]
    fn disabled_and_restart_never_bridge_unobserved_time() {
        let mut c = Collector::default();
        let mut periods = vec![];
        c.sample(&mut periods, 1000, Some(100), 60);
        c.sample(&mut periods, 1010, Some(110), 60);
        c.sample(&mut periods, 1020, None, 60);
        c.sample(&mut periods, 2000, Some(1100), 60);
        assert_eq!(
            periods[0],
            IdlePeriod {
                started_at: 1000,
                ended_at: 1010
            }
        );
        assert_eq!(periods[1].started_at, 2000);
    }

    #[test]
    fn continues_and_closes_a_period_clipped_by_retention() {
        let mut c = Collector::default();
        let mut periods = vec![];
        c.sample(&mut periods, DAY, Some(0), 60);
        c.sample(&mut periods, DAY * 3, Some(DAY * 2), 60);
        prune(&mut periods, DAY * 3, 1);
        c.sample(&mut periods, DAY * 3 + 10, Some(2), 60);
        assert_eq!(periods.len(), 1);
        assert_eq!(periods[0].started_at, DAY * 2);
        assert_eq!(periods[0].ended_at, DAY * 3 + 8);
    }

    #[test]
    fn raising_threshold_does_not_duplicate_an_ongoing_period() {
        let mut c = Collector::default();
        let mut periods = vec![];
        c.sample(&mut periods, 1000, Some(0), 60);
        c.sample(&mut periods, 1060, Some(60), 60);
        c.sample(&mut periods, 1070, Some(70), 120);
        c.sample(&mut periods, 1120, Some(120), 120);
        assert_eq!(
            periods,
            vec![IdlePeriod {
                started_at: 1000,
                ended_at: 1120
            }]
        );
    }

    #[test]
    fn detects_activity_between_idle_samples() {
        let mut c = Collector::default();
        let mut periods = vec![];
        c.sample(&mut periods, 1000, Some(0), 60);
        c.sample(&mut periods, 1060, Some(60), 60);
        c.sample(&mut periods, 1200, Some(80), 60);
        assert_eq!(periods.len(), 2);
        assert_eq!(periods[0].ended_at, 1120);
        assert_eq!(periods[1].started_at, 1120);
    }

    #[test]
    fn retention_removes_old_and_clips_overlapping_periods() {
        let mut periods = vec![
            IdlePeriod {
                started_at: 1,
                ended_at: DAY,
            },
            IdlePeriod {
                started_at: DAY,
                ended_at: DAY * 3,
            },
        ];
        prune(&mut periods, DAY * 4, 2);
        assert_eq!(
            periods,
            vec![IdlePeriod {
                started_at: DAY * 2,
                ended_at: DAY * 3
            }]
        );
    }
}
