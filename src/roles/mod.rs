//! Character identity and static metadata (no per-game state).

mod data;

pub use data::{Character, CharacterType, Team};

impl Character {
    pub fn team(self) -> Team {
        match self.character_type() {
            CharacterType::Townsfolk | CharacterType::Outsider => Team::Good,
            CharacterType::Minion | CharacterType::Demon => Team::Evil,
        }
    }

    pub fn character_type(self) -> CharacterType {
        use Character::*;
        match self {
            Washerwoman | Librarian | Investigator | Chef | Empath | FortuneTeller
            | Undertaker | Monk | Ravenkeeper | Virgin | Slayer | Soldier | Mayor => {
                CharacterType::Townsfolk
            }
            Butler | Drunk | Recluse | Saint => CharacterType::Outsider,
            Poisoner | Spy | ScarletWoman | Baron => CharacterType::Minion,
            Imp => CharacterType::Demon,
        }
    }

    /// Path under repo root for rules text loading.
    pub fn rules_doc_path(self) -> &'static str {
        data::rules_path(self)
    }

    pub fn display_name(self) -> &'static str {
        data::display_name(self)
    }
}

/// All 13 Trouble Brewing Townsfolk (script pool).
pub fn all_townsfolk() -> &'static [Character] {
    use Character::*;
    &[
        Washerwoman,
        Librarian,
        Investigator,
        Chef,
        Empath,
        FortuneTeller,
        Undertaker,
        Monk,
        Ravenkeeper,
        Virgin,
        Slayer,
        Soldier,
        Mayor,
    ]
}

/// All 4 Trouble Brewing Outsiders.
pub fn all_outsiders() -> &'static [Character] {
    use Character::*;
    &[Butler, Drunk, Recluse, Saint]
}

/// All 4 Trouble Brewing Minions.
pub fn all_minions() -> &'static [Character] {
    use Character::*;
    &[Poisoner, Spy, ScarletWoman, Baron]
}

/// Demon pool (Imp only on TB).
pub fn all_demons() -> &'static [Character] {
    &[Character::Imp]
}
