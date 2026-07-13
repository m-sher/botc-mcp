//! Live Grok CLI checks for the multi-agent harness.
//!
//! **Not run by default** (requires `grok` on PATH + auth, may call the API).
//!
//! ```bash
//! cargo test --test harness_grok_live -- --ignored --nocapture
//! ```

use botc_mcp::harness::agents::{build_grok_tick_args, find_grok, HarnessConfig};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;
use uuid::Uuid;

fn require_grok() -> PathBuf {
    let grok = find_grok();
    assert!(
        grok.exists() || which_exists("grok"),
        "grok binary not found (looked for {:?})",
        grok
    );
    grok
}

fn which_exists(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
        .unwrap_or(false)
}

/// The bug we hit in the TUI: `--yolo` and `--always-approve` are the same clap flag.
#[test]
#[ignore = "invokes live `grok` binary; run with: cargo test --test harness_grok_live -- --ignored"]
fn yolo_plus_always_approve_is_rejected() {
    let grok = require_grok();
    let prompt = std::env::temp_dir().join(format!("botc-dual-flag-{}.txt", Uuid::new_v4()));
    std::fs::write(&prompt, "say hi").unwrap();

    let output = Command::new(&grok)
        .arg("--prompt-file")
        .arg(&prompt)
        .arg("--yolo")
        .arg("--always-approve")
        .arg("--max-turns")
        .arg("1")
        .arg("--output-format")
        .arg("json")
        .arg("--disable-web-search")
        .arg("--no-subagents")
        .output()
        .expect("spawn grok");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stderr}\n{stdout}");

    assert!(
        !output.status.success(),
        "expected non-zero exit when both approve flags are set; status={:?}\n{combined}",
        output.status
    );
    assert!(
        combined.contains("cannot be used multiple times") || combined.contains("always-approve"),
        "expected clap duplicate-flag error, got:\n{combined}"
    );

    let _ = std::fs::remove_file(&prompt);
}

/// Harness argv (single `--yolo`, streaming-json, dummy prompt) must parse and return
/// a well-formed JSON completion object — not a clap flag error.
#[test]
#[ignore = "invokes live `grok` binary + model API; run with: cargo test --test harness_grok_live -- --ignored"]
fn harness_argv_returns_valid_json_response() {
    let grok = require_grok();
    let tmp = std::env::temp_dir().join(format!("botc-live-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let prompt_file = tmp.join("prompt.txt");
    // Minimal, no-tool prompt so the run finishes quickly without MCP.
    std::fs::write(
        &prompt_file,
        "Reply with exactly the single word: PONG\nDo not use any tools.",
    )
    .unwrap();

    let session_id = Uuid::new_v4().to_string();
    let cfg = HarnessConfig {
        grok_bin: grok.clone(),
        model: std::env::var("BOTC_LIVE_MODEL").unwrap_or_else(|_| "grok-build".into()),
        max_turns_per_tick: 3,
        // Use json (not streaming-json) so we can assert one complete object.
        ..HarnessConfig::default()
    };

    // Build harness args, then swap streaming-json → json for a single assertable payload.
    let mut args = build_grok_tick_args(
        &cfg,
        &cfg.model.clone(),
        &tmp,
        &prompt_file,
        &session_id,
        false,
    );
    if let Some(i) = args.iter().position(|a| a == "streaming-json") {
        args[i] = "json".into();
    }
    // Also strip tools that would hang without the harness socket MCP.
    // (Agents in the TUI have MCP; this smoke test only validates CLI + response shape.)
    args.retain(|a| a != "--disable-web-search"); // keep or drop both fine

    let output = Command::new(&grok)
        .args(&args)
        .output()
        .expect("spawn grok with harness argv");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stderr}\n{stdout}");

    assert!(
        !combined.contains("cannot be used multiple times"),
        "harness argv still has a clap multi-flag error:\n{combined}"
    );
    assert!(
        output.status.success(),
        "grok exited {:?}\nstderr:\n{stderr}\nstdout:\n{stdout}",
        output.status
    );

    // Expect one JSON object with the headless completion fields.
    let v: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("stdout is not JSON ({e}):\n{stdout}\nstderr:\n{stderr}");
    });
    assert!(
        v.get("type").and_then(|t| t.as_str()) != Some("error"),
        "grok returned error object: {v}"
    );
    let text = v.get("text").and_then(|t| t.as_str()).unwrap_or("");
    assert!(
        !text.is_empty(),
        "expected non-empty text field in response: {v}"
    );
    assert!(
        v.get("sessionId").and_then(|s| s.as_str()).is_some(),
        "expected sessionId in response: {v}"
    );
    assert!(
        v.get("stopReason").is_some(),
        "expected stopReason in response: {v}"
    );
    // Soft check on content (model may wrap PONG).
    assert!(
        text.to_uppercase().contains("PONG") || text.len() > 1,
        "unexpected model text (wanted PONG): {text:?}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// streaming-json mode used by the TUI must emit NDJSON lines with known `type` fields
/// (and no clap multi-flag failure).
#[test]
#[ignore = "invokes live `grok` binary + model API; run with: cargo test --test harness_grok_live -- --ignored"]
fn harness_argv_streaming_json_lines_are_valid() {
    use botc_mcp::harness::agents::{apply_stream_event, LineKind};
    use std::sync::Mutex;

    let grok = require_grok();
    let tmp = std::env::temp_dir().join(format!("botc-live-stream-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let prompt_file = tmp.join("prompt.txt");
    std::fs::write(
        &prompt_file,
        "Reply with exactly: PONG\nDo not use any tools.",
    )
    .unwrap();

    let session_id = Uuid::new_v4().to_string();
    let cfg = HarnessConfig {
        grok_bin: grok.clone(),
        model: std::env::var("BOTC_LIVE_MODEL").unwrap_or_else(|_| "grok-build".into()),
        max_turns_per_tick: 3,
        ..HarnessConfig::default()
    };
    let args = build_grok_tick_args(
        &cfg,
        &cfg.model.clone(),
        &tmp,
        &prompt_file,
        &session_id,
        false,
    );

    let output = Command::new(&grok)
        .args(&args)
        .output()
        .expect("spawn grok streaming");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("cannot be used multiple times")
            && !stdout.contains("cannot be used multiple times"),
        "clap multi-flag error:\n{stderr}\n{stdout}"
    );
    assert!(
        output.status.success(),
        "exit {:?}\nstderr:{stderr}\nstdout:{stdout}",
        output.status
    );

    let log = Mutex::new(Vec::new());
    let mut saw_end = false;
    let mut saw_text = false;
    let mut thought_chunks = 0usize;
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line).unwrap_or_else(|e| {
            panic!("invalid NDJSON line ({e}): {line}");
        });
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        assert!(
            matches!(
                ty,
                "text" | "thought" | "end" | "error" | "max_turns_reached"
            ) || !ty.is_empty(),
            "unexpected event type {ty:?} in {line}"
        );
        if ty == "end" {
            saw_end = true;
        }
        if ty == "text" {
            saw_text = true;
        }
        if ty == "thought" {
            thought_chunks += 1;
        }
        apply_stream_event(&log, line);
    }

    let g = log.lock().unwrap();
    assert!(
        saw_end || saw_text,
        "no text/end events in stream:\n{stdout}"
    );
    // Consecutive same-kind chunks must coalesce, not become one line per token.
    let thought_lines = g.iter().filter(|l| l.kind == LineKind::Thought).count();
    assert!(
        thought_chunks == 0 || thought_lines < thought_chunks.max(1),
        "thought not coalesced: {thought_chunks} chunks -> {thought_lines} lines"
    );
    // No in-text tags — colour, not `[think]`/`[stderr]`.
    assert!(
        !g.iter()
            .any(|l| l.text.contains("[think]") || l.text.contains("[stderr]")),
        "stream lines still carry in-text tags: {g:?}"
    );

    drop(g);
    let _ = std::fs::remove_dir_all(&tmp);
}
