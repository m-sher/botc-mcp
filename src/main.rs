//! Binary entry — will host the MCP transport later.
//!
//! Library sketch: `botc_mcp`. Design: `docs/architecture.md`.

fn main() {
    println!("botc-mcp — engine sketch in lib; see docs/architecture.md");
}

#[cfg(test)]
mod smoke {
    use botc_mcp::comms::PublicEvent;
    use botc_mcp::game::{Game, GameId, RoleAssignment, SeatId};
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
        ])
        .unwrap();

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

    #[test]
    fn drunk_private_state_shows_townsfolk_face_not_drunk() {
        let lobby = Game::new_lobby(GameId(2), vec!["Dana".into(), "Eve".into()]);
        let mut game = lobby.game;
        let dana = lobby.player_tokens[0].clone();

        game.start_game_assign(vec![
            RoleAssignment::drunk(SeatId(0), Character::Empath).unwrap(),
            RoleAssignment::normal(SeatId(1), Character::Imp),
        ])
        .unwrap();

        let facing = game.player_facing_character(SeatId(0));
        assert_eq!(facing, Some(Character::Empath));

        let dana_priv = tools::get_private_state(&game, &dana, 0).unwrap();
        assert_eq!(dana_priv.character_label.as_deref(), Some("Empath"));
        assert!(dana_priv
            .rules_path
            .as_deref()
            .is_some_and(|p| p.contains("empath")));
        assert_ne!(dana_priv.character_label.as_deref(), Some("Drunk"));
        assert!(!dana_priv
            .private_messages_since
            .iter()
            .any(|(_, m)| format!("{m:?}").contains("Drunk")));

        // Grimoire truth still Drunk.
        let true_c = game.seats[0].true_character;
        assert_eq!(true_c, Some(Character::Drunk));
    }

    #[test]
    fn drunk_without_face_rejected() {
        let lobby = Game::new_lobby(GameId(3), vec!["A".into()]);
        let mut game = lobby.game;
        let err = game
            .start_game_assign_for_sketch(vec![(SeatId(0), Character::Drunk)])
            .unwrap_err();
        assert!(matches!(err, botc_mcp::game::GameError::IllegalAction(_)));
    }
}
