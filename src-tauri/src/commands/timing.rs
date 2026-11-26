//! Timings that test builds show, to measure how long things take (#20, #36).

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// A moment of starting the app, in milliseconds since the Unix epoch (the page takes
/// its own moments by the same clock, so the calls that fetch these do not count).
#[derive(Debug, Clone, Serialize)]
pub struct StartupMark {
    pub name: &'static str,
    pub at_ms: f64,
}

static MARKS: Mutex<Vec<StartupMark>> = Mutex::new(Vec::new());

fn epoch_ms(time: SystemTime) -> f64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64() * 1000.0)
}

/// Record that starting the app reached `name` now. The first of each name counts:
/// `run` first thing in `run`, then `built` (plugins and state), `window` (the window
/// and its webview created), `ready` (the event loop running) and `page_load` (the
/// page starting to load).
pub fn mark(name: &'static str) {
    let Ok(mut marks) = MARKS.lock() else {
        return;
    };
    if marks.iter().all(|mark| mark.name != name) {
        marks.push(StartupMark {
            name,
            at_ms: epoch_ms(SystemTime::now()),
        });
    }
}

/// When the operating system created this process (before `run`: loading the program
/// and its libraries), where known.
fn process_created_ms() -> Option<f64> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        let (mut created, mut exited, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        // SAFETY: the current process's pseudo handle needs no closing, and the four
        // pointers are to local FILETIMEs.
        let ok = unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        };
        if ok == 0 {
            return None;
        }
        // 100-nanosecond intervals since 1601-01-01.
        let ticks = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
        const EPOCH_DIFFERENCE_MS: f64 = 11_644_473_600_000.0;
        Some(ticks as f64 / 10_000.0 - EPOCH_DIFFERENCE_MS)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// The moments recorded so far, in order, with when the process was created first
/// (`process`) where known.
#[tauri::command]
pub fn startup_marks() -> Vec<StartupMark> {
    let mut marks = MARKS.lock().map(|marks| marks.clone()).unwrap_or_default();
    if let Some(at_ms) = process_created_ms() {
        marks.insert(
            0,
            StartupMark {
                name: "process",
                at_ms,
            },
        );
    }
    marks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_keep_the_first_of_each_name_in_order() {
        mark("run");
        mark("built");
        mark("run");
        let marks = startup_marks();
        let names = marks
            .iter()
            .map(|mark| mark.name)
            .filter(|name| *name != "process")
            .collect::<Vec<_>>();
        assert_eq!(names, ["run", "built"]);
        assert!(marks.windows(2).all(|pair| pair[0].at_ms <= pair[1].at_ms));
        // Within the last day, by the same clock as the page's.
        let now = epoch_ms(SystemTime::now());
        assert!(marks
            .iter()
            .all(|mark| mark.at_ms <= now && mark.at_ms > now - 86_400_000.0));
    }
}
