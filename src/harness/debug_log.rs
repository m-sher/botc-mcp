//! Verbose, timestamped debug log to a file, for post-mortem of harness state.
//!
//! The TUI runs full-screen (alternate screen), so stderr/stdout are unusable for
//! tracing. This writes a persistent, flushed-per-line log to a stable path
//! (default `/tmp/botc-tui-debug.log`, override with `BOTC_TUI_LOG`) that survives
//! quit (it is *not* under the per-run work root, which is deleted on exit).
//!
//! Use the [`crate::dlog!`] macro. No-ops (and cheap) until [`init`] is called.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static LOG: OnceLock<Mutex<Option<File>>> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();

/// Resolve the log path: `$BOTC_TUI_LOG` or `/tmp/botc-tui-debug.log`.
pub fn log_path() -> String {
    std::env::var("BOTC_TUI_LOG").unwrap_or_else(|_| "/tmp/botc-tui-debug.log".to_string())
}

/// Open (truncate) the log file and start the clock. Idempotent.
pub fn init(path: &str) {
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .ok();
    let _ = START.set(Instant::now());
    let _ = LOG.set(Mutex::new(file));
    log_line(&format!("=== botc-tui debug log start ({path}) ==="));
}

/// True if a log file is open.
pub fn enabled() -> bool {
    LOG.get()
        .map(|m| m.lock().unwrap().is_some())
        .unwrap_or(false)
}

/// Write one timestamped line (flushed). No-op if not initialised.
pub fn log_line(msg: &str) {
    let Some(m) = LOG.get() else {
        return;
    };
    let mut g = m.lock().unwrap();
    if let Some(f) = g.as_mut() {
        let _ = writeln!(f, "{} {msg}", timestamp());
        let _ = f.flush();
    }
}

fn timestamp() -> String {
    let elapsed = START.get().map(|s| s.elapsed().as_millis()).unwrap_or(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let (hh, mm, ss, ms) = (
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60,
        now.subsec_millis(),
    );
    format!("[{hh:02}:{mm:02}:{ss:02}.{ms:03} +{elapsed:>6}ms]")
}

/// Verbose timestamped logging to the harness debug file. `dlog!("x={}", x)`.
#[macro_export]
macro_rules! dlog {
    ($($arg:tt)*) => {
        $crate::harness::debug_log::log_line(&format!($($arg)*))
    };
}
