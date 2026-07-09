//! `Game` aggregate: seats, phase, comms handles, win state.

use crate::auth::{Token, TokenBook};
use crate::comms::{PrivateInboxes, PrivateMessage, PublicEvent, PublicLog};
use crate::game::phase::{NightStep, Phase};
use crate::game::seat::{Seat, SeatId};
use crate::roles::{Character, Team};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GameId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Winner {
    Good,
    Evil,
}

impl From<Winner> for Team {
    fn from(w: Winner) -> Self {
        match w {
            Winner::Good => Team::Good,
            Winner::Evil => Team::Evil,
        }
    }
}

/// Public projection of a seat (no characters).
#[derive(Debug, Clone)]
pub struct PublicSeatView {
    pub id: SeatId,
    pub name: String,
    pub alive: bool,
    pub ghost_vote_available: bool,
}

#[derive(Debug)]
pub struct Game {
    pub id: GameId,
    pub phase: Phase,
    pub seats: Vec<Seat>,
    pub tokens: TokenBook,
    pub public_log: PublicLog,
    pub private_inboxes: PrivateInboxes,
    pub winner: Option<Winner>,
    /// Fortune Teller red herring seat, if FT in play.
    pub red_herring: Option<SeatId>,
    /// Three not-in-play good characters shown to Imp (7+).
    pub demon_bluffs: Vec<Character>,
}

/// Result of opening a lobby: host token + player tokens in seat order.
pub struct Lobby {
    pub game: Game,
    pub host_token: Token,
    pub player_tokens: Vec<Token>,
}

impl Game {
    pub fn new_lobby(id: GameId, names: Vec<String>) -> Lobby {
        let mut tokens = TokenBook::default();
        let host_token = tokens.issue_host();
        let mut player_tokens = Vec::with_capacity(names.len());
        let seats: Vec<Seat> = names
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                let seat = SeatId(i as u8);
                player_tokens.push(tokens.issue_player(seat));
                Seat::new(seat, name)
            })
            .collect();

        Lobby {
            game: Self {
                id,
                phase: Phase::Lobby,
                seats,
                tokens,
                public_log: PublicLog::default(),
                private_inboxes: PrivateInboxes::default(),
                winner: None,
                red_herring: None,
                demon_bluffs: Vec::new(),
            },
            host_token,
            player_tokens,
        }
    }

    pub fn public_seats(&self) -> Vec<PublicSeatView> {
        self.seats
            .iter()
            .map(|s| PublicSeatView {
                id: s.id,
                name: s.display_name.clone(),
                alive: s.alive,
                ghost_vote_available: s.ghost_vote_available,
            })
            .collect()
    }

    /// Public-only speech. No recipient field by design.
    pub fn say(&mut self, seat: SeatId, text: String) -> Result<(), GameError> {
        let name = self
            .seats
            .iter()
            .find(|s| s.id == seat)
            .map(|s| s.display_name.clone())
            .ok_or(GameError::NoSuchSeat)?;
        self.public_log.push(PublicEvent::Chat { seat, name, text });
        Ok(())
    }

    pub fn st_announce(&mut self, text: impl Into<String>) {
        self.public_log.push(PublicEvent::StorytellerAnnounce {
            text: text.into(),
        });
    }

    /// Deliver Storyteller private info to one seat only.
    pub fn st_whisper(&mut self, seat: SeatId, msg: PrivateMessage) {
        self.private_inboxes.push(seat, msg);
    }

    /// Sketch: assign characters & push private `YouAre` messages.
    pub fn start_game_assign_for_sketch(&mut self, assignments: Vec<(SeatId, Character)>) {
        for (seat_id, character) in &assignments {
            if let Some(seat) = self.seats.iter_mut().find(|s| s.id == *seat_id) {
                seat.true_character = Some(*character);
                seat.is_drunk_outsider = matches!(character, Character::Drunk);
                // Drunk face assignment is a later setup step; until then visible = true.
                seat.believed_character = None;
            }
        }
        for seat in &self.seats {
            let Some(true_c) = seat.true_character else {
                continue;
            };
            let visible = seat.visible_character().unwrap_or(true_c);
            self.private_inboxes.push(
                seat.id,
                PrivateMessage::YouAre {
                    character_label: visible.display_name().to_string(),
                    team: true_c.team(),
                    rules_path: visible.rules_doc_path().to_string(),
                    note: None,
                },
            );
        }
        self.phase = Phase::FirstNight {
            step: NightStep::SetupMarkers,
        };
        self.st_announce("Night falls. Eyes closed. The first night begins.");
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameError {
    NoSuchSeat,
    Unauthorized,
    WrongPhase,
    IllegalAction(&'static str),
    GameAlreadyEnded,
}
