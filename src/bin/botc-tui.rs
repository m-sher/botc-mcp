//! Interactive multi-agent Trouble Brewing monitor.
//!
//! ```text
//! cargo build --bins
//! cargo run --bin botc-tui
//! ```

use botc_mcp::harness::run_tui;

fn main() {
    if let Err(e) = run_tui() {
        eprintln!("botc-tui error: {e}");
        std::process::exit(1);
    }
}
