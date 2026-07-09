//! Night queue construction and night machine (pending wake protocol §9.2).

use crate::auth::{Actor, Token};
use crate::comms::{PrivateMessage, PublicEvent};
use crate::error::GameError;
use crate::game::ids::SeatId;
use crate::game::phase::{DayStage, NightStep, Phase};
use crate::game::state::Game;
use crate::roles::night_order::{
    FirstNightSlot, OtherNightSlot, FIRST_NIGHT_CHARACTER_ORDER, OTHER_NIGHT_CHARACTER_ORDER,
};
use crate::roles::{Character, CharacterType};

/// Legal shape of a night choice (shown to the acting seat only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChoiceSchema {
    /// No target; player acknowledges the wake (info-only roles that still wait).
    Ack,
    /// One seat target.
    PickOne {
        /// If true, any seat id in the game; else living only when `living_only`.
        any_seat: bool,
        living_only: bool,
        exclude_self: bool,
    },
    /// Two seat targets (Fortune Teller).
    PickTwo { any_seat: bool },
}

/// Player payload for [`Game::night_action`]. Role is inferred from pending wake / seat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NightActionPayload {
    Ack,
    PickOne { target: SeatId },
    PickTwo { a: SeatId, b: SeatId },
    PickCharacter { name: String },
}

/// Active wake waiting on a player (or host skip).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingWake {
    pub step: NightStep,
    pub seat: SeatId,
    pub schema: ChoiceSchema,
    pub prompt: String,
}

/// Build the ordered first-night step list for the current grimoire (spec §9.1).
pub fn build_first_night_queue(game: &Game) -> Vec<NightStep> {
    let mut q = Vec::new();
    q.push(NightStep::SetupMarkers);

    let n = game.seats.len();
    if n >= 7 {
        if has_living_minion(game) {
            q.push(NightStep::MinionBriefing);
        }
        q.push(NightStep::DemonBriefing);
    }

    for slot in FIRST_NIGHT_CHARACTER_ORDER {
        if let Some(seat) = find_wake_seat(game, slot.character(), slot.uses_true_character()) {
            q.push(first_night_step(*slot, seat));
        }
    }

    q.push(NightStep::Dawn);
    q
}

/// Build the ordered other-night step list (no N1 setup/briefings; includes Imp kill).
///
/// Ravenkeeper is included only when a seat in `deaths_tonight` faces as Ravenkeeper
/// (typically filled after the demon kill resolves). Undertaker is included when the
/// seat is alive **and** there was an execution today (`executed_today`).
pub fn build_other_night_queue(game: &Game) -> Vec<NightStep> {
    let mut q = Vec::new();

    for slot in OTHER_NIGHT_CHARACTER_ORDER {
        match slot {
            OtherNightSlot::Ravenkeeper => {
                // Spec: true Ravenkeeper who died to the demon (Drunk face is not true RK).
                for &dead in &game.deaths_tonight {
                    if seat_matches_wake(game, dead, Character::Ravenkeeper, true) {
                        q.push(NightStep::Ravenkeeper { seat: dead });
                    }
                }
            }
            OtherNightSlot::Undertaker => {
                if game.executed_today.is_some() {
                    if let Some(seat) =
                        find_wake_seat(game, Character::Undertaker, false /* face */)
                    {
                        q.push(NightStep::Undertaker { seat });
                    }
                }
            }
            OtherNightSlot::Imp => {
                if let Some(seat) = find_wake_seat(game, Character::Imp, true) {
                    q.push(NightStep::DemonKill { seat });
                }
            }
            other => {
                let role = other.character();
                if let Some(seat) = find_wake_seat(game, role, other.uses_true_character()) {
                    q.push(other_night_step(*other, seat));
                }
            }
        }
    }

    q.push(NightStep::Dawn);
    q
}

fn first_night_step(slot: FirstNightSlot, seat: SeatId) -> NightStep {
    use FirstNightSlot::*;
    match slot {
        Poisoner => NightStep::Poisoner { seat },
        Spy => NightStep::Spy { seat },
        Washerwoman => NightStep::Washerwoman { seat },
        Librarian => NightStep::Librarian { seat },
        Investigator => NightStep::Investigator { seat },
        Chef => NightStep::Chef { seat },
        Empath => NightStep::Empath { seat },
        FortuneTeller => NightStep::FortuneTeller { seat },
        Butler => NightStep::Butler { seat },
    }
}

fn other_night_step(slot: OtherNightSlot, seat: SeatId) -> NightStep {
    use OtherNightSlot::*;
    match slot {
        Poisoner => NightStep::Poisoner { seat },
        Monk => NightStep::Monk { seat },
        Spy => NightStep::Spy { seat },
        Imp => NightStep::DemonKill { seat },
        Ravenkeeper => NightStep::Ravenkeeper { seat },
        Undertaker => NightStep::Undertaker { seat },
        Empath => NightStep::Empath { seat },
        FortuneTeller => NightStep::FortuneTeller { seat },
        Butler => NightStep::Butler { seat },
    }
}

fn has_living_minion(game: &Game) -> bool {
    game.seats.iter().any(|s| {
        s.alive
            && s.true_character
                .is_some_and(|c| c.character_type() == CharacterType::Minion)
    })
}

/// First living seat matching role via true character or player-facing character.
fn find_wake_seat(game: &Game, role: Character, use_true: bool) -> Option<SeatId> {
    game.seats.iter().find_map(|s| {
        if !s.alive {
            return None;
        }
        if seat_matches_wake(game, s.id, role, use_true) {
            Some(s.id)
        } else {
            None
        }
    })
}

fn seat_matches_wake(game: &Game, seat: SeatId, role: Character, use_true: bool) -> bool {
    let Some(s) = game.seats.iter().find(|x| x.id == seat) else {
        return false;
    };
    if use_true {
        s.true_character == Some(role)
    } else {
        game.player_facing_character(seat) == Some(role)
    }
}

// ---------------------------------------------------------------------------
// Night machine (§9.2)
// ---------------------------------------------------------------------------

impl Game {
    /// Process night queue from the cursor until a player choice is pending or dawn completes.
    pub fn night_tick(&mut self) {
        loop {
            if self.pending_night.is_some() {
                return;
            }
            if !matches!(self.phase, Phase::FirstNight { .. } | Phase::Night { .. }) {
                return;
            }
            if self.night_cursor >= self.night_queue.len() {
                return;
            }

            let step = self.night_queue[self.night_cursor];
            match self.begin_step(step) {
                StepOutcome::Advanced => {
                    // cursor already advanced
                }
                StepOutcome::Waiting => return,
                StepOutcome::DawnDone => return,
            }
        }
    }

    /// Player submits a night choice for the current pending wake.
    pub fn night_action(
        &mut self,
        token: &Token,
        payload: NightActionPayload,
    ) -> Result<(), GameError> {
        let actor = self.tokens.resolve(token).ok_or(GameError::Unauthorized)?;
        let seat = match actor {
            Actor::Player { seat } => seat,
            Actor::Host => {
                return Err(GameError::BadRequest("host cannot night_action"));
            }
        };
        self.apply_night_action(seat, payload)
    }

    /// Host applies a documented default for the pending wake and continues.
    pub fn skip_night_action(&mut self, host: &Token) -> Result<(), GameError> {
        match self.tokens.resolve(host) {
            Some(Actor::Host) => {}
            _ => return Err(GameError::Unauthorized),
        }
        let pending = self
            .pending_night
            .clone()
            .ok_or(GameError::IllegalAction("no pending night action"))?;
        let payload = self.default_payload_for(&pending)?;
        self.apply_night_action(pending.seat, payload)
    }

    fn apply_night_action(
        &mut self,
        seat: SeatId,
        payload: NightActionPayload,
    ) -> Result<(), GameError> {
        let pending = self
            .pending_night
            .clone()
            .ok_or(GameError::NotYourWake)?;
        if pending.seat != seat {
            return Err(GameError::NotYourWake);
        }
        if !matches!(self.phase, Phase::FirstNight { .. } | Phase::Night { .. }) {
            return Err(GameError::WrongPhase);
        }

        self.validate_payload(&pending, &payload)?;
        self.resolve_pending_stub(&pending, &payload)?;

        self.pending_night = None;
        self.advance_cursor();
        self.night_tick();
        Ok(())
    }

    fn begin_step(&mut self, step: NightStep) -> StepOutcome {
        match step {
            NightStep::SetupMarkers => {
                self.advance_cursor();
                StepOutcome::Advanced
            }
            NightStep::MinionBriefing => {
                self.push_minion_briefings();
                self.advance_cursor();
                StepOutcome::Advanced
            }
            NightStep::DemonBriefing => {
                self.push_demon_briefing();
                self.advance_cursor();
                StepOutcome::Advanced
            }
            NightStep::Dawn => {
                self.resolve_dawn();
                StepOutcome::DawnDone
            }
            // Info-only / ST-computed: stub NightResult and advance (full resolve Task 8–9).
            NightStep::Spy { seat }
            | NightStep::Washerwoman { seat }
            | NightStep::Librarian { seat }
            | NightStep::Investigator { seat }
            | NightStep::Chef { seat }
            | NightStep::Empath { seat }
            | NightStep::Undertaker { seat } => {
                self.private_inboxes.push(
                    seat,
                    PrivateMessage::NightResult {
                        text: format!(
                            "Night information for {} (stub).",
                            step_label(step)
                        ),
                    },
                );
                self.advance_cursor();
                StepOutcome::Advanced
            }
            // Choice-required wakes: set pending and stop.
            NightStep::Poisoner { seat } => {
                self.set_pending(
                    step,
                    seat,
                    ChoiceSchema::PickOne {
                        any_seat: true,
                        living_only: false,
                        exclude_self: false,
                    },
                    "Choose a player to poison tonight.",
                );
                StepOutcome::Waiting
            }
            NightStep::FortuneTeller { seat } => {
                self.set_pending(
                    step,
                    seat,
                    ChoiceSchema::PickTwo { any_seat: true },
                    "Choose two players. You learn if either is the Demon.",
                );
                StepOutcome::Waiting
            }
            NightStep::Butler { seat } => {
                self.set_pending(
                    step,
                    seat,
                    ChoiceSchema::PickOne {
                        any_seat: true,
                        living_only: false,
                        exclude_self: true,
                    },
                    "Choose a master for tomorrow.",
                );
                StepOutcome::Waiting
            }
            NightStep::Monk { seat } => {
                self.set_pending(
                    step,
                    seat,
                    ChoiceSchema::PickOne {
                        any_seat: false,
                        living_only: true,
                        exclude_self: true,
                    },
                    "Choose a living player to protect tonight.",
                );
                StepOutcome::Waiting
            }
            NightStep::DemonKill { seat } => {
                self.set_pending(
                    step,
                    seat,
                    ChoiceSchema::PickOne {
                        any_seat: true,
                        living_only: false,
                        exclude_self: false,
                    },
                    "Choose a player to kill tonight.",
                );
                StepOutcome::Waiting
            }
            NightStep::Ravenkeeper { seat } => {
                self.set_pending(
                    step,
                    seat,
                    ChoiceSchema::PickOne {
                        any_seat: true,
                        living_only: false,
                        exclude_self: false,
                    },
                    "Choose a player. You learn their character.",
                );
                StepOutcome::Waiting
            }
        }
    }

    fn set_pending(&mut self, step: NightStep, seat: SeatId, schema: ChoiceSchema, prompt: &str) {
        self.private_inboxes.push(
            seat,
            PrivateMessage::NightPrompt {
                text: prompt.to_string(),
            },
        );
        self.pending_night = Some(PendingWake {
            step,
            seat,
            schema,
            prompt: prompt.to_string(),
        });
    }

    fn advance_cursor(&mut self) {
        self.night_cursor += 1;
        match &mut self.phase {
            Phase::FirstNight { cursor } => *cursor = self.night_cursor,
            Phase::Night { cursor, .. } => *cursor = self.night_cursor,
            _ => {}
        }
    }

    fn push_minion_briefings(&mut self) {
        let demon: Vec<(SeatId, String)> = self
            .seats
            .iter()
            .filter(|s| {
                s.true_character
                    .is_some_and(|c| c.character_type() == CharacterType::Demon)
            })
            .map(|s| (s.id, s.display_name.clone()))
            .collect();
        let minions: Vec<(SeatId, String)> = self
            .seats
            .iter()
            .filter(|s| {
                s.true_character
                    .is_some_and(|c| c.character_type() == CharacterType::Minion)
            })
            .map(|s| (s.id, s.display_name.clone()))
            .collect();

        let demon_text = if demon.is_empty() {
            "There is no Demon (error).".to_string()
        } else {
            demon
                .iter()
                .map(|(id, name)| format!("{name} (seat {})", id.0))
                .collect::<Vec<_>>()
                .join(", ")
        };

        for (mid, _) in &minions {
            let others: Vec<String> = minions
                .iter()
                .filter(|(id, _)| id != mid)
                .map(|(id, name)| format!("{name} (seat {})", id.0))
                .collect();
            let fellows = if others.is_empty() {
                "none".to_string()
            } else {
                others.join(", ")
            };
            let text = format!("The Demon is {demon_text}. Fellow Minions: {fellows}.");
            self.private_inboxes
                .push(*mid, PrivateMessage::EvilBriefing { text });
        }
    }

    fn push_demon_briefing(&mut self) {
        let minions: Vec<String> = self
            .seats
            .iter()
            .filter(|s| {
                s.true_character
                    .is_some_and(|c| c.character_type() == CharacterType::Minion)
            })
            .map(|s| format!("{} (seat {})", s.display_name, s.id.0))
            .collect();
        let minion_text = if minions.is_empty() {
            "none".to_string()
        } else {
            minions.join(", ")
        };
        let bluffs: Vec<String> = self
            .demon_bluffs
            .iter()
            .map(|c| c.display_name().to_string())
            .collect();
        let bluff_text = if bluffs.is_empty() {
            "none".to_string()
        } else {
            bluffs.join(", ")
        };
        let text =
            format!("Your Minions: {minion_text}. Not-in-play bluffs: {bluff_text}.");

        for s in &self.seats {
            if s.true_character
                .is_some_and(|c| c.character_type() == CharacterType::Demon)
            {
                self.private_inboxes
                    .push(s.id, PrivateMessage::EvilBriefing { text: text.clone() });
            }
        }
    }

    fn resolve_dawn(&mut self) {
        for seat in &mut self.seats {
            seat.monk_protected_tonight = false;
        }

        let deaths = self.deaths_tonight.clone();
        if deaths.is_empty() {
            self.st_announce("Dawn. Nobody died.");
            self.public_log
                .push(PublicEvent::DiedInNight { seats: vec![] });
        } else {
            let names: Vec<String> = deaths
                .iter()
                .filter_map(|id| {
                    self.seats
                        .iter()
                        .find(|s| s.id == *id)
                        .map(|s| s.display_name.clone())
                })
                .collect();
            self.st_announce(format!("Dawn. Died in the night: {}.", names.join(", ")));
            self.public_log
                .push(PublicEvent::DiedInNight { seats: deaths });
        }
        self.deaths_tonight.clear();

        let day = match self.phase {
            Phase::FirstNight { .. } => 1u32,
            Phase::Night { night, .. } => night,
            _ => 1,
        };
        self.phase = Phase::Day {
            day,
            stage: DayStage::Discussion,
        };
        self.night_queue.clear();
        self.night_cursor = 0;
        self.pending_night = None;
        self.public_log.push(PublicEvent::PhaseChanged {
            summary: format!("Day {day} — Discussion"),
        });
    }

    fn validate_payload(
        &self,
        pending: &PendingWake,
        payload: &NightActionPayload,
    ) -> Result<(), GameError> {
        match (&pending.schema, payload) {
            (ChoiceSchema::Ack, NightActionPayload::Ack) => Ok(()),
            (
                ChoiceSchema::PickOne {
                    any_seat,
                    living_only,
                    exclude_self,
                },
                NightActionPayload::PickOne { target },
            ) => {
                let seat = self
                    .seats
                    .iter()
                    .find(|s| s.id == *target)
                    .ok_or(GameError::NoSuchSeat)?;
                if *living_only && !seat.alive {
                    return Err(GameError::IllegalAction("target must be living"));
                }
                if *exclude_self && *target == pending.seat {
                    return Err(GameError::IllegalAction("cannot target self"));
                }
                let _ = any_seat; // any_seat true means dead allowed (already handled via living_only)
                Ok(())
            }
            (ChoiceSchema::PickTwo { .. }, NightActionPayload::PickTwo { a, b }) => {
                if !self.seats.iter().any(|s| s.id == *a) || !self.seats.iter().any(|s| s.id == *b)
                {
                    return Err(GameError::NoSuchSeat);
                }
                Ok(())
            }
            _ => Err(GameError::WrongPayload),
        }
    }

    /// Stub resolve: Poisoner applies poison; other choices only ack and advance (Task 8–9).
    fn resolve_pending_stub(
        &mut self,
        pending: &PendingWake,
        payload: &NightActionPayload,
    ) -> Result<(), GameError> {
        match pending.step {
            NightStep::Poisoner { seat: _ } => {
                if let NightActionPayload::PickOne { target } = payload {
                    // Clear prior poison; apply if ability not disabled.
                    let disabled = self
                        .seats
                        .iter()
                        .find(|s| s.id == pending.seat)
                        .map(|s| s.poisoned || s.is_drunk_outsider)
                        .unwrap_or(false);
                    for s in &mut self.seats {
                        s.poisoned = false;
                    }
                    if !disabled {
                        if let Some(t) = self.seats.iter_mut().find(|s| s.id == *target) {
                            t.poisoned = true;
                        }
                    }
                }
                Ok(())
            }
            NightStep::Butler { .. } => {
                if let NightActionPayload::PickOne { target } = payload {
                    if let Some(s) = self.seats.iter_mut().find(|s| s.id == pending.seat) {
                        s.butler_master = Some(*target);
                    }
                }
                Ok(())
            }
            NightStep::Monk { .. } => {
                if let NightActionPayload::PickOne { target } = payload {
                    let disabled = self
                        .seats
                        .iter()
                        .find(|s| s.id == pending.seat)
                        .map(|s| s.poisoned || s.is_drunk_outsider)
                        .unwrap_or(false);
                    if !disabled {
                        if let Some(t) = self.seats.iter_mut().find(|s| s.id == *target) {
                            t.monk_protected_tonight = true;
                        }
                    }
                }
                Ok(())
            }
            // Full ability resolve is Task 8–9; accepting the choice is enough to advance.
            _ => Ok(()),
        }
    }

    fn default_payload_for(
        &self,
        pending: &PendingWake,
    ) -> Result<NightActionPayload, GameError> {
        match &pending.schema {
            ChoiceSchema::Ack => Ok(NightActionPayload::Ack),
            ChoiceSchema::PickOne {
                living_only,
                exclude_self,
                ..
            } => {
                let target = self
                    .seats
                    .iter()
                    .find(|s| {
                        if *living_only && !s.alive {
                            return false;
                        }
                        if *exclude_self && s.id == pending.seat {
                            return false;
                        }
                        true
                    })
                    .map(|s| s.id)
                    .or_else(|| self.seats.first().map(|s| s.id))
                    .ok_or(GameError::IllegalAction("no legal default target"))?;
                Ok(NightActionPayload::PickOne { target })
            }
            ChoiceSchema::PickTwo { .. } => {
                let a = self
                    .seats
                    .first()
                    .map(|s| s.id)
                    .ok_or(GameError::IllegalAction("no seats"))?;
                let b = self
                    .seats
                    .get(1)
                    .map(|s| s.id)
                    .unwrap_or(a);
                Ok(NightActionPayload::PickTwo { a, b })
            }
        }
    }
}

enum StepOutcome {
    Advanced,
    Waiting,
    DawnDone,
}

fn step_label(step: NightStep) -> &'static str {
    match step {
        NightStep::SetupMarkers => "Setup",
        NightStep::MinionBriefing => "Minion briefing",
        NightStep::DemonBriefing => "Demon briefing",
        NightStep::Poisoner { .. } => "Poisoner",
        NightStep::Spy { .. } => "Spy",
        NightStep::Washerwoman { .. } => "Washerwoman",
        NightStep::Librarian { .. } => "Librarian",
        NightStep::Investigator { .. } => "Investigator",
        NightStep::Chef { .. } => "Chef",
        NightStep::Empath { .. } => "Empath",
        NightStep::FortuneTeller { .. } => "Fortune Teller",
        NightStep::Butler { .. } => "Butler",
        NightStep::Monk { .. } => "Monk",
        NightStep::DemonKill { .. } => "Demon",
        NightStep::Ravenkeeper { .. } => "Ravenkeeper",
        NightStep::Undertaker { .. } => "Undertaker",
        NightStep::Dawn => "Dawn",
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    use crate::game::{RoleAssignment, StartOpts};
    use crate::roles::Character;

    fn tiny_game() -> Game {
        let names = vec![
            "A".into(),
            "B".into(),
            "C".into(),
            "D".into(),
            "E".into(),
        ];
        let lobby = Game::create(names, 1).unwrap();
        let host = lobby.host_token.clone();
        let mut g = lobby.game;
        g.start_game(
            &host,
            StartOpts {
                assignments: Some(vec![
                    RoleAssignment::drunk(SeatId(0), Character::FortuneTeller).unwrap(),
                    RoleAssignment::normal(SeatId(1), Character::Imp),
                    RoleAssignment::normal(SeatId(2), Character::Poisoner),
                    RoleAssignment::normal(SeatId(3), Character::Butler),
                    RoleAssignment::normal(SeatId(4), Character::Spy),
                ]),
            },
        )
        .unwrap();
        g
    }

    #[test]
    fn drunk_fortune_teller_face_wakes_as_ft() {
        let g = tiny_game();
        // Queue was built at start; rebuild for assertion (same grimoire).
        let q = build_first_night_queue(&g);
        assert!(q
            .iter()
            .any(|s| matches!(s, NightStep::FortuneTeller { seat: SeatId(0) })));
        assert!(q
            .iter()
            .any(|s| matches!(s, NightStep::Butler { seat: SeatId(3) })));
        assert!(q
            .iter()
            .any(|s| matches!(s, NightStep::Spy { seat: SeatId(4) })));
    }

    #[test]
    fn start_game_ticks_to_poisoner_pending() {
        let g = tiny_game();
        let p = g.pending_night.as_ref().expect("pending after start");
        assert!(matches!(p.step, NightStep::Poisoner { seat: SeatId(2) }));
        assert_eq!(p.seat, SeatId(2));
    }
}
