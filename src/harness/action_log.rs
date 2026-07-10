//! Central action feed: every agent tool RPC that reaches the shared engine.
//!
//! All proxies funnel through the socket's `dispatch`, so recording there gives
//! one authoritative, chronological view of what every agent is *doing* (not just
//! thinking) — including failed and read-only calls. The TUI renders this feed and
//! highlights game-affecting actions.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::Instant;

use serde_json::Value;

/// How prominently to show an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    /// Changes or advances the game (say/nominate/vote/night_action/host moves).
    Game,
    /// Read-only inspection (get_*_state, list_*, rules).
    Info,
    /// Anything else (setup, unknown).
    Meta,
}

/// Which agent issued an action (for labels + per-agent colour).
#[derive(Debug, Clone)]
pub struct ActorLabel {
    pub name: String,
    pub seat: Option<u8>,
    pub is_host: bool,
}

/// One recorded action.
#[derive(Debug, Clone)]
pub struct ActionEntry {
    pub seq: u64,
    /// Seconds since the log started (monotonic; for "12s ago"-style display).
    pub secs: u64,
    pub actor: ActorLabel,
    pub tool: String,
    /// Short human summary of the salient args (e.g. `→P1 YES`).
    pub summary: String,
    pub kind: ActionKind,
    pub ok: bool,
    pub error: Option<String>,
}

struct Inner {
    entries: VecDeque<ActionEntry>,
    seq: u64,
    /// token string -> actor label, set once at launch.
    labels: HashMap<String, ActorLabel>,
}

/// Bounded, shared ring buffer of agent actions.
pub struct ActionLog {
    inner: Mutex<Inner>,
    start: Instant,
    cap: usize,
}

impl Default for ActionLog {
    fn default() -> Self {
        Self::new(2000)
    }
}

impl ActionLog {
    pub fn new(cap: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                entries: VecDeque::new(),
                seq: 0,
                labels: HashMap::new(),
            }),
            start: Instant::now(),
            cap,
        }
    }

    /// Register token -> actor labels (called once, after the game is created).
    pub fn set_labels(&self, labels: HashMap<String, ActorLabel>) {
        self.inner.lock().unwrap().labels = labels;
    }

    /// Record one dispatched RPC. `token` is the raw token string from the args.
    pub fn record_rpc(
        &self,
        token: Option<&str>,
        tool: &str,
        args: &Value,
        ok: bool,
        error: Option<String>,
    ) {
        let secs = self.start.elapsed().as_secs();
        let mut g = self.inner.lock().unwrap();
        let actor = token
            .and_then(|t| g.labels.get(t).cloned())
            .unwrap_or(ActorLabel {
                name: "?".into(),
                seat: None,
                is_host: false,
            });
        g.seq += 1;
        let seq = g.seq;
        let entry = ActionEntry {
            seq,
            secs,
            actor,
            tool: tool.to_string(),
            summary: summarize(tool, args),
            kind: classify(tool),
            ok,
            error,
        };
        g.entries.push_back(entry);
        while g.entries.len() > self.cap {
            g.entries.pop_front();
        }
    }

    /// The most recent `n` entries, oldest-first (ready to render top→bottom).
    pub fn recent(&self, n: usize) -> Vec<ActionEntry> {
        let g = self.inner.lock().unwrap();
        let start = g.entries.len().saturating_sub(n);
        g.entries.iter().skip(start).cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Classify a tool for display prominence.
pub fn classify(tool: &str) -> ActionKind {
    match tool {
        "say" | "st_announce" | "night_action" | "day_action" | "nominate" | "vote"
        | "pass_vote" | "open_nominations" | "close_vote" | "end_nominations"
        | "skip_night_action" | "host_decide" | "host_queue_lie" | "start_game" => ActionKind::Game,
        "get_public_state" | "get_public_log" | "get_private_state" | "get_host_state"
        | "get_character_rules" | "list_characters" | "list_rules_topics" | "get_rules_topic" => {
            ActionKind::Info
        }
        _ => ActionKind::Meta,
    }
}

/// `P{n}` for a seat-number value, else `None`.
fn seatp(v: Option<&Value>) -> Option<String> {
    v.and_then(|x| x.as_u64()).map(|n| format!("P{n}"))
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.trim().replace('\n', " ");
    if s.chars().count() <= max {
        s
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// Short, readable summary of the salient args for a tool call.
pub fn summarize(tool: &str, args: &Value) -> String {
    let payload = args.get("payload").unwrap_or(args);
    match tool {
        "say" | "st_announce" => args
            .get("text")
            .and_then(|v| v.as_str())
            .map(|t| format!("“{}”", truncate(t, 44)))
            .unwrap_or_default(),
        "nominate" => seatp(args.get("target"))
            .map(|s| format!("→{s}"))
            .unwrap_or_default(),
        "vote" => {
            let who = seatp(args.get("nominee").or_else(|| args.get("target"))).unwrap_or_default();
            let yes = args.get("support").and_then(|v| v.as_bool()).unwrap_or(false);
            format!("→{who} {}", if yes { "YES" } else { "no" })
        }
        "pass_vote" => "abstain".into(),
        "day_action" => seatp(payload.get("target").or_else(|| args.get("target")))
            .map(|s| format!("slay →{s}"))
            .unwrap_or_else(|| "slay".into()),
        "night_action" => {
            if let Some(t) = seatp(payload.get("target")) {
                t
            } else if let (Some(a), Some(b)) = (
                seatp(payload.get("target_a").or_else(|| payload.get("a"))),
                seatp(payload.get("target_b").or_else(|| payload.get("b"))),
            ) {
                format!("{a},{b}")
            } else if let Some(arr) = payload.get("targets").and_then(|v| v.as_array()) {
                let names: Vec<String> = arr.iter().filter_map(|v| seatp(Some(v))).collect();
                names.join(",")
            } else {
                String::new()
            }
        }
        "host_decide" => {
            let choice = args
                .get("decision")
                .or_else(|| args.get("choice"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let tgt = seatp(args.get("target").or_else(|| args.get("seat")))
                .map(|s| format!(" →{s}"))
                .unwrap_or_default();
            format!("{choice}{tgt}").trim().to_string()
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_game_vs_info() {
        assert_eq!(classify("vote"), ActionKind::Game);
        assert_eq!(classify("nominate"), ActionKind::Game);
        assert_eq!(classify("host_decide"), ActionKind::Game);
        assert_eq!(classify("get_public_state"), ActionKind::Info);
        assert_eq!(classify("list_characters"), ActionKind::Info);
        assert_eq!(classify("create_game"), ActionKind::Meta);
    }

    #[test]
    fn summarizes_key_actions() {
        assert_eq!(summarize("vote", &json!({"nominee": 1, "support": true})), "→P1 YES");
        assert_eq!(summarize("vote", &json!({"nominee": 2, "support": false})), "→P2 no");
        assert_eq!(summarize("nominate", &json!({"target": 3})), "→P3");
        assert_eq!(summarize("pass_vote", &json!({})), "abstain");
        assert_eq!(
            summarize("night_action", &json!({"payload": {"target": 4}})),
            "P4"
        );
        assert_eq!(summarize("say", &json!({"text": "hi"})), "“hi”");
        assert_eq!(summarize("get_public_state", &json!({})), "");
    }

    #[test]
    fn records_and_labels() {
        let log = ActionLog::new(4);
        let mut labels = HashMap::new();
        labels.insert(
            "tok-host".to_string(),
            ActorLabel { name: "Host".into(), seat: None, is_host: true },
        );
        labels.insert(
            "tok-p1".to_string(),
            ActorLabel { name: "P1".into(), seat: Some(1), is_host: false },
        );
        log.set_labels(labels);
        log.record_rpc(Some("tok-p1"), "vote", &json!({"nominee": 0, "support": true}), true, None);
        log.record_rpc(Some("tok-host"), "close_vote", &json!({}), true, None);
        let r = log.recent(10);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].actor.name, "P1");
        assert_eq!(r[0].tool, "vote");
        assert_eq!(r[0].kind, ActionKind::Game);
        assert_eq!(r[1].actor.name, "Host");
        // ring buffer cap
        for _ in 0..10 {
            log.record_rpc(Some("tok-p1"), "get_public_state", &json!({}), true, None);
        }
        assert!(log.len() <= 4);
    }
}
