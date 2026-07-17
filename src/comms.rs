//! Public table log and per-seat private Storyteller inbox.
//!
//! Policy: agent-to-agent communication is **public only**.
//! Private messages are Storyteller → single seat (abilities, briefings).

use crate::game::SeatId;
use crate::roles::Team;
use std::collections::HashMap;

/// Monotonic event id for polling cursors.
pub type EventId = u64;

/// Append-only public feed (design §12 / architecture public log).
///
/// Everyone with a game token can read the full sequence via `get_public_log`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicEvent {
    /// Player speech — visible to every agent.
    ///
    /// Optional `to` addresses another seat **publicly** (not a whisper). The harness
    /// may immediately wake that seat; the message remains on the shared log.
    Chat {
        seat: SeatId,
        name: String,
        text: String,
        /// When set, this was directed at another seat (still fully public).
        to: Option<SeatId>,
    },
    StorytellerAnnounce {
        text: String,
    },
    Nominated {
        by: SeatId,
        target: SeatId,
    },
    VoteCast {
        seat: SeatId,
        nominee: SeatId,
        /// `true` = hand up / support execution.
        support: bool,
    },
    Executed {
        seat: SeatId,
    },
    NoExecution,
    DiedInNight {
        seats: Vec<SeatId>,
    },
    /// Immediate day death (Slayer success, Virgin bounce, etc.).
    PlayerDied {
        seat: SeatId,
    },
    /// Optional public miss signal; engine may also stay silent.
    SlayerMiss {
        slayer: SeatId,
        target: SeatId,
    },
    PhaseChanged {
        summary: String,
    },
    GameEnded {
        winner: Team,
    },
}

#[derive(Debug, Clone)]
pub struct PublicLog {
    next_id: EventId,
    events: Vec<(EventId, PublicEvent)>,
}

impl Default for PublicLog {
    fn default() -> Self {
        Self {
            next_id: 1,
            events: Vec::new(),
        }
    }
}

impl PublicLog {
    pub fn push(&mut self, event: PublicEvent) -> EventId {
        let id = self.next_id;
        self.next_id += 1;
        self.events.push((id, event));
        id
    }

    /// Events with id > cursor (cursor 0 = from start).
    pub fn since(&self, cursor: EventId) -> Vec<(EventId, &PublicEvent)> {
        self.events
            .iter()
            .filter(|(id, _)| *id > cursor)
            .map(|(id, e)| (*id, e))
            .collect()
    }
}

/// Storyteller-only payload for one seat. Never copied into [`PublicLog`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivateMessage {
    /// Initial (or updated) identity as this player should see it.
    YouAre {
        /// What the player should believe (Drunk: Townsfolk face; others: true).
        character_label: String,
        team: Team,
        rules_path: String,
        note: Option<String>,
    },
    NightPrompt {
        text: String,
    },
    NightResult {
        text: String,
    },
    EvilBriefing {
        /// Structured later; free text for v1.
        text: String,
    },
    System {
        text: String,
    },
}

#[derive(Debug, Default)]
pub struct PrivateInboxes {
    by_seat: HashMap<SeatId, Vec<(EventId, PrivateMessage)>>,
    next_id: EventId,
}

impl PrivateInboxes {
    pub fn push(&mut self, seat: SeatId, msg: PrivateMessage) -> EventId {
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        self.by_seat.entry(seat).or_default().push((id, msg));
        id
    }

    pub fn since(&self, seat: SeatId, cursor: EventId) -> Vec<(EventId, &PrivateMessage)> {
        self.by_seat
            .get(&seat)
            .map(|v| {
                v.iter()
                    .filter(|(id, _)| *id > cursor)
                    .map(|(id, m)| (*id, m))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Reject any attempt to model player–player private channels.
#[derive(Debug, Clone, Copy)]
pub struct PrivatePlayerCommsDisabled;

impl PrivatePlayerCommsDisabled {
    pub fn whisper_allowed() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_inbox_is_per_seat() {
        let mut boxes = PrivateInboxes::default();
        boxes.push(
            SeatId(0),
            PrivateMessage::System {
                text: "only-0".into(),
            },
        );
        boxes.push(
            SeatId(1),
            PrivateMessage::System {
                text: "only-1".into(),
            },
        );
        assert_eq!(boxes.since(SeatId(0), 0).len(), 1);
        assert!(format!("{:?}", boxes.since(SeatId(0), 0)[0].1).contains("only-0"));
        assert!(!format!("{:?}", boxes.since(SeatId(1), 0)[0].1).contains("only-0"));
    }

    #[test]
    fn private_inbox_cursor_and_empty_seat() {
        let mut boxes = PrivateInboxes::default();
        let id1 = boxes.push(SeatId(0), PrivateMessage::System { text: "a".into() });
        let id2 = boxes.push(SeatId(0), PrivateMessage::NightPrompt { text: "b".into() });
        assert!(id2 > id1);
        assert_eq!(boxes.since(SeatId(0), id1).len(), 1);
        assert_eq!(boxes.since(SeatId(0), id2).len(), 0);
        // Unknown seat → empty, not panic.
        assert!(boxes.since(SeatId(9), 0).is_empty());
    }

    #[test]
    fn public_log_push_and_since_cursor() {
        let mut log = PublicLog::default();
        let e1 = log.push(PublicEvent::StorytellerAnnounce {
            text: "Night has fallen.".into(),
        });
        let e2 = log.push(PublicEvent::Chat {
            seat: SeatId(0),
            name: "Alice".into(),
            text: "hello".into(),
            to: None,
        });
        assert_eq!(e1, 1);
        assert_eq!(e2, 2);
        assert_eq!(log.since(0).len(), 2);
        assert_eq!(log.since(e1).len(), 1);
        assert_eq!(log.since(e2).len(), 0);
        assert!(matches!(
            log.since(e1)[0].1,
            PublicEvent::Chat { text, to: None, .. } if text == "hello"
        ));
    }

    #[test]
    fn public_event_variants_match_spec() {
        // Construct every §12 public event type so the enum stays complete.
        let samples = vec![
            PublicEvent::Chat {
                seat: SeatId(0),
                name: "A".into(),
                text: "hi".into(),
                to: Some(SeatId(2)),
            },
            PublicEvent::StorytellerAnnounce {
                text: "Nominations open.".into(),
            },
            PublicEvent::Nominated {
                by: SeatId(0),
                target: SeatId(1),
            },
            PublicEvent::VoteCast {
                seat: SeatId(2),
                nominee: SeatId(1),
                support: true,
            },
            PublicEvent::Executed { seat: SeatId(1) },
            PublicEvent::NoExecution,
            PublicEvent::DiedInNight {
                seats: vec![SeatId(3)],
            },
            PublicEvent::PlayerDied { seat: SeatId(4) },
            PublicEvent::SlayerMiss {
                slayer: SeatId(0),
                target: SeatId(1),
            },
            PublicEvent::PhaseChanged {
                summary: "Day 1 Discussion".into(),
            },
            PublicEvent::GameEnded { winner: Team::Good },
        ];
        let mut log = PublicLog::default();
        for e in samples {
            log.push(e);
        }
        assert_eq!(log.since(0).len(), 11);
    }

    #[test]
    fn private_message_variants_cover_brief() {
        let mut boxes = PrivateInboxes::default();
        boxes.push(
            SeatId(0),
            PrivateMessage::YouAre {
                character_label: "Empath".into(),
                team: Team::Good,
                rules_path: "docs/roles/townsfolk/empath.md".into(),
                note: None,
            },
        );
        boxes.push(
            SeatId(0),
            PrivateMessage::NightPrompt {
                text: "Choose two players.".into(),
            },
        );
        boxes.push(SeatId(0), PrivateMessage::NightResult { text: "1".into() });
        boxes.push(
            SeatId(0),
            PrivateMessage::EvilBriefing {
                text: "Your Demon is seat 2.".into(),
            },
        );
        boxes.push(
            SeatId(0),
            PrivateMessage::System {
                text: "You are now the Imp.".into(),
            },
        );
        assert_eq!(boxes.since(SeatId(0), 0).len(), 5);
        // Drunk face must never appear as label in YouAre content tests elsewhere;
        // here we only assert YouAre carries the face label field.
        let you = &boxes.since(SeatId(0), 0)[0].1;
        assert!(matches!(
            you,
            PrivateMessage::YouAre {
                character_label,
                ..
            } if character_label == "Empath"
        ));
    }

    #[test]
    fn player_player_whispers_disabled() {
        assert!(!PrivatePlayerCommsDisabled::whisper_allowed());
    }

    #[test]
    fn private_ids_do_not_leak_across_seats_in_since() {
        let mut boxes = PrivateInboxes::default();
        boxes.push(SeatId(0), PrivateMessage::System { text: "s0".into() });
        boxes.push(SeatId(1), PrivateMessage::System { text: "s1".into() });
        boxes.push(SeatId(0), PrivateMessage::System { text: "s0b".into() });
        let s0: Vec<_> = boxes
            .since(SeatId(0), 0)
            .into_iter()
            .map(|(_, m)| m)
            .collect();
        assert_eq!(s0.len(), 2);
        for m in s0 {
            assert!(!format!("{m:?}").contains("s1"));
        }
    }
}
