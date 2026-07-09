//! Coarse phase machine and night step identities.

use crate::game::ids::SeatId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Lobby,
    /// First night; `cursor` indexes into [`crate::game::state::Game::night_queue`].
    FirstNight { cursor: usize },
    Day { day: u32, stage: DayStage },
    /// Subsequent nights (`night >= 2`); `cursor` indexes into `night_queue`.
    Night { night: u32, cursor: usize },
    Ended { winner: Winner, reason: EndReason },
}

impl Phase {
    /// Night queue cursor when in FirstNight / Night; otherwise `None`.
    pub fn cursor_if_night(&self) -> Option<usize> {
        match self {
            Phase::FirstNight { cursor } | Phase::Night { cursor, .. } => Some(*cursor),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayStage {
    Discussion,
    Nominations,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Winner {
    Good,
    Evil,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    DemonDead,
    EvilTwoAlive,
    SaintExecuted,
    MayorThreeNoExec,
}

/// One concrete wake / ST step in a built night queue (spec §9.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NightStep {
    /// ST internal markers already applied at start; no player wait if complete.
    SetupMarkers,
    /// Evil learns each other (n ≥ 7).
    MinionBriefing,
    /// Imp learns minions + bluffs (n ≥ 7).
    DemonBriefing,
    Poisoner { seat: SeatId },
    Spy { seat: SeatId },
    Washerwoman { seat: SeatId },
    Librarian { seat: SeatId },
    Investigator { seat: SeatId },
    Chef { seat: SeatId },
    Empath { seat: SeatId },
    FortuneTeller { seat: SeatId },
    Butler { seat: SeatId },
    Monk { seat: SeatId },
    /// Imp chooses a kill target (not on first night).
    DemonKill { seat: SeatId },
    Ravenkeeper { seat: SeatId },
    Undertaker { seat: SeatId },
    Dawn,
}
