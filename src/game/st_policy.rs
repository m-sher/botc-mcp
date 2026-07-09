//! Storyteller policy: host-first decisions with random fallback on skip.

use crate::game::ids::SeatId;
use crate::game::phase::NightStep;

/// How the engine treats Storyteller discretion at runtime.
///
/// Default is [`StChoiceMode::HostFirst`]: the host must choose (or skip to random).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StChoiceMode {
    /// Pause for `host_decide`; `skip_night_action` applies the seeded-random default.
    #[default]
    HostFirst,
    /// Immediately use seeded-random ST policy (eval/harness convenience).
    Random,
}

/// How Spy/Recluse registration draws resolve when the engine applies the random path
/// (skip fallback, or [`StChoiceMode::Random`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RegistrationMode {
    /// Independent p=0.5 draws (almanac-style default for the random path).
    #[default]
    Random,
    /// Never misregister or hide — always true type/alignment/token.
    AlwaysTrue,
    /// Always take the legal misregister/hide branch when Spy/Recluse and not disabled.
    AlwaysMisreg,
}

/// Night/day decisions that pause until the host resolves them.
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
    /// Storyteller must author a private night info result (pair info, counts, grimoire, etc.).
    ///
    /// On skip, the engine runs the seeded-random resolution for `step`.
    NightInfo {
        /// Recipient seat for the private result.
        seat: SeatId,
        /// Night step being resolved (cursor stays here until host finishes).
        step: NightStep,
        /// Ability label for host UI ("Washerwoman", "Empath", …).
        ability: String,
        /// Why the host is asked (e.g. "pair_info", "false_info", "registration").
        reason: String,
        /// Optional player payload already collected (Fortune Teller / Ravenkeeper).
        payload: Option<NightInfoPayload>,
    },
    /// Virgin: does the Spy nominator register as Townsfolk? (skip → random).
    VirginSpyReg {
        nominator: SeatId,
        virgin: SeatId,
    },
    /// Slayer: does the Recluse target register as Demon? (skip → random).
    SlayerRecluseReg {
        slayer: SeatId,
        target: SeatId,
    },
}

/// Payload needed to finish a night info step after a host pause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NightInfoPayload {
    PickTwo { a: SeatId, b: SeatId },
    PickOne { target: SeatId },
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
    /// Free-text private result for [`PendingHostDecision::NightInfo`].
    NightInfo {
        text: String,
    },
    /// Boolean registration choice for Virgin Spy / Slayer Recluse.
    Registration {
        /// `true` = misregister as the relevant type (Townsfolk / Demon).
        register: bool,
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

impl StChoiceMode {
    pub fn is_host_first(self) -> bool {
        matches!(self, StChoiceMode::HostFirst)
    }
}

impl PendingHostDecision {
    pub fn kind_str(&self) -> &'static str {
        match self {
            PendingHostDecision::MayorRedirect { .. } => "mayor_redirect",
            PendingHostDecision::StarpassPick { .. } => "starpass_pick",
            PendingHostDecision::NightInfo { .. } => "night_info",
            PendingHostDecision::VirginSpyReg { .. } => "virgin_spy_reg",
            PendingHostDecision::SlayerRecluseReg { .. } => "slayer_recluse_reg",
        }
    }
}
