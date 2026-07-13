//! MCP tool handlers (sketch).
//!
//! Transport (JSON-RPC / rmcp / etc.) is not wired yet. These functions are the
//! semantic API the MCP layer should call after deserializing arguments.

mod rules_docs;
mod rules_text;
mod views;

pub use rules_docs::{
    list_characters as list_character_pool, list_rules_topics, load_rules_topic,
    CharacterListEntry, RulesTopic,
};
pub use rules_text::{load_character_rules_text, rules_markdown_path};
pub use views::{
    AwaitingView, CharacterRulesView, HostDecisionView, HostPendingView, HostSeatView,
    HostStateView, PrivateStateView, PublicStateView,
};

use crate::auth::{Actor, Token};
use crate::comms::{EventId, PublicEvent};
use crate::error::GameError;
use crate::game::{
    CreateGameResult, Game, GameId, HostDecision, NightActionPayload, SeatId, StartOpts,
};
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
///
/// `secret_salt`: `None` → CSPRNG salt (production); `Some(s)` → deterministic replay with `seed`.
pub fn create_game(
    store: &mut GameStore,
    names: Vec<String>,
    seed: u64,
    secret_salt: Option<u64>,
) -> Result<CreateGameResponse, ToolError> {
    let CreateGameResult {
        game,
        host_token,
        player_tokens,
    } = match secret_salt {
        Some(salt) => Game::create_with_salt(names, seed, salt)?,
        None => Game::create(names, seed)?,
    };

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
    create_game_in_memory_with_salt(names, seed, None)
}

/// Like [`create_game_in_memory`] with optional fixed salt.
pub fn create_game_in_memory_with_salt(
    names: Vec<String>,
    seed: u64,
    secret_salt: Option<u64>,
) -> CreateGameResponse {
    let mut store = GameStore::new();
    create_game(&mut store, names, seed, secret_salt)
        .expect("create_game_in_memory: valid player count 5–15")
}

/// Host locks lobby, assigns bag, enters First Night (and runs [`Game::night_tick`]).
pub fn start_game(game: &mut Game, host: &Token, opts: StartOpts) -> Result<(), ToolError> {
    game.start_game(host, opts).map_err(ToolError::from)
}

/// Host or player: public snapshot (no roles, **no** pending night seat).
///
/// Night order leakage: pending wake seat is **not** returned here. The acting
/// seat sees it via [`get_private_state`]; the host via [`get_host_state`].
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
    let awaiting = game.pending_night.as_ref().and_then(|p| {
        if p.seat == seat_id {
            Some(AwaitingView {
                action: "night",
                prompt: p.prompt.clone(),
                schema: p.schema.clone(),
            })
        } else {
            None
        }
    });
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
        awaiting_action: awaiting.is_some(),
        awaiting,
    })
}

/// Host only: full grimoire (true roles), pending wake, seed.
pub fn get_host_state(game: &Game, token: &Token) -> Result<HostStateView, ToolError> {
    match game.tokens.resolve(token) {
        Some(Actor::Host) => {}
        Some(Actor::Player { .. }) | None => return Err(ToolError::Unauthorized),
    }

    let seats = game
        .seats
        .iter()
        .map(|s| HostSeatView {
            seat_id: s.id,
            name: s.display_name.clone(),
            alive: s.alive,
            ghost_vote_available: s.ghost_vote_available,
            true_character: s.true_character.map(|c| c.display_name()),
            believed_character: s.believed_character.map(|c| c.display_name()),
            poisoned: s.poisoned,
            is_drunk_outsider: s.is_drunk_outsider,
            monk_protected_tonight: s.monk_protected_tonight,
            slayer_used: s.slayer_used,
            virgin_ability_used: s.virgin_ability_used,
            butler_master: s.butler_master,
        })
        .collect();

    let pending = game.pending_night.as_ref().map(|p| HostPendingView {
        seat_id: p.seat,
        prompt: p.prompt.clone(),
        schema: p.schema.clone(),
        step_debug: format!("{:?}", p.step),
    });

    let pending_host = game
        .pending_host
        .as_ref()
        .map(HostDecisionView::from_pending);

    Ok(HostStateView {
        seed: game.seed,
        secret_salt: game.secret_salt,
        phase: format!("{:?}", game.phase),
        seats,
        pending,
        pending_host,
        registration_mode: format!("{:?}", game.registration_mode),
        st_choice_mode: format!("{:?}", game.st_choice_mode),
        host_lie_queue_len: game.host_lie_queue.len(),
        red_herring: game.red_herring,
        demon_bluffs: game.demon_bluffs.iter().map(|c| c.display_name()).collect(),
        winner: game.winner,
    })
}

/// Player speech → public log only. No `to` / whisper args.
pub fn say(game: &mut Game, token: &Token, text: String) -> Result<EventId, ToolError> {
    let actor = game.tokens.resolve(token).ok_or(ToolError::Unauthorized)?;
    let seat = match actor {
        Actor::Player { seat } => seat,
        Actor::Host => {
            return Err(ToolError::BadRequest(
                "players use say; host uses st_announce",
            ));
        }
    };
    game.say(seat, text).map_err(ToolError::Game)?;
    Ok(game
        .public_log
        .since(0)
        .last()
        .map(|(id, _)| *id)
        .unwrap_or(0))
}

/// Host: public storyteller announcement.
pub fn st_announce(game: &mut Game, host: &Token, text: String) -> Result<EventId, ToolError> {
    match game.tokens.resolve(host) {
        Some(Actor::Host) => {}
        _ => return Err(ToolError::Unauthorized),
    }
    game.st_announce(text);
    Ok(game
        .public_log
        .since(0)
        .last()
        .map(|(id, _)| *id)
        .unwrap_or(0))
}

/// Public character sheet entry (ability text loaded from docs). Not secret.
pub fn get_character_rules(character: Character) -> Result<CharacterRulesView, ToolError> {
    let text = load_character_rules_text(character)?;
    Ok(CharacterRulesView {
        name: character.display_name(),
        path: character.rules_doc_path(),
        team: format!("{:?}", character.team()),
        character_type: format!("{:?}", character.character_type()),
        text,
    })
}

/// Public list of rules topics (ids + titles). No game secrets.
pub fn get_rules_topics() -> Vec<&'static RulesTopic> {
    list_rules_topics()
}

/// Load one public rules topic by id (e.g. `gameplay_loop`).
pub fn get_rules_topic(topic_id: &str) -> Result<(&'static RulesTopic, String), ToolError> {
    load_rules_topic(topic_id)
}

/// Full TB character pool (names, types, paths). No in-play secrets.
pub fn get_character_list() -> Vec<CharacterListEntry> {
    list_character_pool()
}

pub fn night_action(
    game: &mut Game,
    token: &Token,
    payload: NightActionPayload,
) -> Result<(), ToolError> {
    game.night_action(token, payload).map_err(ToolError::from)
}

/// Host only: apply default for pending wake (or pending host decision) and continue night_tick.
pub fn skip_night_action(game: &mut Game, host: &Token) -> Result<(), ToolError> {
    game.skip_night_action(host).map_err(ToolError::from)
}

/// Host only: resolve Mayor bounce / starpass pick.
pub fn host_decide(game: &mut Game, host: &Token, decision: HostDecision) -> Result<(), ToolError> {
    game.host_decide(host, decision).map_err(ToolError::from)
}

/// Host only: enqueue free-text false info for the next disabled info result (FIFO).
pub fn host_queue_lie(game: &mut Game, host: &Token, text: String) -> Result<(), ToolError> {
    game.host_queue_lie(host, text).map_err(ToolError::from)
}

/// Host: Discussion → Nominations.
pub fn open_nominations(game: &mut Game, host: &Token) -> Result<(), ToolError> {
    game.open_nominations(host).map_err(ToolError::from)
}

/// Player: nominate a living target (once per day each way).
pub fn nominate(game: &mut Game, token: &Token, target: SeatId) -> Result<(), ToolError> {
    let actor = game.tokens.resolve(token).ok_or(ToolError::Unauthorized)?;
    let by = match actor {
        Actor::Player { seat } => seat,
        Actor::Host => return Err(ToolError::BadRequest("host cannot nominate")),
    };
    game.nominate(by, target).map_err(ToolError::from)
}

/// Player: cast yes/no on the open nomination (`nominee` must match current).
pub fn vote(
    game: &mut Game,
    token: &Token,
    nominee: SeatId,
    support: bool,
) -> Result<(), ToolError> {
    let actor = game.tokens.resolve(token).ok_or(ToolError::Unauthorized)?;
    let seat = match actor {
        Actor::Player { seat } => seat,
        Actor::Host => return Err(ToolError::BadRequest("host cannot vote")),
    };
    game.vote(seat, nominee, support).map_err(ToolError::from)
}

/// Dead player: abstain without spending the ghost vote (enables auto-close).
pub fn pass_vote(game: &mut Game, token: &Token) -> Result<(), ToolError> {
    let actor = game.tokens.resolve(token).ok_or(ToolError::Unauthorized)?;
    let seat = match actor {
        Actor::Player { seat } => seat,
        Actor::Host => return Err(ToolError::BadRequest("host cannot pass_vote")),
    };
    game.pass_vote(seat).map_err(ToolError::from)
}

/// Host: close the current vote window (also auto-runs when living + ghost-holders responded).
pub fn close_vote(game: &mut Game, host: &Token) -> Result<(), ToolError> {
    game.close_vote(host).map_err(ToolError::from)
}

/// Host: execute the vote leader (if any), then begin the next night if ongoing.
pub fn end_nominations(game: &mut Game, host: &Token) -> Result<(), ToolError> {
    game.end_nominations(host).map_err(ToolError::from)
}

/// Player day ability payload (Slayer slay for TB).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DayActionPayload {
    Slay { target: SeatId },
}

/// Player: once-per-game day ability (Slayer).
pub fn day_action(
    game: &mut Game,
    token: &Token,
    payload: DayActionPayload,
) -> Result<(), ToolError> {
    let actor = game.tokens.resolve(token).ok_or(ToolError::Unauthorized)?;
    let seat = match actor {
        Actor::Player { seat } => seat,
        Actor::Host => return Err(ToolError::BadRequest("host cannot use day_action")),
    };
    match payload {
        DayActionPayload::Slay { target } => {
            game.day_action_slay(seat, target).map_err(ToolError::from)
        }
    }
}
