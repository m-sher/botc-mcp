//! Day machine: nominations, voting, Virgin, Butler, ghost votes (design §10).

use crate::auth::{Actor, Token};
use crate::comms::PublicEvent;
use crate::error::GameError;
use crate::game::ids::SeatId;
use crate::game::phase::{DayStage, EndReason, Phase, Winner};
use crate::game::state::Game;
use crate::game::win::{apply_demon_death, end_game, living_count as win_living_count, win_check};
use crate::roles::Character;

/// In-progress nomination with open vote window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenNomination {
    pub by: SeatId,
    pub target: SeatId,
    /// Explicit votes cast so far (`true` = support / yes).
    pub votes: Vec<(SeatId, bool)>,
    /// Dead seats that passed (abstain without spending ghost vote).
    pub passes: Vec<SeatId>,
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
    game.require_no_pending_host()?;
    open_nominations_inner(game)
}

/// Discussion → Nominations (no host auth; used by host tool and first nominate auto-open).
fn open_nominations_inner(game: &mut Game) -> Result<(), GameError> {
    require_not_ended(game)?;
    game.require_no_pending_host()?;
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

/// Record a nomination on the public day trackers (after ST registration is known).
fn commit_nomination_public(game: &mut Game, by: SeatId, target: SeatId) {
    if !game.day_nominators.contains(&by) {
        game.day_nominators.push(by);
    }
    if !game.day_nominees.contains(&target) {
        game.day_nominees.push(target);
    }
    game.public_log.push(PublicEvent::Nominated { by, target });
}

/// Open the vote window and try the nominator's automatic yes.
///
/// Delegates to [`vote`] so Butler / ghost / double-vote legality stays in one place.
/// A living Butler whose master has not yet voted yes cannot auto-yes yet — they simply
/// take a normal Vote turn later under the master rule. Disabled Butler (poisoned/drunk)
/// may auto-yes freely, matching `vote()`.
fn open_nomination_with_nominator_yes(game: &mut Game, by: SeatId, target: SeatId) {
    game.current_nomination = Some(OpenNomination {
        by,
        target,
        votes: Vec::new(),
        passes: Vec::new(),
    });
    // Ignore Err: e.g. Butler without master yes — nominator remains pending voter.
    let _ = vote(game, by, target, true);
}

/// Player: open a nomination and start the vote window (or Virgin bounce).
///
/// Allowed from Day Nominations, or from Discussion (auto-opens nominations first).
///
/// All legality checks run **before** auto-opening Nominations from Discussion so a
/// rejected nominate never mutates phase or the public log.
///
/// On success (when a vote window opens), the nominator is recorded as an automatic
/// **yes** — they do not need to call `vote` for their own nomination.
pub fn nominate(game: &mut Game, by: SeatId, target: SeatId) -> Result<(), GameError> {
    require_not_ended(game)?;
    game.require_no_pending_host()?;
    let (_day, stage) = day_stage(game)?;
    match stage {
        DayStage::Nominations | DayStage::Discussion => {}
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
    // Snapshot nominee fields before any mut borrow (auto-open / Virgin).
    let (nominee_alive, is_virgin_first_nom, virgin_disabled) = {
        let nominee = seat_ref(game, target)?;
        (
            nominee.alive,
            nominee.true_character == Some(Character::Virgin) && !nominee.virgin_ability_used,
            nominee.ability_disabled(),
        )
    };
    if !nominee_alive {
        return Err(GameError::IllegalAction("cannot nominate the dead"));
    }
    if game.day_nominees.contains(&target) {
        return Err(GameError::IllegalAction("target already nominated today"));
    }

    // Auto-open only after the nomination is known-legal (Discussion → Nominations).
    if stage == DayStage::Discussion {
        open_nominations_inner(game)?;
    }

    // Virgin: first nomination always spends the once-per-game ability, even if poisoned/drunk.
    // Execution only if ability is active AND nominator registers as Townsfolk (Spy may).
    //
    // Spy/Recluse registration for the Virgin is resolved **immediately** via
    // `registration_mode` (never a day-blocking host pause) so the public path is
    // identical to a normal nomination — no limbo leak and no probe channel.
    if is_virgin_first_nom {
        if let Ok(s) = seat_mut(game, target) {
            s.virgin_ability_used = true;
        }
        commit_nomination_public(game, by, target);
        if !virgin_disabled {
            let virgin_label = format!("virgin_reg:day:nom:{}", by.0);
            let nominator_registers_townsfolk =
                crate::game::ability::register::registers_as_townsfolk(game, by, &virgin_label);
            if nominator_registers_townsfolk {
                game.st_announce("The Virgin's power triggers. The nominator is executed.");
                resolve_execution(game, by);
                try_auto_end_day(game)?;
                return Ok(());
            }
        }
        // Disabled, or nominator does not register as Townsfolk: ability spent, vote proceeds.
        open_nomination_with_nominator_yes(game, by, target);
        return Ok(());
    }

    commit_nomination_public(game, by, target);
    open_nomination_with_nominator_yes(game, by, target);
    Ok(())
}

/// Player: cast yes/no on the current open nomination.
pub fn vote(
    game: &mut Game,
    seat: SeatId,
    nominee: SeatId,
    support: bool,
) -> Result<(), GameError> {
    require_not_ended(game)?;
    game.require_no_pending_host()?;
    let (_day, stage) = day_stage(game)?;
    if stage != DayStage::Nominations {
        return Err(GameError::WrongPhase);
    }
    let open = game
        .current_nomination
        .as_ref()
        .ok_or(GameError::IllegalAction("no open nomination"))?;
    if open.target != nominee {
        return Err(GameError::IllegalAction(
            "nominee is not current nomination",
        ));
    }
    // One ballot per seat per open nomination (yes or no). Multiple nominations
    // per day are fine; re-voting the same open nom is not.
    if open.votes.iter().any(|(s, _)| *s == seat) {
        return Err(GameError::IllegalAction(
            "already voted on this nomination (one vote per seat per nomination)",
        ));
    }
    if open.passes.contains(&seat) {
        return Err(GameError::IllegalAction(
            "already passed on this nomination (one response per seat per nomination)",
        ));
    }

    let voter = seat_ref(game, seat)?;
    let alive = voter.alive;
    let ghost_ok = voter.ghost_vote_available;
    let is_butler = voter.true_character == Some(Character::Butler);
    let butler_disabled = voter.ability_disabled();
    let butler_master = voter.butler_master;

    if !alive {
        // Dead with spent ghost: no further votes (yes or no).
        if !ghost_ok {
            return Err(GameError::IllegalAction("ghost vote already spent"));
        }
        // Dead with ghost available: yes spends; no does not spend but records a response.
    }

    if support && is_butler && alive && !butler_disabled {
        // Butler may only vote yes if master has already voted yes on this nom.
        let master_yes = match butler_master {
            Some(m) => open.votes.iter().any(|(s, yes)| *s == m && *yes),
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

    // Auto-close when living all voted and every dead with ghost remaining has voted or passed.
    if nomination_ready_to_auto_close(game) {
        close_vote_inner(game)?;
    }
    Ok(())
}

/// Dead player only: abstain on the open nomination without spending the ghost vote.
///
/// Marks the seat as having responded so auto-close can proceed once all living have voted
/// and every other ghost-holder has voted or passed. Host [`close_vote`] may still force-close.
pub fn pass_vote(game: &mut Game, seat: SeatId) -> Result<(), GameError> {
    require_not_ended(game)?;
    game.require_no_pending_host()?;
    let (_day, stage) = day_stage(game)?;
    if stage != DayStage::Nominations {
        return Err(GameError::WrongPhase);
    }
    let open = game
        .current_nomination
        .as_ref()
        .ok_or(GameError::IllegalAction("no open nomination"))?;
    if open.votes.iter().any(|(s, _)| *s == seat) {
        return Err(GameError::IllegalAction(
            "already voted on this nomination (one vote per seat per nomination)",
        ));
    }
    if open.passes.contains(&seat) {
        return Err(GameError::IllegalAction(
            "already passed on this nomination (one response per seat per nomination)",
        ));
    }

    let voter = seat_ref(game, seat)?;
    if voter.alive {
        return Err(GameError::IllegalAction(
            "only dead players may pass a vote",
        ));
    }
    if !voter.ghost_vote_available {
        return Err(GameError::IllegalAction("ghost vote already spent"));
    }

    // Ghost token is retained; seat is recorded as responded for auto-close.
    if let Some(open) = game.current_nomination.as_mut() {
        open.passes.push(seat);
    }

    if nomination_ready_to_auto_close(game) {
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

fn dead_responded(open: &OpenNomination, seat: SeatId) -> bool {
    open.votes.iter().any(|(id, _)| *id == seat) || open.passes.contains(&seat)
}

/// Living all voted, and every dead seat with remaining ghost vote has voted or [`pass_vote`]d.
/// Dead without ghost vote remaining are skipped. Host [`close_vote`] may still force-close.
fn nomination_ready_to_auto_close(game: &Game) -> bool {
    let Some(open) = game.current_nomination.as_ref() else {
        return false;
    };
    if !all_living_have_voted(game) {
        return false;
    }
    game.seats
        .iter()
        .filter(|s| !s.alive && s.ghost_vote_available)
        .all(|s| dead_responded(open, s.id))
}

/// Host: finalize the current nomination's tally.
///
/// When no further legal nomination remains after the tally, this also runs
/// day end (execution leader / enter night) via [`try_auto_end_day`].
pub fn close_vote(game: &mut Game, host: &Token) -> Result<(), GameError> {
    match game.tokens.resolve(host) {
        Some(Actor::Host) => {}
        _ => return Err(GameError::Unauthorized),
    }
    require_not_ended(game)?;
    game.require_no_pending_host()?;
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
    // When no further legal nomination exists, end the day automatically.
    try_auto_end_day(game)?;
    Ok(())
}

/// True when some living player can still open a legal nomination.
fn any_legal_nomination_remaining(game: &Game) -> bool {
    if game.executed_today.is_some() {
        return false;
    }
    if game.current_nomination.is_some() {
        return true;
    }
    let living: Vec<SeatId> = game
        .seats
        .iter()
        .filter(|s| s.alive)
        .map(|s| s.id)
        .collect();
    for &by in &living {
        if game.day_nominators.contains(&by) {
            continue;
        }
        for &target in &living {
            if target == by {
                continue;
            }
            if game.day_nominees.contains(&target) {
                continue;
            }
            return true;
        }
    }
    false
}

/// End day (execute leader / enter night) when no further nominations are possible.
fn try_auto_end_day(game: &mut Game) -> Result<(), GameError> {
    if game.winner.is_some() || matches!(game.phase, Phase::Ended { .. }) {
        return Ok(());
    }
    // Never auto-end while a Storyteller decision is outstanding.
    if game.pending_host.is_some() {
        return Ok(());
    }
    let Ok((_, stage)) = day_stage(game) else {
        return Ok(());
    };
    if stage != DayStage::Nominations {
        return Ok(());
    }
    if game.current_nomination.is_some() {
        return Ok(());
    }
    if any_legal_nomination_remaining(game) {
        return Ok(());
    }
    end_nominations_inner(game)
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

/// Host: force-end nominations (execute leader if any), then begin the next night.
///
/// Day also auto-ends via [`try_auto_end_day`] when no legal nominations remain.
pub fn end_nominations(game: &mut Game, host: &Token) -> Result<(), GameError> {
    match game.tokens.resolve(host) {
        Some(Actor::Host) => {}
        _ => return Err(GameError::Unauthorized),
    }
    game.require_no_pending_host()?;
    end_nominations_inner(game)
}

/// Execute the vote leader (if any), win checks, enter night. No host auth.
fn end_nominations_inner(game: &mut Game) -> Result<(), GameError> {
    require_not_ended(game)?;
    game.require_no_pending_host()?;
    let (day, stage) = day_stage(game)?;
    if stage != DayStage::Nominations {
        return Err(GameError::WrongPhase);
    }
    // Close any dangling open vote as no majority progress (missing = no).
    // Avoid re-entering try_auto_end_day recursion: close tally only.
    if game.current_nomination.is_some() {
        close_vote_tally_only(game)?;
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
    game.enter_night(next_night)?;
    Ok(())
}

/// Finalize open nomination tally without auto-ending the day.
fn close_vote_tally_only(game: &mut Game) -> Result<(), GameError> {
    let open = game
        .current_nomination
        .take()
        .ok_or(GameError::IllegalAction("no open nomination"))?;
    let yes = open.votes.iter().filter(|(_, y)| *y).count() as u32;
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

/// True when living==3 and a living Mayor has an active (non-disabled) ability.
fn mayor_three_no_exec_wins(game: &Game) -> bool {
    if living_count(game) != 3 {
        return false;
    }
    game.seats
        .iter()
        .any(|s| s.alive && s.true_character == Some(Character::Mayor) && !s.ability_disabled())
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

    // Poison ends when the Poisoner dies (any path).
    if true_char == Some(Character::Poisoner) {
        crate::game::ability::on_poisoner_left_play(game);
    }

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
/// Hit: target is true Imp (or Recluse registering as Demon) and Slayer is active → death + win check.
pub fn day_action_slay(game: &mut Game, slayer: SeatId, target: SeatId) -> Result<(), GameError> {
    require_not_ended(game)?;
    game.require_no_pending_host()?;
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

    // Validate the target BEFORE spending / the disabled short-circuit so a Drunk-face or
    // poisoned Slayer returns exactly the same result (error / silent miss) as a healthy
    // Slayer for a given target — no disabled-status is inferable from an error.
    let target_seat = seat_ref(game, target)?;
    let target_alive = target_seat.alive;
    let target_true = target_seat.true_character;
    let target_disabled = target_seat.ability_disabled();

    seat_mut(game, slayer)?.slayer_used = true;

    if !true_is_slayer || disabled {
        // Silent miss (Drunk face / poisoned / drunk).
        return Ok(());
    }

    if !target_alive {
        return Ok(());
    }

    // True Imp: always a hit (no ST discretion).
    if target_true == Some(Character::Imp) {
        return apply_slayer_kill(game, target, true);
    }

    // Recluse-as-Demon via `registration_mode` immediately (no day-blocking host pause).
    if target_true == Some(Character::Recluse) && !target_disabled {
        let demon_label = format!("slayer_reg:day:{}", target.0);
        if crate::game::ability::register::register_demon_for_ft(game, target, &demon_label) {
            return apply_slayer_kill(game, target, false);
        }
        return Ok(());
    }

    // Silent miss.
    Ok(())
}

fn apply_slayer_kill(game: &mut Game, target: SeatId, was_true_imp: bool) -> Result<(), GameError> {
    let Some(s) = game.seats.iter().find(|s| s.id == target) else {
        return Ok(());
    };
    if !s.alive {
        return Ok(());
    }
    let alive_before = living_count(game);
    if let Some(s) = game.seats.iter_mut().find(|s| s.id == target) {
        s.alive = false;
        s.ghost_vote_available = true;
    }
    game.public_log
        .push(PublicEvent::PlayerDied { seat: target });
    game.st_announce(format!("Seat {} dies.", target.0));
    if was_true_imp {
        apply_demon_death(game, target, alive_before);
    }
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

    pub fn pass_vote(&mut self, seat: SeatId) -> Result<(), GameError> {
        pass_vote(self, seat)
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
