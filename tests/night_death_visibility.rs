//! A night kill must NOT be visible to players (roster / `get_public_state`) until
//! the dawn announcement. Only the Storyteller (`get_host_state`) and ability
//! resolution see the true alive-state before dawn. Regression for the night-death
//! roster leak (a later-night-order agent's prompt showed the victim as `dead:`).

mod common;

use botc_mcp::game::{Game, NightActionPayload, Phase, RoleAssignment, SeatId, StartOpts, Winner};
use botc_mcp::roles::Character;
use botc_mcp::tools::{
    get_host_state, get_private_state, get_public_state, night_action, skip_night_action,
};
use common::{advance_to_imp_kill, five_names};

#[test]
fn night_kill_hidden_from_public_view_until_dawn() {
    // Victim is the Ravenkeeper: killing it fires its on-death wake, so the night
    // stays open AFTER the kill — exactly the pre-dawn window whose prompt/tool
    // leaked the death — and lets us also assert the legitimate own-death exception.
    let lobby = Game::create(five_names(), 5).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::normal(SeatId(0), Character::Empath), // observer
                RoleAssignment::normal(SeatId(1), Character::Imp),
                RoleAssignment::normal(SeatId(2), Character::Ravenkeeper), // victim
                RoleAssignment::normal(SeatId(3), Character::Poisoner),
                RoleAssignment::normal(SeatId(4), Character::Soldier),
            ]),
            ..Default::default()
        },
    )
    .unwrap();

    // Finish first night (no Imp kill), then enter night 2.
    while g.pending_night.is_some() || g.pending_host.is_some() {
        skip_night_action(&mut g, &host).unwrap();
    }
    g.enter_night(2).unwrap();

    // Drive to the Imp's kill step (poison a harmless seat; no Monk).
    advance_to_imp_kill(&mut g, &host, &tokens, SeatId(4), None);
    let victim = SeatId(2);
    night_action(
        &mut g,
        &tokens[1],
        NightActionPayload::PickOne { target: victim },
    )
    .unwrap();

    // ---- PRE-DAWN: still Night (Ravenkeeper's on-death wake); victim secretly dead ----
    assert!(
        g.pending_night.is_some() || g.pending_host.is_some(),
        "night should still be open (Ravenkeeper woken on its own death)"
    );
    assert!(!g.seats[2].alive, "true state: victim is dead");
    assert!(
        g.deaths_tonight.contains(&victim),
        "victim is an unannounced night death"
    );
    assert!(
        g.seat_publicly_alive(&g.seats[2]),
        "publicly the victim must still read as ALIVE before dawn"
    );

    // Another player's public view must NOT reveal the death.
    let pv = get_public_state(&g, &tokens[0]).unwrap();
    let seen = pv.seats.iter().find(|s| s.id == victim).unwrap();
    assert!(
        seen.alive,
        "LEAK: get_public_state shows the night victim dead before dawn"
    );
    // The Storyteller grimoire sees the true (dead) state.
    let hv = get_host_state(&g, &host).unwrap();
    let hseat = hv.seats.iter().find(|s| s.seat_id == victim).unwrap();
    assert!(!hseat.alive, "host grimoire sees the true dead state");
    // LEGITIMATE EXCEPTION: the victim's OWN private view reflects its own death
    // (the Ravenkeeper is woken precisely because it died) — self-only, not a roster.
    let own = get_private_state(&g, &tokens[2], 0).unwrap();
    assert!(
        !own.alive,
        "a player knows its OWN death (Ravenkeeper woken on death)"
    );

    // ---- DAWN: finish the night; the death becomes public ----
    while g.pending_night.is_some() || g.pending_host.is_some() {
        skip_night_action(&mut g, &host).unwrap();
    }
    assert!(
        g.deaths_tonight.is_empty(),
        "deaths_tonight cleared at dawn"
    );
    assert!(
        !g.seat_publicly_alive(&g.seats[2]),
        "after dawn the death is public"
    );
    let pv2 = get_public_state(&g, &tokens[0]).unwrap();
    let seen2 = pv2.seats.iter().find(|s| s.id == victim).unwrap();
    assert!(
        !seen2.alive,
        "after dawn get_public_state shows the victim dead"
    );
}

#[test]
fn public_alive_mask_distinguishes_night_vs_day_death() {
    // Direct mask check: a seat in deaths_tonight (unannounced night kill) reads
    // ALIVE publicly; a dead seat NOT in deaths_tonight (day death / already
    // announced) reads DEAD; a living seat reads alive. Dawn clears the mask.
    let lobby = Game::create(five_names(), 5).unwrap();
    let host = lobby.host_token.clone();
    let mut g = lobby.game;
    g.start_game(&host, StartOpts::default()).unwrap();
    g.phase = Phase::Night {
        night: 2,
        cursor: 0,
    }; // masking only applies during a night

    g.seats[2].alive = false;
    g.deaths_tonight.push(SeatId(2)); // secret night kill
    g.seats[3].alive = false; // NOT in deaths_tonight -> a public (day/announced) death

    assert!(
        g.seat_publicly_alive(&g.seats[2]),
        "unannounced night death -> publicly alive"
    );
    assert!(
        !g.seat_publicly_alive(&g.seats[3]),
        "announced/day death -> publicly dead"
    );
    assert!(
        g.seats[0].alive && g.seat_publicly_alive(&g.seats[0]),
        "living seat -> alive"
    );

    g.deaths_tonight.clear(); // dawn
    assert!(
        !g.seat_publicly_alive(&g.seats[2]),
        "after dawn -> publicly dead"
    );

    // Phase gate: outside a night (game ended) the true state shows even if a
    // stale entry lingers in deaths_tonight (game-ending kill never clears it).
    g.deaths_tonight.push(SeatId(2));
    g.phase = Phase::Ended {
        winner: Winner::Evil,
        reason: botc_mcp::game::EndReason::EvilTwoAlive,
    };
    assert!(
        !g.seat_publicly_alive(&g.seats[2]),
        "at game end the deciding death must be public despite stale deaths_tonight"
    );
}

#[test]
fn game_ending_night_kill_does_not_hide_the_deciding_death() {
    // Regression: when a night kill ENDS the game, `dawn()` is skipped so
    // `deaths_tonight` is never cleared. The public roster must STILL show the
    // deciding victim dead (the whole board is revealed at game end) — the mask
    // only applies while a night is actually in progress.
    let lobby = Game::create(five_names(), 5).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::normal(SeatId(0), Character::Imp),
                RoleAssignment::normal(SeatId(1), Character::Poisoner),
                RoleAssignment::normal(SeatId(2), Character::Soldier),
                RoleAssignment::normal(SeatId(3), Character::Empath),
                RoleAssignment::normal(SeatId(4), Character::Mayor),
            ]),
            ..Default::default()
        },
    )
    .unwrap();
    while g.pending_night.is_some() || g.pending_host.is_some() {
        skip_night_action(&mut g, &host).unwrap();
    }
    common::clear_all_poisons(&mut g); // a random N1 poison must not fizzle the N2 kill
                                       // Down to 3 alive (Imp P0, Empath P3, Mayor P4): simulate prior deaths of P1/P2.
    g.seats[1].alive = false;
    g.seats[2].alive = false;

    // Night 2: Imp kills the Empath -> 2 alive -> Evil wins mid-night (dawn skipped).
    g.enter_night(2).unwrap();
    advance_to_imp_kill(&mut g, &host, &tokens, SeatId(4), None);
    night_action(
        &mut g,
        &tokens[0],
        NightActionPayload::PickOne { target: SeatId(3) },
    )
    .unwrap();

    assert!(
        matches!(
            g.phase,
            Phase::Ended {
                winner: Winner::Evil,
                ..
            }
        ),
        "expected Evil win, got {:?}",
        g.phase
    );
    assert!(!g.seats[3].alive, "victim truly dead");
    // The lifecycle gap remains (deaths_tonight not cleared on a game-ending kill)...
    assert!(
        g.deaths_tonight.contains(&SeatId(3)),
        "deaths_tonight still holds the victim after a game-ending kill"
    );
    // ...but the phase gate means the public view is NOT over-hidden at game end.
    assert!(
        !g.seat_publicly_alive(&g.seats[3]),
        "the deciding death must be public once the game is Ended"
    );
    let pv = get_public_state(&g, &tokens[4]).unwrap();
    let seen = pv.seats.iter().find(|s| s.id == SeatId(3)).unwrap();
    assert!(
        !seen.alive,
        "get_public_state must show the deciding victim dead at game end"
    );
}
