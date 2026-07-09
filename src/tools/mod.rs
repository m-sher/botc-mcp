//! MCP tool handlers (sketch).
//!
//! Transport (JSON-RPC / rmcp / etc.) is not wired yet. These functions are the
//! semantic API the MCP layer should call after deserializing arguments.

use crate::auth::{Actor, Token};
use crate::comms::{EventId, PrivateMessage, PublicEvent};
use crate::error::GameError;
use crate::game::Phase;
use crate::game::SeatId;
use crate::game::{CreateGameResult, Game, GameId, PublicSeatView, StartOpts, Winner};
use crate::roles::Character;
use crate::store::GameStore;

// Re-export for callers that import tool-layer errors from this module.
pub use crate::error::ToolError;

/// One seat’s credentials returned from [`create_game`].
#[derive(Debug, Clone)]
pub struct PlayerSeatToken {
    pub seat_id: SeatId,
    pub name: String,
    pub player_token: Token,
}

/// Response for [`create_game`] / [`create_game_in_memory`].
#[derive(Debug)]
pub struct CreateGameResponse {
    pub game_id: GameId,
    pub host_token: Token,
    pub players: Vec<PlayerSeatToken>,
}

/// Create a lobby, issue host + player tokens, insert into `store`.
pub fn create_game(
    store: &mut GameStore,
    names: Vec<String>,
    seed: u64,
) -> Result<CreateGameResponse, ToolError> {
    let CreateGameResult {
        game,
        host_token,
        player_tokens,
    } = Game::create(names, seed)?;

    let players: Vec<PlayerSeatToken> = game
        .seats
        .iter()
        .zip(player_tokens)
        .map(|(seat, player_token)| PlayerSeatToken {
            seat_id: seat.id,
            name: seat.display_name.clone(),
            player_token,
        })
        .collect();

    let game_id = store.insert(game);
    Ok(CreateGameResponse {
        game_id,
        host_token,
        players,
    })
}

/// Test helper: builds a lobby response only (ephemeral store is dropped).
///
/// Suitable for asserting token/shape fields; does **not** retain a live
/// [`GameStore`]. For stateful tests, use [`create_game`] with an owned store.
/// Panics if lobby size is illegal (not 5–15).
pub fn create_game_in_memory(names: Vec<String>, seed: u64) -> CreateGameResponse {
    let mut store = GameStore::new();
    create_game(&mut store, names, seed).expect("create_game_in_memory: valid player count 5–15")
}

#[derive(Debug)]
pub struct PublicStateView {
    pub phase: String,
    pub seats: Vec<PublicSeatView>,
    pub winner: Option<Winner>,
}

/// Player-facing private snapshot. Must never expose Drunk as `character_label`.
#[derive(Debug)]
pub struct PrivateStateView {
    pub seat: SeatId,
    pub name: String,
    pub alive: bool,
    /// Character the player should play as (Drunk → Townsfolk face only).
    pub character_label: Option<String>,
    pub team_label: Option<String>,
    pub rules_path: Option<String>,
    pub private_messages_since: Vec<(EventId, PrivateMessage)>,
    pub awaiting_action: bool,
}

/// Host locks lobby, assigns bag, enters First Night.
pub fn start_game(game: &mut Game, host: &Token, opts: StartOpts) -> Result<(), ToolError> {
    game.start_game(host, opts).map_err(ToolError::from)
}

/// Host or player: public snapshot (no roles).
pub fn get_public_state(game: &Game, token: &Token) -> Result<PublicStateView, ToolError> {
    game.tokens.resolve(token).ok_or(ToolError::Unauthorized)?;
    Ok(PublicStateView {
        phase: format!("{:?}", game.phase),
        seats: game.public_seats(),
        winner: game.winner,
    })
}

/// Host or player: public log after cursor.
pub fn get_public_log(
    game: &Game,
    token: &Token,
    cursor: EventId,
) -> Result<Vec<(EventId, PublicEvent)>, ToolError> {
    game.tokens.resolve(token).ok_or(ToolError::Unauthorized)?;
    Ok(game
        .public_log
        .since(cursor)
        .into_iter()
        .map(|(id, e)| (id, e.clone()))
        .collect())
}

/// Player only: identity + private ST inbox.
pub fn get_private_state(
    game: &Game,
    token: &Token,
    private_cursor: EventId,
) -> Result<PrivateStateView, ToolError> {
    let actor = game.tokens.resolve(token).ok_or(ToolError::Unauthorized)?;
    let seat_id = match actor {
        Actor::Player { seat } => seat,
        Actor::Host => return Err(ToolError::BadRequest("host has no private player state")),
    };
    let seat = game
        .seats
        .iter()
        .find(|s| s.id == seat_id)
        .ok_or(ToolError::Game(GameError::NoSuchSeat))?;

    let visible = seat.visible_character();
    Ok(PrivateStateView {
        seat: seat_id,
        name: seat.display_name.clone(),
        alive: seat.alive,
        character_label: visible.map(|c| c.display_name().to_string()),
        team_label: visible.map(|c| format!("{:?}", c.team())),
        rules_path: visible.map(|c| c.rules_doc_path().to_string()),
        private_messages_since: game
            .private_inboxes
            .since(seat_id, private_cursor)
            .into_iter()
            .map(|(id, m)| (id, m.clone()))
            .collect(),
        awaiting_action: matches!(
            game.phase,
            Phase::FirstNight { .. } | Phase::Night { .. } | Phase::Day { .. }
        ),
    })
}

/// Player speech → public log only. No `to` / whisper args.
pub fn say(game: &mut Game, token: &Token, text: String) -> Result<EventId, ToolError> {
    let actor = game.tokens.resolve(token).ok_or(ToolError::Unauthorized)?;
    let seat = match actor {
        Actor::Player { seat } => seat,
        Actor::Host => {
            return Err(ToolError::BadRequest(
                "players use say; host uses storyteller announce",
            ));
        }
    };
    game.say(seat, text).map_err(ToolError::Game)?;
    Ok(game.public_log.since(0).last().map(|(id, _)| *id).unwrap_or(0))
}

/// Public character sheet entry (ability text path). Not secret.
pub fn get_character_rules(character: Character) -> CharacterRulesView {
    CharacterRulesView {
        name: character.display_name(),
        path: character.rules_doc_path(),
        team: format!("{:?}", character.team()),
        character_type: format!("{:?}", character.character_type()),
    }
}

pub struct CharacterRulesView {
    pub name: &'static str,
    pub path: &'static str,
    pub team: String,
    pub character_type: String,
}

/// Night choice payload — role inferred server-side from seat.
#[derive(Debug, Clone)]
pub enum NightActionPayload {
    /// No choice (info-only wake acknowledgment).
    Ack,
    PickOne { target: SeatId },
    PickTwo { a: SeatId, b: SeatId },
    PickCharacter { name: String },
}

pub fn night_action(
    game: &mut Game,
    token: &Token,
    _payload: NightActionPayload,
) -> Result<(), ToolError> {
    let actor = game.tokens.resolve(token).ok_or(ToolError::Unauthorized)?;
    let _seat = match actor {
        Actor::Player { seat } => seat,
        Actor::Host => return Err(ToolError::BadRequest("host cannot night_action")),
    };
    Err(ToolError::BadRequest(
        "night_action resolution not implemented yet",
    ))
}

pub fn nominate(game: &mut Game, token: &Token, target: SeatId) -> Result<(), ToolError> {
    let actor = game.tokens.resolve(token).ok_or(ToolError::Unauthorized)?;
    let by = match actor {
        Actor::Player { seat } => seat,
        Actor::Host => return Err(ToolError::BadRequest("host cannot nominate")),
    };
    // TODO: living, once/day, phase Nominations, Virgin, etc.
    game.public_log
        .push(PublicEvent::Nominated { by, target });
    Ok(())
}
