//! Turn-routing scheduler.
//!
//! Trouble Brewing is strictly sequential (one night wake at a time; one
//! nomination/vote at a time), but agents run as independent headless sessions.
//! Fan-ticking everyone each cycle leaves most agents with no legal action, so
//! they idle and probe tools. This planner reads the engine state and returns
//! only the agent(s) the game is actually waiting on, so play advances one turn
//! at a time with a targeted prompt.

use std::collections::HashSet;

use crate::game::{DayStage, Game, OpenNomination, Phase, SeatId};

/// One targeted tick the scheduler wants to run this cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchedTarget {
    Host(HostTask),
    Player { seat: SeatId, task: PlayerTask },
}

/// What the host agent should do this cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostTask {
    /// Game is in lobby; start it.
    StartGame,
    /// A `pending_host` Storyteller decision is blocking progress.
    ResolveDecision { kind: String },
    /// Night with no pending player/host wait; advance the night machine.
    AdvanceNight,
    /// A player wake has been stuck for several cycles; skip it to the engine
    /// default so the night can advance (escalation fallback).
    SkipStuckWake { seat: SeatId },
    /// Day discussion; pace it and open nominations when ready.
    PaceDiscussion,
    /// Nominations stage, no open vote; manage nominations.
    ManageNominations,
    /// A vote is open; close/tally it when votes are in.
    CloseVoting,
}

/// Consecutive cycles the engine may sit on the same wait before the scheduler
/// escalates to the host (skip a stuck night wake / force-close a stalled vote).
/// Gives the responsible agent time to actually act before the ST overrides it.
pub const STALL_ESCALATE: usize = 3;

/// What a player agent should do this cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerTask {
    /// The engine is waiting on this seat's night choice.
    NightWake { prompt: String },
    /// Day discussion turn: speak / stake out reads / consider nominating.
    Discuss,
    /// Nominations stage: nominate someone (or decline) this turn.
    Nominate,
    /// A nomination is open and this seat has not voted yet.
    Vote { nomination: String },
}

/// Plan which agents tick this cycle from the current game state.
///
/// `rotation` advances round-robin turns during discussion / nomination so each
/// living player gets a turn to speak or nominate across successive cycles.
/// `stall` is how many consecutive prior cycles the engine has sat on the *same*
/// wait (see [`wait_signature`]); once it reaches [`STALL_ESCALATE`] the plan adds
/// a host fallback so a non-acting player / stalled vote can't wedge the game.
/// Returns an empty plan once the game has `Ended` (caller should stop ticking).
pub fn plan_ticks(game: &Game, rotation: usize, stall: usize) -> Vec<SchedTarget> {
    match &game.phase {
        Phase::Lobby => vec![SchedTarget::Host(HostTask::StartGame)],
        Phase::Ended { .. } => vec![],

        // Night is strictly sequential: host-or-one-player, never a fan-out.
        Phase::FirstNight { .. } | Phase::Night { .. } => {
            if let Some(ph) = &game.pending_host {
                vec![SchedTarget::Host(HostTask::ResolveDecision {
                    kind: ph.kind_str().to_string(),
                })]
            } else if let Some(w) = &game.pending_night {
                // Tick only the woken seat. If it keeps failing to act, escalate to
                // a host skip so the night advances instead of looping forever.
                let mut v = vec![SchedTarget::Player {
                    seat: w.seat,
                    task: PlayerTask::NightWake {
                        prompt: w.prompt.clone(),
                    },
                }];
                if stall >= STALL_ESCALATE {
                    v.push(SchedTarget::Host(HostTask::SkipStuckWake { seat: w.seat }));
                }
                v
            } else {
                vec![SchedTarget::Host(HostTask::AdvanceNight)]
            }
        }

        Phase::Day {
            stage: DayStage::Discussion,
            ..
        } => {
            let mut v = vec![SchedTarget::Host(HostTask::PaceDiscussion)];
            if let Some(seat) = nth_living_rotating(game, rotation) {
                v.push(SchedTarget::Player {
                    seat,
                    task: PlayerTask::Discuss,
                });
            }
            v
        }

        Phase::Day {
            stage: DayStage::Nominations,
            ..
        } => {
            if let Some(nom) = &game.current_nomination {
                let voters = pending_voters(game, nom);
                if voters.is_empty() {
                    // Everyone eligible has voted/passed — host tallies.
                    vec![SchedTarget::Host(HostTask::CloseVoting)]
                } else {
                    // Tick only the un-voted seats. Do NOT co-schedule the host
                    // closer here — it could `close_vote` before their votes land
                    // (votes counted as no, late votes dropped). The engine
                    // auto-closes once all living vote; only escalate to a host
                    // close if voting genuinely stalls.
                    let desc = nom_desc(nom);
                    let mut v: Vec<SchedTarget> = voters
                        .into_iter()
                        .map(|seat| SchedTarget::Player {
                            seat,
                            task: PlayerTask::Vote {
                                nomination: desc.clone(),
                            },
                        })
                        .collect();
                    if stall >= STALL_ESCALATE {
                        v.push(SchedTarget::Host(HostTask::CloseVoting));
                    }
                    v
                }
            } else {
                let mut v = vec![SchedTarget::Host(HostTask::ManageNominations)];
                if let Some(seat) = nth_living_rotating(game, rotation) {
                    v.push(SchedTarget::Player {
                        seat,
                        task: PlayerTask::Nominate,
                    });
                }
                v
            }
        }
    }
}

/// A stable signature of what the engine is currently waiting on, for stall
/// detection. `None` in phases that shouldn't stall-escalate (lobby, discussion,
/// ended). The caller compares this cycle's signature to the previous one: same
/// non-`None` signature => increment the stall count; otherwise reset it.
pub fn wait_signature(game: &Game) -> Option<String> {
    match &game.phase {
        Phase::FirstNight { .. } | Phase::Night { .. } => {
            if let Some(ph) = &game.pending_host {
                Some(format!("night_host:{}", ph.kind_str()))
            } else {
                game.pending_night
                    .as_ref()
                    .map(|w| format!("night_wake:{}", w.seat.0))
            }
        }
        Phase::Day {
            stage: DayStage::Nominations,
            ..
        } => game
            .current_nomination
            .as_ref()
            .map(|n| format!("vote:{}-{}", n.by.0, n.target.0)),
        _ => None,
    }
}

/// Human-readable description of an open nomination for the vote prompt.
fn nom_desc(nom: &OpenNomination) -> String {
    format!("seat {} nominated seat {}", nom.by.0, nom.target.0)
}

/// The `rotation`-th living seat (round-robin), or `None` if nobody is alive.
fn nth_living_rotating(game: &Game, rotation: usize) -> Option<SeatId> {
    let living: Vec<SeatId> = game
        .seats
        .iter()
        .filter(|s| s.alive)
        .map(|s| s.id)
        .collect();
    if living.is_empty() {
        None
    } else {
        Some(living[rotation % living.len()])
    }
}

/// Seats still eligible to act on an open nomination and not yet voted/passed:
/// living players, plus dead players who still hold their one ghost vote.
fn pending_voters(game: &Game, nom: &OpenNomination) -> Vec<SeatId> {
    let done: HashSet<SeatId> = nom
        .votes
        .iter()
        .map(|(s, _)| *s)
        .chain(nom.passes.iter().copied())
        .collect();
    game.seats
        .iter()
        .filter(|s| (s.alive || s.ghost_vote_available) && !done.contains(&s.id))
        .map(|s| s.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Game;

    fn new_game(n: usize) -> Game {
        let names: Vec<String> = (0..n).map(|i| format!("P{i}")).collect();
        Game::create(names, 42).expect("create").game
    }

    #[test]
    fn lobby_routes_to_host_start() {
        let g = new_game(7);
        assert_eq!(
            plan_ticks(&g, 0, 0),
            vec![SchedTarget::Host(HostTask::StartGame)]
        );
    }

    #[test]
    fn ended_routes_to_nobody() {
        let mut g = new_game(7);
        g.phase = Phase::Ended {
            winner: crate::game::Winner::Good,
            reason: crate::game::EndReason::DemonDead,
        };
        assert!(plan_ticks(&g, 0, 0).is_empty());
    }

    #[test]
    fn night_pending_host_routes_to_host_only() {
        let mut g = new_game(7);
        g.phase = Phase::FirstNight { cursor: 0 };
        g.pending_host = Some(crate::game::PendingHostDecision::NightInfo {
            seat: SeatId(0),
            step: crate::game::NightStep::Washerwoman { seat: SeatId(0) },
            ability: "Washerwoman".into(),
            reason: "pair_info".into(),
            payload: None,
        });
        let plan = plan_ticks(&g, 0, 0);
        assert_eq!(plan.len(), 1);
        assert!(matches!(plan[0], SchedTarget::Host(HostTask::ResolveDecision { .. })));
    }

    #[test]
    fn night_pending_wake_routes_to_that_seat_only() {
        let mut g = new_game(7);
        g.phase = Phase::FirstNight { cursor: 0 };
        g.pending_host = None;
        g.pending_night = Some(crate::game::PendingWake {
            step: crate::game::NightStep::Poisoner { seat: SeatId(3) },
            seat: SeatId(3),
            schema: crate::game::ChoiceSchema::PickOne {
                any_seat: false,
                living_only: true,
                exclude_self: true,
            },
            prompt: "Choose a player to poison".into(),
        });
        let plan = plan_ticks(&g, 0, 0);
        assert_eq!(
            plan,
            vec![SchedTarget::Player {
                seat: SeatId(3),
                task: PlayerTask::NightWake {
                    prompt: "Choose a player to poison".into()
                }
            }]
        );
    }

    #[test]
    fn night_idle_routes_to_host_advance() {
        let mut g = new_game(7);
        g.phase = Phase::Night {
            night: 2,
            cursor: 0,
        };
        g.pending_host = None;
        g.pending_night = None;
        assert_eq!(
            plan_ticks(&g, 0, 0),
            vec![SchedTarget::Host(HostTask::AdvanceNight)]
        );
    }

    #[test]
    fn discussion_ticks_host_plus_one_rotating_player() {
        let mut g = new_game(7);
        g.phase = Phase::Day {
            day: 1,
            stage: DayStage::Discussion,
        };
        let p0 = plan_ticks(&g, 0, 0);
        assert!(matches!(p0[0], SchedTarget::Host(HostTask::PaceDiscussion)));
        assert_eq!(p0.len(), 2);
        // rotation advances the chosen player
        let seat_a = match &plan_ticks(&g, 0, 0)[1] {
            SchedTarget::Player { seat, .. } => *seat,
            _ => panic!(),
        };
        let seat_b = match &plan_ticks(&g, 1, 0)[1] {
            SchedTarget::Player { seat, .. } => *seat,
            _ => panic!(),
        };
        assert_ne!(seat_a, seat_b);
    }

    fn open_vote_game() -> Game {
        let mut g = new_game(7);
        g.phase = Phase::Day {
            day: 1,
            stage: DayStage::Nominations,
        };
        g.current_nomination = Some(OpenNomination {
            by: SeatId(0),
            target: SeatId(1),
            votes: vec![(SeatId(0), true)],
            passes: vec![],
        });
        g
    }

    fn voter_seats(plan: &[SchedTarget]) -> Vec<u8> {
        plan.iter()
            .filter_map(|t| match t {
                SchedTarget::Player {
                    seat,
                    task: PlayerTask::Vote { .. },
                } => Some(seat.0),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn open_vote_routes_to_unvoted_and_no_host_closer_before_stall() {
        let g = open_vote_game();
        let plan = plan_ticks(&g, 0, 0);
        let voters = voter_seats(&plan);
        assert!(!voters.contains(&0), "seat 0 already voted");
        assert!(voters.contains(&2));
        // #60 fix: host must NOT be co-scheduled to close while votes are pending.
        assert!(
            !plan.iter().any(|t| matches!(t, SchedTarget::Host(HostTask::CloseVoting))),
            "host closer must not race un-voted seats"
        );
    }

    #[test]
    fn open_vote_escalates_to_host_close_after_stall() {
        let g = open_vote_game();
        let plan = plan_ticks(&g, 0, STALL_ESCALATE);
        assert!(
            plan.iter().any(|t| matches!(t, SchedTarget::Host(HostTask::CloseVoting))),
            "a stalled vote should escalate to a host close"
        );
        // voters still ticked too
        assert!(voter_seats(&plan).contains(&2));
    }

    #[test]
    fn stuck_night_wake_escalates_to_host_skip() {
        let mut g = new_game(7);
        g.phase = Phase::FirstNight { cursor: 0 };
        g.pending_host = None;
        g.pending_night = Some(crate::game::PendingWake {
            step: crate::game::NightStep::Poisoner { seat: SeatId(3) },
            seat: SeatId(3),
            schema: crate::game::ChoiceSchema::PickOne {
                any_seat: false,
                living_only: true,
                exclude_self: true,
            },
            prompt: "poison".into(),
        });
        // Below the threshold: only the seat.
        let below = plan_ticks(&g, 0, STALL_ESCALATE - 1);
        assert_eq!(below.len(), 1);
        assert!(matches!(below[0], SchedTarget::Player { .. }));
        // At the threshold: seat + host skip fallback.
        let at = plan_ticks(&g, 0, STALL_ESCALATE);
        assert!(at.iter().any(|t| matches!(
            t,
            SchedTarget::Host(HostTask::SkipStuckWake { seat }) if seat.0 == 3
        )));
        assert!(at.iter().any(|t| matches!(t, SchedTarget::Player { .. })));
    }

    #[test]
    fn wait_signature_tracks_the_current_block() {
        // Discussion: no stall signature.
        let mut g = new_game(7);
        g.phase = Phase::Day {
            day: 1,
            stage: DayStage::Discussion,
        };
        assert_eq!(wait_signature(&g), None);
        // Open vote: signature keyed on the nomination.
        let gv = open_vote_game();
        assert_eq!(wait_signature(&gv).as_deref(), Some("vote:0-1"));
        // A different nomination is a different signature (resets stall).
        assert_ne!(wait_signature(&gv), Some("vote:2-3".to_string()));
    }
}
