//! In-memory game registry keyed by [`GameId`].

use std::collections::HashMap;

use crate::game::{Game, GameId};

/// Process-local store of live games (MCP layer wraps this in a mutex later).
#[derive(Debug)]
pub struct GameStore {
    next_id: u64,
    games: HashMap<GameId, Game>,
}

impl Default for GameStore {
    fn default() -> Self {
        Self::new()
    }
}

impl GameStore {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            games: HashMap::new(),
        }
    }

    /// Assign a fresh id, store the game, return the id.
    pub fn insert(&mut self, mut game: Game) -> GameId {
        let id = GameId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        game.id = id;
        self.games.insert(id, game);
        id
    }

    pub fn get(&self, id: GameId) -> Option<&Game> {
        self.games.get(&id)
    }

    pub fn get_mut(&mut self, id: GameId) -> Option<&mut Game> {
        self.games.get_mut(&id)
    }

    pub fn len(&self) -> usize {
        self.games.len()
    }

    pub fn is_empty(&self) -> bool {
        self.games.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Game;

    #[test]
    fn insert_assigns_monotonic_ids() {
        let mut store = GameStore::new();
        let a = Game::create(
            vec!["A".into(), "B".into(), "C".into(), "D".into(), "E".into()],
            1,
        )
        .unwrap();
        let b = Game::create(
            vec!["A".into(), "B".into(), "C".into(), "D".into(), "E".into()],
            2,
        )
        .unwrap();
        let id1 = store.insert(a.game);
        let id2 = store.insert(b.game);
        assert_eq!(id1, GameId(1));
        assert_eq!(id2, GameId(2));
        assert_eq!(store.get(id1).unwrap().seed, 1);
        assert_eq!(store.get_mut(id2).unwrap().seed, 2);
    }

    #[test]
    fn default_insert_starts_at_game_id_one() {
        let mut store = GameStore::default();
        let created = Game::create(
            vec!["A".into(), "B".into(), "C".into(), "D".into(), "E".into()],
            0,
        )
        .unwrap();
        let id = store.insert(created.game);
        assert_eq!(id, GameId(1));
        assert_eq!(store.get(id).unwrap().id, GameId(1));
    }
}
