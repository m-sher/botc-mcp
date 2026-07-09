//! `Game` aggregate: seats, phase, comms handles, win state.

use crate::auth::{Token, TokenBook};
use crate::comms::{PrivateInboxes, PrivateMessage, PublicEvent, PublicLog};
use crate::game::phase::{NightStep, Phase};
use crate::game::seat::{Seat, SeatId};
use crate::roles::{Character, CharacterType, Team};

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

/// Per-seat assignment at start. Drunk **must** include a Townsfolk face.
#[derive(Debug, Clone)]
pub struct RoleAssignment {
    pub seat: SeatId,
    pub true_character: Character,
    /// Required when `true_character` is Drunk; ignored otherwise (should be None).
    pub believed_character: Option<Character>,
}

impl RoleAssignment {
    pub fn normal(seat: SeatId, true_character: Character) -> Self {
        Self {
            seat,
            true_character,
            believed_character: None,
        }
    }

    pub fn drunk(seat: SeatId, townsfolk_face: Character) -> Result<Self, GameError> {
        if townsfolk_face.character_type() != CharacterType::Townsfolk {
            return Err(GameError::IllegalAction(
                "Drunk face must be a Townsfolk character",
            ));
        }
        if townsfolk_face == Character::Drunk {
            return Err(GameError::IllegalAction("Drunk face cannot be Drunk"));
        }
        Ok(Self {
            seat,
            true_character: Character::Drunk,
            believed_character: Some(townsfolk_face),
        })
    }
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

    /// Player-facing character for private role tools. Never returns Drunk when a face is set.
    pub fn player_facing_character(&self, seat: SeatId) -> Option<Character> {
        self.seats
            .iter()
            .find(|s| s.id == seat)
            .and_then(|s| s.visible_character())
    }

    /// Assign characters and push private `YouAre` using **player-facing** identity only.
    pub fn start_game_assign(
        &mut self,
        assignments: Vec<RoleAssignment>,
    ) -> Result<(), GameError> {
        for a in &assignments {
            if a.true_character == Character::Drunk {
                let face = a.believed_character.ok_or(GameError::IllegalAction(
                    "Drunk assignment requires a Townsfolk believed_character face",
                ))?;
                if face.character_type() != CharacterType::Townsfolk {
                    return Err(GameError::IllegalAction(
                        "Drunk face must be a Townsfolk character",
                    ));
                }
            } else if a.believed_character.is_some() {
                return Err(GameError::IllegalAction(
                    "believed_character only valid for Drunk",
                ));
            }
        }

        for a in &assignments {
            let seat = self
                .seats
                .iter_mut()
                .find(|s| s.id == a.seat)
                .ok_or(GameError::NoSuchSeat)?;
            seat.true_character = Some(a.true_character);
            seat.is_drunk_outsider = a.true_character == Character::Drunk;
            seat.believed_character = a.believed_character;
        }

        // Snapshot faces for private messages (immutable borrow after mut loop).
        let briefings: Vec<(SeatId, Character, Team)> = self
            .seats
            .iter()
            .filter_map(|seat| {
                let true_c = seat.true_character?;
                let facing = seat.visible_character()?;
                Some((seat.id, facing, true_c.team()))
            })
            .collect();

        for (seat_id, facing, team) in briefings {
            // facing is never Drunk if setup enforced a face.
            debug_assert!(
                facing != Character::Drunk,
                "player-facing identity must not be Drunk"
            );
            self.private_inboxes.push(
                seat_id,
                PrivateMessage::YouAre {
                    character_label: facing.display_name().to_string(),
                    team,
                    rules_path: facing.rules_doc_path().to_string(),
                    note: None,
                },
            );
        }

        self.phase = Phase::FirstNight {
            step: NightStep::SetupMarkers,
        };
        self.st_announce("Night falls. Eyes closed. The first night begins.");
        Ok(())
    }

    /// Sketch helper: plain (seat, character) pairs. Drunk is rejected — use [`RoleAssignment::drunk`].
    pub fn start_game_assign_for_sketch(
        &mut self,
        assignments: Vec<(SeatId, Character)>,
    ) -> Result<(), GameError> {
        let mapped = assignments
            .into_iter()
            .map(|(seat, c)| {
                if c == Character::Drunk {
                    Err(GameError::IllegalAction(
                        "use RoleAssignment::drunk(seat, townsfolk_face) for Drunk",
                    ))
                } else {
                    Ok(RoleAssignment::normal(seat, c))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.start_game_assign(mapped)
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
