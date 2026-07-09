//! Opaque tokens and actor resolution.

use crate::game::SeatId;
use std::collections::HashMap;
use std::fmt;

/// Unguessable secret presented on every tool call.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Token(String);

impl Token {
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_unique_and_resolve() {
        let mut book = TokenBook::default();
        let h = book.issue_host();
        let p0 = book.issue_player(SeatId(0));
        let p1 = book.issue_player(SeatId(1));
        assert_ne!(h.as_str(), p0.as_str());
        assert_ne!(h.as_str(), p1.as_str());
        assert_ne!(p0.as_str(), p1.as_str());
        assert!(matches!(book.resolve(&h), Some(Actor::Host)));
        assert!(matches!(
            book.resolve(&p0),
            Some(Actor::Player { seat: SeatId(0) })
        ));
        assert!(matches!(
            book.resolve(&p1),
            Some(Actor::Player { seat: SeatId(1) })
        ));
        assert!(book.resolve(&Token::from_shared("nope")).is_none());
        assert_eq!(book.host_token().map(Token::as_str), Some(h.as_str()));
        assert_eq!(
            book.player_token(SeatId(0)).map(Token::as_str),
            Some(p0.as_str())
        );
        assert_eq!(format!("{:?}", h), "Token(***)");
        // CSPRNG: UUID v4 hyphenated form (36 chars)
        assert_eq!(h.as_str().len(), 36);
        assert!(uuid::Uuid::parse_str(h.as_str()).is_ok());
        assert!(uuid::Uuid::parse_str(p0.as_str()).is_ok());
    }
}
