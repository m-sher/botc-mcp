//! Turn-routing scheduler.
//!
//! Trouble Brewing is strictly sequential (one night wake at a time; one
//! speaker at a time; votes counted clockwise), but agents run as independent
//! headless sessions. This planner reads the engine state and returns the ONE
//! agent whose turn it is (plus, rarely, a host fallback), so play advances
//! turn by turn with a targeted prompt.
//!
//! Host-minimalism: the engine already auto-opens nominations on a player's
//! `nominate`, auto-closes a vote when everyone has voted, and auto-ends the
//! day into night. The host is therefore woken ONLY for genuine Storyteller
//! decisions (`pending_host`) and as a stall fallback (stuck night wake,
//! stalled vote, or a table that is done with the day → `end_nominations`).

use std::collections::HashSet;

use crate::game::{DayStage, Game, OpenNomination, PendingHostDecision, Phase, SeatId};

/// One targeted tick the scheduler wants to run this cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchedTarget {
    Host(HostTask),
    Player { seat: SeatId, task: PlayerTask },
}

/// What the host agent should do this cycle. Only [`HostTask::ResolveDecision`]
/// is a routine wake; everything else is a lobby/stall fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostTask {
    /// Game is in lobby; start it.
    StartGame,
    /// A `pending_host` Storyteller decision is blocking progress. `detail` is a
    /// concrete, human description of exactly what to decide (seat/ability/choices).
    ResolveDecision { kind: String, detail: String },
    /// Night with no pending player/host wait; advance the night machine.
    AdvanceNight,
    /// A player wake has been stuck for several cycles; skip it to the engine
    /// default so the night can advance (escalation fallback).
    SkipStuckWake { seat: SeatId },
    /// A vote is open but stopped progressing; close/tally it.
    CloseVoting,
    /// The table is done with the day (talk rounds spent / nobody nominating);
    /// end the day. `in_discussion` = still in Discussion (host must
    /// `open_nominations` first, then `end_nominations`).
    EndDay { in_discussion: bool },
}

/// Consecutive cycles the engine may sit on the same wait before the scheduler
/// escalates to the host (skip a stuck night wake / force-close a stalled vote).
/// Gives the responsible agent time to actually act before the ST overrides it.
pub const STALL_ESCALATE: usize = 3;

/// Full table rounds of discussion (each living player speaks once per round)
/// before the scheduler moves the day toward nominations/close.
pub const DISCUSSION_ROUNDS: usize = 2;

/// What a player agent should do this cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerTask {
    /// The engine is waiting on this seat's night choice.
    NightWake { prompt: String },
    /// Day discussion: it is this seat's turn to speak (round-robin, one at a
    /// time). `last_round` = final talk round before the day moves on.
    Discuss { round: usize, last_round: bool },
    /// Nominations stage: this seat may nominate now (or decline).
    Nominate,
    /// A nomination is open and it is this seat's turn to vote (clockwise).
    /// `can_pass` = seat is dead and may abstain with `pass_vote` (living must
    /// vote yes/no — the engine rejects `pass_vote` from living seats).
    Vote {
        nomination: String,
        tally: String,
        can_pass: bool,
    },
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
                    detail: describe_pending_host(ph),
                })]
            } else if let Some(w) = &game.pending_night {
                // Tick only the woken seat. If it keeps failing to act, escalate to
                // a host skip — HOST ONLY on the escalation tick: co-scheduling the
                // player would race it (if the player finally acted first, the
                // un-scoped skip_night_action would default the NEXT seat's wake).
                if stall >= STALL_ESCALATE {
                    return vec![SchedTarget::Host(HostTask::SkipStuckWake { seat: w.seat })];
                }
                vec![SchedTarget::Player {
                    seat: w.seat,
                    task: PlayerTask::NightWake {
                        prompt: w.prompt.clone(),
                    },
                }]
            } else {
                vec![SchedTarget::Host(HostTask::AdvanceNight)]
            }
        }

        Phase::Day {
            stage: DayStage::Discussion,
            ..
        } => {
            // Discussion is sequential: ONE speaker per tick, in seat order, so
            // each speaker sees everything said before them. The host is NOT
            // woken — the day is player-driven (any player's `nominate`
            // auto-opens nominations). The caller resets `rotation` to 0 when
            // the stage begins, so round = rotation / living-count.
            let living: Vec<SeatId> = game
                .seats
                .iter()
                .filter(|s| s.alive)
                .map(|s| s.id)
                .collect();
            if living.is_empty() {
                return vec![];
            }
            let round = rotation / living.len();
            if round >= DISCUSSION_ROUNDS {
                // Talk rounds spent and nobody nominated — close the day.
                return vec![SchedTarget::Host(HostTask::EndDay {
                    in_discussion: true,
                })];
            }
            vec![SchedTarget::Player {
                seat: living[rotation % living.len()],
                task: PlayerTask::Discuss {
                    round,
                    last_round: round + 1 == DISCUSSION_ROUNDS,
                },
            }]
        }

        Phase::Day {
            stage: DayStage::Nominations,
            ..
        } => {
            if let Some(nom) = &game.current_nomination {
                // Votes are counted one at a time, clockwise from the nominee
                // (each voter sees the hands before theirs). The engine
                // auto-closes once everyone eligible has voted/passed.
                let voters = pending_voters_clockwise(game, nom);
                if voters.is_empty() {
                    // Lingering open vote (normally auto-closed) — host tallies.
                    return vec![SchedTarget::Host(HostTask::CloseVoting)];
                }
                // A stalled voter must not block the queue: each stalled cycle
                // offers the NEXT pending voter, so everyone gets a turn before
                // the host force-closes (which counts the missing as "no").
                // HOST ONLY on the escalation tick — co-scheduling a voter with
                // close_vote would race it and discard recovered votes.
                if stall >= voters.len().max(STALL_ESCALATE) {
                    return vec![SchedTarget::Host(HostTask::CloseVoting)];
                }
                let seat = voters[stall % voters.len()];
                let alive = game
                    .seats
                    .iter()
                    .find(|s| s.id == seat)
                    .map(|s| s.alive)
                    .unwrap_or(true);
                vec![SchedTarget::Player {
                    seat,
                    task: PlayerTask::Vote {
                        nomination: nom_desc(nom),
                        tally: tally_desc(game, nom),
                        // pass_vote is dead-only in the engine.
                        can_pass: !alive,
                    },
                }]
            } else {
                // No open vote: offer the nomination turn to living players who
                // have not nominated today, one per tick. When everyone has had
                // a fair chance and nothing changed (stall), the host ends the day.
                let eligible: Vec<SeatId> = game
                    .seats
                    .iter()
                    .filter(|s| s.alive && !game.day_nominators.contains(&s.id))
                    .map(|s| s.id)
                    .collect();
                let everyone_had_a_turn = stall >= eligible.len().max(2);
                if eligible.is_empty() || everyone_had_a_turn {
                    return vec![SchedTarget::Host(HostTask::EndDay {
                        in_discussion: false,
                    })];
                }
                vec![SchedTarget::Player {
                    seat: eligible[rotation % eligible.len()],
                    task: PlayerTask::Nominate,
                }]
            }
        }
    }
}

/// A stable signature of what the engine is currently waiting on, for stall
/// detection. `None` in phases that shouldn't stall-escalate (lobby, discussion
/// — talk rounds are bounded by [`DISCUSSION_ROUNDS`], not by stalling — and
/// ended). The caller compares this cycle's signature to the previous one: same
/// non-`None` signature => increment the stall count; otherwise reset it.
/// Vote signatures include the vote/pass count so each landed vote resets the
/// stall; the nominations-idle signature includes the nominator/nominee counts
/// so each new nomination resets it.
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
            day,
            stage: DayStage::Nominations,
        } => {
            // Include the living count: a mid-stage death (Slayer) changes the
            // roster, so the fairness window must restart.
            let living = game.seats.iter().filter(|s| s.alive).count();
            match &game.current_nomination {
                Some(n) => Some(format!(
                    "vote:{}-{}:{}:{living}",
                    n.by.0,
                    n.target.0,
                    n.votes.len() + n.passes.len()
                )),
                None => Some(format!(
                    "noms_idle:{day}:{}:{}:{living}",
                    game.day_nominators.len(),
                    game.day_nominees.len()
                )),
            }
        }
        _ => None,
    }
}

/// Human-readable description of an open nomination for the vote prompt.
fn nom_desc(nom: &OpenNomination) -> String {
    format!("P{} nominated P{} for execution", nom.by.0, nom.target.0)
}

/// Human-readable running tally for the vote prompt ("P0 YES, P2 no — 2 of 6
/// eligible voters have acted").
fn tally_desc(game: &Game, nom: &OpenNomination) -> String {
    let mut parts: Vec<String> = nom
        .votes
        .iter()
        .map(|(s, yes)| format!("P{} {}", s.0, if *yes { "YES" } else { "no" }))
        .collect();
    parts.extend(nom.passes.iter().map(|s| format!("P{} passed", s.0)));
    let acted_ids: HashSet<SeatId> = nom
        .votes
        .iter()
        .map(|(s, _)| *s)
        .chain(nom.passes.iter().copied())
        .collect();
    // Eligible = living, dead with a ghost vote remaining, or anyone who already
    // acted (a dead ghost-YES voter has spent the token but still counts here).
    let eligible = game
        .seats
        .iter()
        .filter(|s| s.alive || s.ghost_vote_available || acted_ids.contains(&s.id))
        .count();
    let acted = acted_ids.len();
    if parts.is_empty() {
        format!("no votes yet — {eligible} eligible voters")
    } else {
        format!("{} — {acted} of {eligible} eligible have acted", parts.join(", "))
    }
}

/// Seats still to vote, in clockwise order starting after the nominee, among
/// those eligible (living, or dead with a ghost vote) who have not voted/passed.
fn pending_voters_clockwise(game: &Game, nom: &OpenNomination) -> Vec<SeatId> {
    let done: HashSet<SeatId> = nom
        .votes
        .iter()
        .map(|(s, _)| *s)
        .chain(nom.passes.iter().copied())
        .collect();
    let n = game.seats.len();
    (1..=n)
        .map(|off| {
            let idx = (nom.target.0 as usize + off) % n;
            &game.seats[idx]
        })
        .filter(|s| (s.alive || s.ghost_vote_available) && !done.contains(&s.id))
        .map(|s| s.id)
        .collect()
}

/// Concrete description of the pending Storyteller decision, so the host knows
/// exactly what it is deciding (rather than just a `kind` string).
fn describe_pending_host(ph: &PendingHostDecision) -> String {
    let seats = |v: &[SeatId]| {
        v.iter()
            .map(|s| format!("P{}", s.0))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match ph {
        PendingHostDecision::NightInfo {
            seat,
            ability,
            reason,
            ..
        } => format!(
            "P{} (playing {ability}) must receive their night information now (reason: {reason}). \
             Provide it with `host_decide` (the private text they learn), or call `skip_night_action` \
             to have the engine generate valid default info for them. Either one advances the night.",
            seat.0
        ),
        PendingHostDecision::MayorRedirect {
            mayor,
            living_others,
        } => format!(
            "The Imp attacked the Mayor (P{}). With `host_decide` choose one: bounce the kill onto \
             another living player ({}), let nobody die, or let the Mayor die. Or `skip_night_action` \
             for the engine default.",
            mayor.0,
            seats(living_others)
        ),
        PendingHostDecision::StarpassPick { minions, dead_imp } => format!(
            "The Imp (P{}) self-killed (starpass). With `host_decide` choose which minion ({}) becomes \
             the new Imp, or `skip_night_action` for the engine default.",
            dead_imp.0,
            seats(minions)
        ),
    }
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
    fn discussion_is_sequential_one_speaker_no_host() {
        let mut g = new_game(7);
        g.phase = Phase::Day {
            day: 1,
            stage: DayStage::Discussion,
        };
        // One speaker per tick, in seat order; the host is never woken.
        for rot in 0..7 {
            let plan = plan_ticks(&g, rot, 0);
            assert_eq!(plan.len(), 1, "exactly one target per tick");
            match &plan[0] {
                SchedTarget::Player {
                    seat,
                    task: PlayerTask::Discuss { round, .. },
                } => {
                    assert_eq!(seat.0 as usize, rot % 7, "seat order");
                    assert_eq!(*round, 0);
                }
                t => panic!("expected a Discuss player turn, got {t:?}"),
            }
        }
        // Second round: same order, round=1 and flagged as last (DISCUSSION_ROUNDS=2).
        match &plan_ticks(&g, 7, 0)[0] {
            SchedTarget::Player {
                task: PlayerTask::Discuss { round, last_round },
                ..
            } => {
                assert_eq!(*round, 1);
                assert!(*last_round);
            }
            t => panic!("expected Discuss, got {t:?}"),
        }
        // Rounds spent → host ends the day (from Discussion).
        let done = plan_ticks(&g, 7 * DISCUSSION_ROUNDS, 0);
        assert_eq!(
            done,
            vec![SchedTarget::Host(HostTask::EndDay { in_discussion: true })]
        );
        // Dead players don't get speaking turns.
        g.seats[2].alive = false;
        let seats: Vec<u8> = (0..6)
            .map(|rot| match &plan_ticks(&g, rot, 0)[0] {
                SchedTarget::Player { seat, .. } => seat.0,
                t => panic!("expected player, got {t:?}"),
            })
            .collect();
        assert!(!seats.contains(&2));
        assert_eq!(seats.len(), 6);
    }

    #[test]
    fn nominations_offer_turns_then_host_ends_day() {
        let mut g = new_game(7);
        g.phase = Phase::Day {
            day: 1,
            stage: DayStage::Nominations,
        };
        // No open vote: one prospective nominator per tick, never the host.
        let p = plan_ticks(&g, 0, 0);
        assert_eq!(p.len(), 1);
        assert!(matches!(
            p[0],
            SchedTarget::Player { task: PlayerTask::Nominate, .. }
        ));
        // Someone who already nominated today is skipped.
        g.day_nominators.push(SeatId(0));
        let seats: Vec<u8> = (0..6)
            .map(|rot| match &plan_ticks(&g, rot, 0)[0] {
                SchedTarget::Player { seat, .. } => seat.0,
                t => panic!("expected player, got {t:?}"),
            })
            .collect();
        assert!(!seats.contains(&0));
        // Once everyone had a turn with no new nomination (stall), host ends the day.
        let done = plan_ticks(&g, 0, 6);
        assert_eq!(
            done,
            vec![SchedTarget::Host(HostTask::EndDay { in_discussion: false })]
        );
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
    fn open_vote_ticks_one_voter_clockwise_from_nominee() {
        let g = open_vote_game(); // P0 nominated P1; P0 already voted YES
        let plan = plan_ticks(&g, 0, 0);
        // Exactly ONE voter per tick — clockwise from the nominee (P1) the next
        // eligible unvoted seat is P2 (P1 itself may vote, but voting starts at
        // nominee+1; P2 comes first). No host co-scheduled before a stall.
        assert_eq!(voter_seats(&plan), vec![2]);
        assert!(
            !plan.iter().any(|t| matches!(t, SchedTarget::Host(_))),
            "host must not race pending voters"
        );
        // The Vote task carries the running tally.
        match &plan[0] {
            SchedTarget::Player {
                task: PlayerTask::Vote { tally, .. },
                ..
            } => assert!(tally.contains("P0 YES"), "tally missing: {tally}"),
            t => panic!("expected Vote, got {t:?}"),
        }
    }

    #[test]
    fn stalled_vote_offers_next_voters_then_host_closes_alone() {
        let g = open_vote_game(); // P0 nominated P1; P0 voted; pending = P2..P6,P1 (6 voters)
        // Each stalled cycle offers the NEXT pending voter (a stuck voter must
        // not block the queue): stall 0 → P2, stall 1 → P3, …
        for (stall, expect) in [(0usize, 2u8), (1, 3), (2, 4), (3, 5), (4, 6), (5, 1)] {
            let plan = plan_ticks(&g, 0, stall);
            assert_eq!(voter_seats(&plan), vec![expect], "stall={stall}");
            assert!(
                !plan.iter().any(|t| matches!(t, SchedTarget::Host(_))),
                "host must not race pending voters (stall={stall})"
            );
        }
        // Once every pending voter has been offered a turn: HOST ONLY closes
        // (no voter co-scheduled — close_vote would race a recovering voter).
        let esc = plan_ticks(&g, 0, 6);
        assert_eq!(
            esc,
            vec![SchedTarget::Host(HostTask::CloseVoting)],
            "escalation tick must be host-only"
        );
        // Dead living-vote gate: the offered voter task says whether pass is legal.
        match &plan_ticks(&g, 0, 0)[0] {
            SchedTarget::Player {
                task: PlayerTask::Vote { can_pass, .. },
                ..
            } => assert!(!can_pass, "P2 is alive; pass_vote must not be offered"),
            t => panic!("expected Vote, got {t:?}"),
        }
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
        // At the threshold: HOST ONLY (co-scheduling the player would race the
        // un-scoped skip_night_action onto the NEXT seat's wake).
        let at = plan_ticks(&g, 0, STALL_ESCALATE);
        assert_eq!(
            at,
            vec![SchedTarget::Host(HostTask::SkipStuckWake { seat: SeatId(3) })],
            "escalation tick must be host-only"
        );
    }

    #[test]
    fn wait_signature_tracks_the_current_block() {
        // Discussion: no stall signature (rounds bound it instead).
        let mut g = new_game(7);
        g.phase = Phase::Day {
            day: 1,
            stage: DayStage::Discussion,
        };
        assert_eq!(wait_signature(&g), None);
        // Open vote: keyed on the nomination AND vote progress — a landed vote
        // changes the signature, resetting the stall counter.
        let mut gv = open_vote_game();
        assert_eq!(wait_signature(&gv).as_deref(), Some("vote:0-1:1:7"));
        gv.current_nomination.as_mut().unwrap().votes.push((SeatId(2), false));
        assert_eq!(wait_signature(&gv).as_deref(), Some("vote:0-1:2:7"));
        // A mid-stage death (Slayer) changes the roster → signature changes →
        // the fairness window restarts.
        gv.seats[6].alive = false;
        assert_eq!(wait_signature(&gv).as_deref(), Some("vote:0-1:2:6"));
        gv.seats[6].alive = true;
        // Nominations with no open vote: idle signature keyed on nomination
        // counts — a new nomination resets the stall.
        gv.current_nomination = None;
        let sig_a = wait_signature(&gv);
        assert!(sig_a.as_deref().unwrap().starts_with("noms_idle:1:"));
        gv.day_nominators.push(SeatId(3));
        assert_ne!(wait_signature(&gv), sig_a);
    }
}
