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
        // Wake *all* living seats whose true/face character matches (Drunk face + real).
        for seat in find_wake_seats(game, slot.character(), slot.uses_true_character()) {
            q.push(first_night_step(*slot, seat));
        }
    }

    q.push(NightStep::Dawn);
    q
}

/// Build the ordered other-night step list (no N1 setup/briefings; includes Imp kill).
///
/// Ravenkeeper wakes are **not** pre-queued here: they are inserted after the demon kill
/// via `die_from_demon` when a player-facing Ravenkeeper dies. Undertaker is included when
/// the seat is alive **and** there was an execution today (`executed_today`).
pub fn build_other_night_queue(game: &Game) -> Vec<NightStep> {
    let mut q = Vec::new();

    for slot in OTHER_NIGHT_CHARACTER_ORDER {
        match slot {
            OtherNightSlot::Ravenkeeper => {
                // Intentionally empty: death-wake is inserted by die_from_demon only.
            }
            OtherNightSlot::Undertaker => {
                if game.executed_today.is_some() {
                    for seat in find_wake_seats(game, Character::Undertaker, false /* face */) {
                        q.push(NightStep::Undertaker { seat });
                    }
                }
            }
            OtherNightSlot::Imp => {
                for seat in find_wake_seats(game, Character::Imp, true) {
                    q.push(NightStep::DemonKill { seat });
                }
            }
            other => {
                let role = other.character();
                for seat in find_wake_seats(game, role, other.uses_true_character()) {
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

/// All living seats matching role via true character or player-facing character.
///
/// Info Townsfolk (and similar face roles) must wake every seat that presents as that role —
/// e.g. a real Empath and a Drunk with Empath face both get an Empath step.
fn find_wake_seats(game: &Game, role: Character, use_true: bool) -> Vec<SeatId> {
    game.seats
        .iter()
        .filter(|s| s.alive && seat_matches_wake(game, s.id, role, use_true))
        .map(|s| s.id)
        .collect()
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
            if self.pending_night.is_some() || self.pending_host.is_some() {
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

    /// Host applies a documented default for the pending wake (or pending host decision) and continues.
    pub fn skip_night_action(&mut self, host: &Token) -> Result<(), GameError> {
        match self.tokens.resolve(host) {
            Some(Actor::Host) => {}
            _ => return Err(GameError::Unauthorized),
        }
        // Host decisions (Mayor / starpass) take priority when set.
        if self.pending_host.is_some() {
            return self.apply_default_host_decision();
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
        let needs_host = self.resolve_pending_action(&pending, &payload)?;

        self.pending_night = None;
        if needs_host {
            // Cursor stays on DemonKill until host_decide / skip resolves pending_host.
            return Ok(());
        }
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
                dawn(self);
                StepOutcome::DawnDone
            }
            NightStep::Spy { .. }
            | NightStep::Washerwoman { .. }
            | NightStep::Librarian { .. }
            | NightStep::Investigator { .. }
            | NightStep::Chef { .. }
            | NightStep::Empath { .. }
            | NightStep::Undertaker { .. } => {
                let _ = crate::game::ability::resolve_night_step(self, step, None);
                self.advance_cursor();
                StepOutcome::Advanced
            }
            // Choice-required wakes: set pending and stop.
            NightStep::Poisoner { seat } => {
                // §9.4: clear previous poison at the start of the Poisoner step.
                crate::game::ability::clear_poisons(self);
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

    /// Enter other-night `night` (must be ≥ 2): build queue and tick to first pending.
    pub fn enter_night(&mut self, night: u32) {
        self.deaths_tonight.clear();
        self.pending_night = None;
        self.pending_host = None;
        // Host-authored lies are scoped to a single night.
        self.host_lie_queue.clear();
        self.night_queue = build_other_night_queue(self);
        self.night_cursor = 0;
        self.phase = Phase::Night { night, cursor: 0 };
        self.st_announce(format!("Night falls. Night {night} begins."));
        self.night_tick();
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
                if a == b {
                    return Err(GameError::IllegalAction(
                        "Fortune Teller must pick two different seats",
                    ));
                }
                Ok(())
            }
            _ => Err(GameError::WrongPayload),
        }
    }

    /// Resolve a submitted night choice (poison/monk/imp via ability; info via Task 8).
    ///
    /// Returns `true` when a host decision is now pending (cursor must not advance).
    fn resolve_pending_action(
        &mut self,
        pending: &PendingWake,
        payload: &NightActionPayload,
    ) -> Result<bool, GameError> {
        match pending.step {
            NightStep::Poisoner { seat: _ } => {
                if let NightActionPayload::PickOne { target } = payload {
                    let disabled = self
                        .seats
                        .iter()
                        .find(|s| s.id == pending.seat)
                        .map(|s| s.ability_disabled())
                        .unwrap_or(false);
                    // Prior poison already cleared at begin_step.
                    if !disabled {
                        crate::game::ability::apply_poison(self, *target);
                    }
                }
                Ok(false)
            }
            NightStep::Monk { .. } => {
                if let NightActionPayload::PickOne { target } = payload {
                    let disabled = self
                        .seats
                        .iter()
                        .find(|s| s.id == pending.seat)
                        .map(|s| s.ability_disabled())
                        .unwrap_or(false);
                    if !disabled {
                        crate::game::ability::protect::apply_monk_protect(self, *target);
                    }
                }
                Ok(false)
            }
            NightStep::DemonKill { seat: demon } => {
                if let NightActionPayload::PickOne { target } = payload {
                    let result = crate::game::ability::try_demon_kill(self, demon, *target);
                    return Ok(matches!(
                        result,
                        crate::game::ability::KillResult::NeedsHost
                    ));
                }
                Ok(false)
            }
            // Info / Butler / Ravenkeeper (Task 8).
            step => {
                crate::game::ability::resolve_night_step(self, step, Some(payload))?;
                Ok(false)
            }
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

/// Dawn: clear monk protect, announce deaths (names only), enter Day Discussion (§9.7).
pub fn dawn(game: &mut Game) {
    crate::game::ability::protect::clear_monk_protection(game);

    let deaths = game.deaths_tonight.clone();
    if deaths.is_empty() {
        game.st_announce("Dawn. Nobody died.");
        game.public_log
            .push(PublicEvent::DiedInNight { seats: vec![] });
    } else {
        let names: Vec<String> = deaths
            .iter()
            .filter_map(|id| {
                game.seats
                    .iter()
                    .find(|s| s.id == *id)
                    .map(|s| s.display_name.clone())
            })
            .collect();
        game.st_announce(format!("Dawn. Died in the night: {}.", names.join(", ")));
        game.public_log
            .push(PublicEvent::DiedInNight { seats: deaths });
    }
    game.deaths_tonight.clear();
    // Unused host lies do not carry into the next day/night.
    game.host_lie_queue.clear();

    let day = match game.phase {
        Phase::FirstNight { .. } => 1u32,
        Phase::Night { night, .. } => night,
        _ => 1,
    };
    game.phase = Phase::Day {
        day,
        stage: DayStage::Discussion,
    };
    game.night_queue.clear();
    game.night_cursor = 0;
    game.pending_night = None;
    game.pending_host = None;
    // Undertaker already ran this night; clear execution marker for the new day.
    game.executed_today = None;
    crate::game::day::reset_day_vote_state(game);
    game.public_log.push(PublicEvent::PhaseChanged {
        summary: format!("Day {day} — Discussion"),
    });
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
                ..Default::default()
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
