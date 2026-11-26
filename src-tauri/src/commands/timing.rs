//! Timings that test builds show, to measure how long things take (#20).

use std::sync::OnceLock;
use std::time::Instant;

static STARTED: OnceLock<Instant> = OnceLock::new();

/// Record when the app started. Called first thing in `run`.
pub fn record_start() {
    STARTED.get_or_init(Instant::now);
}

/// Milliseconds since the app started.
#[tauri::command]
pub fn startup_elapsed_ms() -> f64 {
    STARTED
        .get()
        .map_or(0.0, |started| started.elapsed().as_secs_f64() * 1000.0)
}
