//! Shared helpers for end-to-end Trouble Brewing integration scenarios.

#![allow(dead_code)]

use botc_mcp::auth::Token;
use botc_mcp::game::{
    CreateGameResult, DayStage, Game, NightActionPayload, NightStep, Phase, RoleAssignment, SeatId,
    StartOpts, Winner,
};
use botc_mcp::roles::Character;
use botc_mcp::tools::{
    end_nominations, night_action, nominate, open_nominations, skip_night_action, start_game, vote,
};

pub fn names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("P{i}")).collect()
}

pub fn five_names() -> Vec<String> {
    names(5)
}

/// Create lobby + start with fixed role assignments. Returns (game, host, player_tokens).
pub fn start_scripted(
    seed: u64,
    assignments: Vec<RoleAssignment>,
) -> (Game, Token, Vec<Token>) {
    let n = assignments.len();
    let CreateGameResult {
        mut game,
        host_token,
        player_tokens,
    } = Game::create(names(n), seed).expect("create lobby");
    start_game(
        &mut game,
        &host_token,
        StartOpts {
            assignments: Some(assignments),
        },
    )
    .expect("start_game");
    (game, host_token, player_tokens)
}

/// Host-skip all pending night wakes until the night ends (or game ends).
pub fn finish_night(g: &mut Game, host: &Token) {
    while g.pending_night.is_some()
        && !matches!(g.phase, Phase::Day { .. } | Phase::Ended { .. })
    {
        skip_night_action(g, host).expect("skip_night_action");
    }
}

/// Finish first night into Day 1 Discussion.
pub fn to_day1(g: &mut Game, host: &Token) {
    finish_night(g, host);
    assert!(
        matches!(
            g.phase,
            Phase::Day {
                day: 1,
                stage: DayStage::Discussion
            }
        ),
        "expected Day 1 Discussion, got {:?}",
        g.phase
    );
}

/// All living seats cast `support` on the open nomination (auto-closes when complete).
pub fn all_vote(g: &mut Game, tokens: &[Token], nominee: SeatId, support: bool) {
    let living: Vec<usize> = g
        .seats
        .iter()
        .filter(|s| s.alive)
        .map(|s| s.id.0 as usize)
        .collect();
    for i in living {
        vote(g, &tokens[i], nominee, support).expect("vote");
    }
}

/// Open noms → nominate `nominee` by `nominator` → all yes → end noms.
pub fn execute_seat(
    g: &mut Game,
    host: &Token,
    tokens: &[Token],
    nominator: SeatId,
    nominee: SeatId,
) {
    open_nominations(g, host).expect("open_nominations");
    nominate(g, &tokens[nominator.0 as usize], nominee).expect("nominate");
    all_vote(g, tokens, nominee, true);
    end_nominations(g, host).expect("end_nominations");
}

/// Clear poison flags (N1 Poisoner skip may leave residual poison).
pub fn clear_all_poisons(g: &mut Game) {
    for s in &mut g.seats {
        s.poisoned = false;
    }
}

/// Advance an other-night until Imp `DemonKill` is pending.
/// Assumes poisoner then optional monk before demon (standard TB other-night).
pub fn advance_to_imp_kill(
    g: &mut Game,
    host: &Token,
    tokens: &[Token],
    poison_target: SeatId,
    monk_target: Option<SeatId>,
) {
    // Drive until DemonKill, handling Poisoner / Monk if present.
    loop {
        let Some(p) = g.pending_night.as_ref() else {
            panic!("no pending night when waiting for Imp kill; phase={:?}", g.phase);
        };
        match p.step {
            NightStep::Poisoner { seat } => {
                night_action(
                    g,
                    &tokens[seat.0 as usize],
                    NightActionPayload::PickOne {
                        target: poison_target,
                    },
                )
                .expect("poisoner");
            }
            NightStep::Monk { seat } => {
                if let Some(t) = monk_target {
                    night_action(
                        g,
                        &tokens[seat.0 as usize],
                        NightActionPayload::PickOne { target: t },
                    )
                    .expect("monk");
                } else {
                    skip_night_action(g, host).expect("skip monk");
                }
            }
            NightStep::DemonKill { .. } => break,
            _ => {
                skip_night_action(g, host).expect("skip unexpected night step");
            }
        }
    }
    let p = g.pending_night.as_ref().expect("imp pending");
    assert!(
        matches!(p.step, NightStep::DemonKill { .. }),
        "expected DemonKill, got {:?}",
        p.step
    );
}

pub fn assert_good_wins_demon_dead(g: &Game) {
    assert_eq!(g.winner, Some(Winner::Good));
    assert!(
        matches!(
            g.phase,
            Phase::Ended {
                winner: Winner::Good,
                reason: botc_mcp::game::EndReason::DemonDead
            }
        ),
        "expected Ended Good DemonDead, got {:?}",
        g.phase
    );
}

pub fn living_count(g: &Game) -> usize {
    g.seats.iter().filter(|s| s.alive).count()
}

/// Find seat with given true character (first match).
pub fn seat_of(g: &Game, ch: Character) -> SeatId {
    g.seats
        .iter()
        .find(|s| s.true_character == Some(ch))
        .map(|s| s.id)
        .unwrap_or_else(|| panic!("no seat with {ch:?}"))
}
