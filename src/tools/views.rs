//! Tool-layer view DTOs (public / private / host / character rules).

use crate::comms::{EventId, PrivateMessage};
use crate::game::{ChoiceSchema, PendingHostDecision, PublicSeatView, SeatId, Winner};

/// Public table snapshot — no roles, no pending night seat identity.
#[derive(Debug, Clone)]
pub struct PublicStateView {
    pub phase: String,
    pub seats: Vec<PublicSeatView>,
    pub winner: Option<Winner>,
}

/// Structured “you must act” info (acting seat only; never leaks other seats’ wakes).
#[derive(Debug, Clone)]
pub struct AwaitingView {
    pub action: &'static str,
    pub prompt: String,
    pub schema: ChoiceSchema,
}

/// Player-facing private snapshot. Must never expose Drunk as `character_label`.
#[derive(Debug)]
pub struct PrivateStateView {
    pub seat: SeatId,
    pub name: String,
    pub alive: bool,
    /// Character the player should play as (Drunk → Townsfolk face only).
    pub character_label: Option<String>,
    pub team_label: Option<String>,
    pub rules_path: Option<String>,
    pub private_messages_since: Vec<(EventId, PrivateMessage)>,
    /// True only when this seat has a pending night (or later day) action.
    pub awaiting_action: bool,
    /// Details for the pending action; `None` unless this seat must act.
    pub awaiting: Option<AwaitingView>,
}

/// One seat in the host grimoire (true roles + markers).
#[derive(Debug, Clone)]
pub struct HostSeatView {
    pub seat_id: SeatId,
    pub name: String,
    pub alive: bool,
    pub ghost_vote_available: bool,
    pub true_character: Option<&'static str>,
    pub believed_character: Option<&'static str>,
    pub poisoned: bool,
    pub is_drunk_outsider: bool,
    pub monk_protected_tonight: bool,
    pub slayer_used: bool,
    pub virgin_ability_used: bool,
    pub butler_master: Option<SeatId>,
}

/// Pending wake as seen by the host (includes acting seat — not public).
#[derive(Debug, Clone)]
pub struct HostPendingView {
    pub seat_id: SeatId,
    pub prompt: String,
    pub schema: ChoiceSchema,
    pub step_debug: String,
}

/// Host-only pending Storyteller decision (Mayor / starpass / night info / day reg).
#[derive(Debug, Clone)]
pub struct HostDecisionView {
    pub kind: String,
    pub detail: String,
    pub seats: Vec<SeatId>,
}

/// Host-only full grimoire + pending + seed/salt (eval/debug).
#[derive(Debug, Clone)]
pub struct HostStateView {
    pub seed: u64,
    /// Per-game secret mixed into RNG substreams. Never exposed on player views.
    pub secret_salt: u64,
    pub phase: String,
    pub seats: Vec<HostSeatView>,
    pub pending: Option<HostPendingView>,
    /// Host Storyteller choice (not shown on player views).
    pub pending_host: Option<HostDecisionView>,
    pub registration_mode: String,
    pub st_choice_mode: String,
    pub host_lie_queue_len: usize,
    pub red_herring: Option<SeatId>,
    pub demon_bluffs: Vec<&'static str>,
    pub winner: Option<Winner>,
}

impl HostDecisionView {
    pub fn from_pending(p: &PendingHostDecision) -> Self {
        match p {
            PendingHostDecision::MayorRedirect {
                mayor,
                living_others,
            } => Self {
                kind: "mayor_redirect".into(),
                detail: format!("mayor seat {}", mayor.0),
                seats: living_others.clone(),
            },
            PendingHostDecision::StarpassPick {
                minions,
                dead_imp,
            } => Self {
                kind: "starpass_pick".into(),
                detail: format!("dead_imp seat {}", dead_imp.0),
                seats: minions.clone(),
            },
            PendingHostDecision::NightInfo {
                seat,
                ability,
                reason,
                ..
            } => Self {
                kind: "night_info".into(),
                detail: format!("{ability} for seat {} ({reason})", seat.0),
                seats: vec![*seat],
            },
        }
    }
}

/// Public character sheet entry with loaded markdown body.
#[derive(Debug, Clone)]
pub struct CharacterRulesView {
    pub name: &'static str,
    pub path: &'static str,
    pub team: String,
    pub character_type: String,
    /// Full markdown from `docs/roles/...`.
    pub text: String,
}
