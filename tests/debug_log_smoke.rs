//! Verify the harness debug log actually writes timestamped lines to its file.
//! Runs in its own test binary so the global logger doesn't affect other tests.

use botc_mcp::dlog;
use botc_mcp::harness::debug_log;

#[test]
fn debug_log_writes_timestamped_lines() {
    let path = std::env::temp_dir().join(format!("botc-dbg-{}.log", uuid::Uuid::new_v4()));
    let p = path.to_string_lossy().to_string();

    // Before init: no file, calls are no-ops (must not panic).
    debug_log::log_line("ignored-before-init");
    assert!(!debug_log::enabled());

    debug_log::init(&p);
    assert!(debug_log::enabled());
    dlog!("HELLO {} {}", "world", 42);
    dlog!("SECOND line");

    let contents = std::fs::read_to_string(&path).expect("log file written");
    assert!(contents.contains("botc-tui debug log start"));
    assert!(contents.contains("HELLO world 42"));
    assert!(contents.contains("SECOND line"));
    assert!(!contents.contains("ignored-before-init"));
    // Timestamp prefix like "[HH:MM:SS.mmm +....ms]".
    assert!(
        contents.lines().all(|l| l.starts_with('[') && l.contains("ms]")),
        "missing timestamp prefix: {contents}"
    );
    let _ = std::fs::remove_file(&path);
}
