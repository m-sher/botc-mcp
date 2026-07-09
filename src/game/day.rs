//! Day machine: nominations, voting, Virgin, Butler, ghost votes (design §10).

use crate::auth::{Actor, Token};
use crate::comms::PublicEvent;
use crate::error::GameError;
use crate::game::ids::SeatId;
use crate::game::phase::{DayStage, EndReason, Phase, Winner};
use crate::game::state::Game;
use crate::game::win::{apply_demon_death, end_game, living_count as win_living_count, win_check};
use crate::roles::{Character, CharacterType};

/// In-progress nomination with open vote window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenNomination {
    pub by: SeatId,
    pub target: SeatId,
    /// Explicit votes cast so far (`true` = support / yes).
    pub votes: Vec<(SeatId, bool)>,
}

/// Closed nomination record for the day (leader comparison).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedNomination {
    pub by: SeatId,
    pub target: SeatId,
    pub yes_votes: u32,
}

/// `yes * 2 >= living` (half rounded up for odd; half for even).
pub fn meets_threshold(yes: u32, living: u32) -> bool {
    if living == 0 {
        return false;
    }
    yes * 2 >= living
}

/// Clear per-day nomination / vote tracking (call at dawn).
pub fn reset_day_vote_state(game: &mut Game) {
    game.day_nominators.clear();
    game.day_nominees.clear();
    game.current_nomination = None;
    game.closed_nominations.clear();
}

fn require_not_ended(game: &Game) -> Result<(), GameError> {
    if game.winner.is_some() || matches!(game.phase, Phase::Ended { .. }) {
        return Err(GameError::GameEnded);
    }
    Ok(())
}

fn day_stage(game: &Game) -> Result<(u32, DayStage), GameError> {
    match game.phase {
        Phase::Day { day, stage } => Ok((day, stage)),
        _ => Err(GameError::WrongPhase),
    }
}

fn living_count(game: &Game) -> u32 {
    win_living_count(game)
}

fn seat_ref(game: &Game, id: SeatId) -> Result<&crate::game::seat::Seat, GameError> {
    game.seats
        .iter()
        .find(|s| s.id == id)
        .ok_or(GameError::NoSuchSeat)
}

fn seat_mut(game: &mut Game, id: SeatId) -> Result<&mut crate::game::seat::Seat, GameError> {
    game.seats
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or(GameError::NoSuchSeat)
}

/// Host: Discussion → Nominations.
pub fn open_nominations(game: &mut Game, host: &Token) -> Result<(), GameError> {
    match game.tokens.resolve(host) {
        Some(Actor::Host) => {}
        _ => return Err(GameError::Unauthorized),
    }
    require_not_ended(game)?;
    let (day, stage) = day_stage(game)?;
    if stage != DayStage::Discussion {
        return Err(GameError::WrongPhase);
    }
    if game.current_nomination.is_some() {
        return Err(GameError::IllegalAction("vote already open"));
    }
    game.phase = Phase::Day {
        day,
        stage: DayStage::Nominations,
    };
    game.st_announce("Nominations are open.");
    game.public_log.push(PublicEvent::PhaseChanged {
        summary: format!("Day {day} — Nominations"),
    });
    Ok(())
}

/// Player: open a nomination and start the vote window (or Virgin bounce).
pub fn nominate(game: &mut Game, by: SeatId, target: SeatId) -> Result<(), GameError> {
    require_not_ended(game)?;
    let (_day, stage) = day_stage(game)?;
    if stage != DayStage::Nominations {
        return Err(GameError::WrongPhase);
    }
    if game.executed_today.is_some() {
        return Err(GameError::IllegalAction("already executed today"));
    }
    if game.current_nomination.is_some() {
        return Err(GameError::IllegalAction("vote in progress"));
    }
    if by == target {
        return Err(GameError::IllegalAction("cannot nominate yourself"));
    }

    let nominator = seat_ref(game, by)?;
    if !nominator.alive {
        return Err(GameError::IllegalAction("dead players cannot nominate"));
    }
    if game.day_nominators.contains(&by) {
        return Err(GameError::IllegalAction("already nominated today"));
    }
    let nominee = seat_ref(game, target)?;
    if !nominee.alive {
        return Err(GameError::IllegalAction("cannot nominate the dead"));
    }
    if game.day_nominees.contains(&target) {
        return Err(GameError::IllegalAction("target already nominated today"));
    }

    // Snapshot virgin / type before mutating.
    let virgin_active = nominee.true_character == Some(Character::Virgin)
        && !nominee.virgin_ability_used
        && !nominee.ability_disabled();
    // Spec §10: Virgin checks true character type of nominator (Drunk is Outsider).
    let nominator_is_townsfolk = nominator
        .true_character
        .is_some_and(|c| c.character_type() == CharacterType::Townsfolk);

    game.day_nominators.push(by);
    game.day_nominees.push(target);
    game.public_log
        .push(PublicEvent::Nominated { by, target });

    if virgin_active {
        if let Ok(s) = seat_mut(game, target) {
            s.virgin_ability_used = true;
        }
        if nominator_is_townsfolk {
            // Immediate execution of nominator — day's execution, no vote.
            game.st_announce("The Virgin's power triggers. The nominator is executed.");
            resolve_execution(game, by);
            // Day's execution is done; nominations effectively closed for further executions.
            return Ok(());
        }
        // Non-Townsfolk nominator: ability spent, vote proceeds.
    }

    game.current_nomination = Some(OpenNomination {
        by,
        target,
        votes: Vec::new(),
    });
    Ok(())
}

/// Player: cast yes/no on the current open nomination.
pub fn vote(game: &mut Game, seat: SeatId, nominee: SeatId, support: bool) -> Result<(), GameError> {
    require_not_ended(game)?;
    let (_day, stage) = day_stage(game)?;
    if stage != DayStage::Nominations {
        return Err(GameError::WrongPhase);
    }
    let open = game
        .current_nomination
        .as_ref()
        .ok_or(GameError::IllegalAction("no open nomination"))?;
    if open.target != nominee {
        return Err(GameError::IllegalAction("nominee is not current nomination"));
    }
    if open.votes.iter().any(|(s, _)| *s == seat) {
        return Err(GameError::IllegalAction("already voted on this nomination"));
    }

    let voter = seat_ref(game, seat)?;
    let alive = voter.alive;
    let ghost_ok = voter.ghost_vote_available;
    let is_butler = voter.true_character == Some(Character::Butler);
    let butler_disabled = voter.ability_disabled();
    let butler_master = voter.butler_master;

    if !alive {
        // Dead: may vote; yes spends the single ghost vote token.
        if support && !ghost_ok {
            return Err(GameError::IllegalAction("ghost vote already spent"));
        }
    }

    if support && is_butler && alive && !butler_disabled {
        // Butler may only vote yes if master has already voted yes on this nom.
        let master_yes = match butler_master {
            Some(m) => open
                .votes
                .iter()
                .any(|(s, yes)| *s == m && *yes),
            None => false,
        };
        if !master_yes {
            return Err(GameError::IllegalAction(
                "Butler may only vote yes if master has voted yes",
            ));
        }
    }

    // Apply ghost spend on yes.
    if !alive && support {
        seat_mut(game, seat)?.ghost_vote_available = false;
    }

    game.public_log.push(PublicEvent::VoteCast {
        seat,
        nominee,
        support,
    });
    if let Some(open) = game.current_nomination.as_mut() {
        open.votes.push((seat, support));
    }

    // Auto-close when every living seat has cast a vote.
    if all_living_have_voted(game) {
        close_vote_inner(game)?;
    }
    Ok(())
}

fn all_living_have_voted(game: &Game) -> bool {
    let Some(open) = game.current_nomination.as_ref() else {
        return false;
    };
    game.seats
        .iter()
        .filter(|s| s.alive)
        .all(|s| open.votes.iter().any(|(id, _)| *id == s.id))
}

/// Host: finalize the current nomination's tally without executing.
pub fn close_vote(game: &mut Game, host: &Token) -> Result<(), GameError> {
    match game.tokens.resolve(host) {
        Some(Actor::Host) => {}
        _ => return Err(GameError::Unauthorized),
    }
    require_not_ended(game)?;
    let (_day, stage) = day_stage(game)?;
    if stage != DayStage::Nominations {
        return Err(GameError::WrongPhase);
    }
    close_vote_inner(game)
}

fn close_vote_inner(game: &mut Game) -> Result<(), GameError> {
    let open = game
        .current_nomination
        .take()
        .ok_or(GameError::IllegalAction("no open nomination"))?;
    let yes = open.votes.iter().filter(|(_, y)| *y).count() as u32;
    // Missing votes count as no (already only counting explicit yes).
    game.closed_nominations.push(ClosedNomination {
        by: open.by,
        target: open.target,
        yes_votes: yes,
    });
    game.st_announce(format!(
        "Votes closed on seat {}: {yes} yes vote(s).",
        open.target.0
    ));
    Ok(())
}

/// Current execution candidate: threshold met and strictly highest yes total today.
pub fn execution_leader(game: &Game) -> Option<SeatId> {
    let living = living_count(game);
    let mut best: Option<(SeatId, u32)> = None;
    let mut tie = false;
    for c in &game.closed_nominations {
        if !meets_threshold(c.yes_votes, living) {
            continue;
        }
        match best {
            None => {
                best = Some((c.target, c.yes_votes));
                tie = false;
            }
            Some((_, byes)) if c.yes_votes > byes => {
                best = Some((c.target, c.yes_votes));
                tie = false;
            }
            Some((_, byes)) if c.yes_votes == byes => {
                tie = true;
            }
            _ => {}
        }
    }
    if tie {
        None
    } else {
        best.map(|(s, _)| s)
    }
}

/// Host: execute the leader (if any), then begin the next night if the game continues.
pub fn end_nominations(game: &mut Game, host: &Token) -> Result<(), GameError> {
    match game.tokens.resolve(host) {
        Some(Actor::Host) => {}
        _ => return Err(GameError::Unauthorized),
    }
    require_not_ended(game)?;
    let (day, stage) = day_stage(game)?;
    if stage != DayStage::Nominations {
        return Err(GameError::WrongPhase);
    }
    // Close any dangling open vote as no majority progress (missing = no).
    if game.current_nomination.is_some() {
        close_vote_inner(game)?;
    }

    if game.executed_today.is_none() {
        if let Some(leader) = execution_leader(game) {
            resolve_execution(game, leader);
        } else {
            game.public_log.push(PublicEvent::NoExecution);
            game.st_announce("No execution today.");
            // Mayor: 3 living + no execution → Good (design §10 / docs/win-conditions).
            if mayor_three_no_exec_wins(game) {
                end_game(game, Winner::Good, EndReason::MayorThreeNoExec);
            } else {
                win_check(game);
            }
        }
    }

    if game.winner.is_some() || matches!(game.phase, Phase::Ended { .. }) {
        return Ok(());
    }

    // Auto-path: next night is day + 1 (Day 1 → Night 2).
    let next_night = day + 1;
    game.enter_night(next_night);
    Ok(())
}

/// True when living==3 and a living Mayor has an active (non-disabled) ability.
fn mayor_three_no_exec_wins(game: &Game) -> bool {
    if living_count(game) != 3 {
        return false;
    }
    game.seats.iter().any(|s| {
        s.alive && s.true_character == Some(Character::Mayor) && !s.ability_disabled()
    })
}

/// Mark seat dead via execution; Saint → Evil; Imp → SW / demon death; then [`win_check`].
pub fn resolve_execution(game: &mut Game, seat: SeatId) {
    let alive_before = living_count(game);
    let (true_char, disabled) = game
        .seats
        .iter()
        .find(|s| s.id == seat)
        .map(|s| (s.true_character, s.ability_disabled()))
        .unwrap_or((None, true));

    if let Some(s) = game.seats.iter_mut().find(|s| s.id == seat) {
        if s.alive {
            s.alive = false;
            // Dead seats keep / gain one ghost vote for the rest of the game.
            s.ghost_vote_available = true;
        }
    }
    game.executed_today = Some(seat);
    game.public_log.push(PublicEvent::Executed { seat });
    // Also surface as immediate day death for consumers that listen for PlayerDied.
    game.public_log.push(PublicEvent::PlayerDied { seat });

    // Saint: execution causes Evil win if ability is active (not poisoned/drunk).
    if true_char == Some(Character::Saint) && !disabled {
        end_game(game, Winner::Evil, EndReason::SaintExecuted);
        return;
    }

    if true_char == Some(Character::Imp) {
        apply_demon_death(game, seat, alive_before);
    }

    win_check(game);
}

/// Slayer once-per-game day action (design §10.3).
///
/// Allowed in Day Discussion or Nominations. Spends `slayer_used` even on miss / disabled.
/// Hit: target is true Imp and Slayer ability is active → immediate death + demon death / win check.
pub fn day_action_slay(game: &mut Game, slayer: SeatId, target: SeatId) -> Result<(), GameError> {
    require_not_ended(game)?;
    let (_day, _stage) = day_stage(game)?;
    // Discussion or Nominations both ok (design §10.3).

    let slayer_seat = seat_ref(game, slayer)?;
    if !slayer_seat.alive {
        return Err(GameError::IllegalAction("dead players cannot slay"));
    }
    // Allow attempt if player-facing role is Slayer (Drunk face) or true Slayer.
    let facing = slayer_seat.visible_character();
    if facing != Some(Character::Slayer) {
        return Err(GameError::IllegalAction("not the Slayer"));
    }
    if slayer_seat.slayer_used {
        return Err(GameError::IllegalAction("Slayer ability already used"));
    }

    // Snapshot before mut.
    let disabled = slayer_seat.ability_disabled();
    let true_is_slayer = slayer_seat.true_character == Some(Character::Slayer);

    seat_mut(game, slayer)?.slayer_used = true;

    // Miss path: disabled, not true Slayer, wrong target, or dead target.
    let target_seat = seat_ref(game, target)?;
    let hit = true_is_slayer
        && !disabled
        && target_seat.alive
        && target_seat.true_character == Some(Character::Imp);

    if !hit {
        // Silent miss (no public confirmation).
        return Ok(());
    }

    let alive_before = living_count(game);
    if let Some(s) = game.seats.iter_mut().find(|s| s.id == target) {
        s.alive = false;
        s.ghost_vote_available = true;
    }
    game.public_log.push(PublicEvent::PlayerDied { seat: target });
    game.st_announce(format!("Seat {} dies.", target.0));
    apply_demon_death(game, target, alive_before);
    win_check(game);
    Ok(())
}

impl Game {
    pub fn open_nominations(&mut self, host: &Token) -> Result<(), GameError> {
        open_nominations(self, host)
    }

    pub fn nominate(&mut self, by: SeatId, target: SeatId) -> Result<(), GameError> {
        nominate(self, by, target)
    }

    pub fn vote(&mut self, seat: SeatId, nominee: SeatId, support: bool) -> Result<(), GameError> {
        vote(self, seat, nominee, support)
    }

    pub fn close_vote(&mut self, host: &Token) -> Result<(), GameError> {
        close_vote(self, host)
    }

    pub fn end_nominations(&mut self, host: &Token) -> Result<(), GameError> {
        end_nominations(self, host)
    }

    pub fn day_action_slay(&mut self, slayer: SeatId, target: SeatId) -> Result<(), GameError> {
        day_action_slay(self, slayer, target)
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn threshold_six_needs_three() {
        assert!(!meets_threshold(2, 6));
        assert!(meets_threshold(3, 6));
        assert!(meets_threshold(4, 6));
    }

    #[test]
    fn threshold_five_needs_three() {
        assert!(!meets_threshold(2, 5));
        assert!(meets_threshold(3, 5));
    }
}
