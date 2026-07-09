//! Opaque tokens and actor resolution.

use crate::game::SeatId;
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// Unguessable secret presented on every tool call.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Token(String);

impl Token {
    pub fn generate() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        // Sketch only — swap for CSPRNG when adding `rand`.
        Self(format!("tok_{n}_{n:x}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn from_shared(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(***)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Host,
    Player { seat: SeatId },
}

/// Per-game token → actor map (host token + one per seat).
#[derive(Debug, Default)]
pub struct TokenBook {
    by_token: HashMap<Token, Actor>,
    host: Option<Token>,
    players: HashMap<SeatId, Token>,
}

impl TokenBook {
    pub fn issue_host(&mut self) -> Token {
        let t = Token::generate();
        self.by_token.insert(t.clone(), Actor::Host);
        self.host = Some(t.clone());
        t
    }

    pub fn issue_player(&mut self, seat: SeatId) -> Token {
        let t = Token::generate();
        self.by_token.insert(t.clone(), Actor::Player { seat });
        self.players.insert(seat, t.clone());
        t
    }

    pub fn resolve(&self, token: &Token) -> Option<Actor> {
        self.by_token.get(token).copied()
    }

    pub fn host_token(&self) -> Option<&Token> {
        self.host.as_ref()
    }

    pub fn player_token(&self, seat: SeatId) -> Option<&Token> {
        self.players.get(&seat)
    }
}
