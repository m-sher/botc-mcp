//! Public table log and per-seat private Storyteller inbox.
//!
//! Policy: agent-to-agent communication is **public only**.
//! Private messages are Storyteller → single seat (abilities, briefings).

use crate::game::SeatId;
use crate::roles::Team;
use std::collections::HashMap;

/// Monotonic event id for polling cursors.
pub type EventId = u64;

#[derive(Debug, Clone)]
pub enum PublicEvent {
    /// Player speech — visible to every agent.
    Chat {
        seat: SeatId,
        name: String,
        text: String,
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
        support: bool,
    },
    Executed {
        seat: SeatId,
    },
    NoExecution,
    DiedInNight {
        seats: Vec<SeatId>,
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
#[derive(Debug, Clone)]
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
        /// Structured later; free text for sketch.
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
