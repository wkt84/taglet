//! Timings that test builds show, to measure how long things take (#20).

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

static STARTED: OnceLock<SystemTime> = OnceLock::new();

/// Record when the app started. Called first thing in `run`.
pub fn record_start() {
    STARTED.get_or_init(SystemTime::now);
}

/// When the app started, in milliseconds since the Unix epoch, for the page to subtract
/// from when its first screen was painted (by the same clock), so that the time this
/// call takes does not count.
#[tauri::command]
pub fn startup_epoch_ms() -> f64 {
    STARTED
        .get()
        .and_then(|started| started.duration_since(UNIX_EPOCH).ok())
        .map_or(0.0, |since| since.as_secs_f64() * 1000.0)
}
