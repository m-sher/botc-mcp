//! Storyteller policy knobs: registration draws, host night decisions, false-info queue.

use crate::game::ids::SeatId;

/// How Spy/Recluse registration draws resolve (default: random coin flips).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RegistrationMode {
    /// Independent p=0.5 draws (almanac-style default).
    #[default]
    Random,
    /// Never misregister or hide — always true type/alignment/token.
    AlwaysTrue,
    /// Always take the legal misregister/hide branch when Spy/Recluse and not disabled.
    AlwaysMisreg,
}

/// Night decisions that pause the queue until the host resolves them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingHostDecision {
    /// Imp attacked an active Mayor; host chooses bounce / nobody / Mayor dies.
    MayorRedirect {
        mayor: SeatId,
        /// Other living seats (host may pick any; non-killable → nobody dies).
        living_others: Vec<SeatId>,
    },
    /// Imp self-targeted with living minions; host chooses which becomes the Imp.
    /// Imp is still alive in the grimoire until the host resolves this decision.
    StarpassPick {
        /// Living minion seats at the moment of starpass (Imp still alive).
        minions: Vec<SeatId>,
        /// Imp seat that will die when starpass completes.
        dead_imp: SeatId,
    },
}

/// Explicit host resolution for [`PendingHostDecision`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostDecision {
    MayorRedirect {
        choice: MayorRedirectChoice,
    },
    StarpassPick {
        minion: SeatId,
    },
}

/// Mayor bounce host choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MayorRedirectChoice {
    /// Kill the Mayor with the demon attack.
    KillMayor,
    /// Attempt to kill `target`; immune/dead/non-killable → nobody dies.
    KillOther { target: SeatId },
    /// Nobody dies (Mayor survives).
    Nobody,
}
