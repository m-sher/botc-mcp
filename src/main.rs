//! Binary entry — will host the MCP transport later.
//!
//! Library sketch: `botc_mcp`. Design: `docs/architecture.md`.

fn main() {
    println!("botc-mcp — engine sketch in lib; see docs/architecture.md");
}

#[cfg(test)]
mod smoke {
    use botc_mcp::comms::PublicEvent;
    use botc_mcp::game::{Game, GameId, SeatId};
    use botc_mcp::roles::Character;
    use botc_mcp::tools;

    #[test]
    fn public_chat_is_shared_roles_are_not() {
        let lobby = Game::new_lobby(GameId(1), vec!["Alice".into(), "Bob".into()]);
        let mut game = lobby.game;
        let alice = lobby.player_tokens[0].clone();
        let bob = lobby.player_tokens[1].clone();

        game.start_game_assign_for_sketch(vec![
            (SeatId(0), Character::Empath),
            (SeatId(1), Character::Imp),
        ]);

        tools::say(&mut game, &alice, "I am the Imp.".into()).unwrap();

        let log = tools::get_public_log(&game, &bob, 0).unwrap();
        assert!(log.iter().any(|(_, e)| matches!(
            e,
            PublicEvent::Chat { text, .. } if text == "I am the Imp."
        )));

        let bob_priv = tools::get_private_state(&game, &bob, 0).unwrap();
        assert_eq!(bob_priv.character_label.as_deref(), Some("Imp"));
        assert!(!bob_priv
            .private_messages_since
            .iter()
            .any(|(_, m)| format!("{m:?}").contains("Empath")));

        let alice_priv = tools::get_private_state(&game, &alice, 0).unwrap();
        assert_eq!(alice_priv.character_label.as_deref(), Some("Empath"));

        let pub_state = tools::get_public_state(&game, &bob).unwrap();
        let dump = format!("{:?}", pub_state.seats);
        assert!(!dump.contains("Empath") && !dump.contains("Imp"));
    }
}
