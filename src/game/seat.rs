use crate::roles::Character;

/// Stable seat index in circle order (0..n-1). Neighbors wrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SeatId(pub u8);

#[derive(Debug, Clone)]
pub struct Seat {
    pub id: SeatId,
    pub display_name: String,
    pub alive: bool,
    /// Dead players: one remaining vote for the whole game.
    pub ghost_vote_available: bool,
    /// True character in the Grimoire.
    pub true_character: Option<Character>,
    /// Drunk face; `None` if player knows their true character label.
    pub believed_character: Option<Character>,
    pub poisoned: bool,
    pub is_drunk_outsider: bool,
    pub monk_protected_tonight: bool,
    pub slayer_used: bool,
    pub virgin_ability_used: bool,
    /// Butler master for today's voting restriction.
    pub butler_master: Option<SeatId>,
}

impl Seat {
    pub fn new(id: SeatId, display_name: impl Into<String>) -> Self {
        Self {
            id,
            display_name: display_name.into(),
            alive: true,
            ghost_vote_available: true,
            true_character: None,
            believed_character: None,
            poisoned: false,
            is_drunk_outsider: false,
            monk_protected_tonight: false,
            slayer_used: false,
            virgin_ability_used: false,
            butler_master: None,
        }
    }

    /// Character string shown in private state (Drunk sees face).
    pub fn visible_character(&self) -> Option<Character> {
        self.believed_character.or(self.true_character)
    }
}
