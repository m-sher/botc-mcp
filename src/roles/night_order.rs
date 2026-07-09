//! Trouble Brewing night order templates (role presence filtered when building a queue).
//!
//! Order matches design spec §9.1. Concrete seats are filled by
//! [`crate::game::night`].

use crate::roles::Character;

/// First-night character wakes in sheet order (after setup/briefings).
/// Minion/Demon use true character; info/outsider use player-facing character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstNightSlot {
    Poisoner,
    Spy,
    Washerwoman,
    Librarian,
    Investigator,
    Chef,
    Empath,
    FortuneTeller,
    Butler,
}

/// Other-night character wakes in sheet order (before Dawn).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtherNightSlot {
    Poisoner,
    Monk,
    Spy,
    Imp,
    /// Only when a Ravenkeeper died to the demon tonight (usually inserted after kill).
    Ravenkeeper,
    Undertaker,
    Empath,
    FortuneTeller,
    Butler,
}

pub const FIRST_NIGHT_CHARACTER_ORDER: &[FirstNightSlot] = &[
    FirstNightSlot::Poisoner,
    FirstNightSlot::Spy,
    FirstNightSlot::Washerwoman,
    FirstNightSlot::Librarian,
    FirstNightSlot::Investigator,
    FirstNightSlot::Chef,
    FirstNightSlot::Empath,
    FirstNightSlot::FortuneTeller,
    FirstNightSlot::Butler,
];

pub const OTHER_NIGHT_CHARACTER_ORDER: &[OtherNightSlot] = &[
    OtherNightSlot::Poisoner,
    OtherNightSlot::Monk,
    OtherNightSlot::Spy,
    OtherNightSlot::Imp,
    OtherNightSlot::Ravenkeeper,
    OtherNightSlot::Undertaker,
    OtherNightSlot::Empath,
    OtherNightSlot::FortuneTeller,
    OtherNightSlot::Butler,
];

impl FirstNightSlot {
    pub fn character(self) -> Character {
        use FirstNightSlot::*;
        match self {
            Poisoner => Character::Poisoner,
            Spy => Character::Spy,
            Washerwoman => Character::Washerwoman,
            Librarian => Character::Librarian,
            Investigator => Character::Investigator,
            Chef => Character::Chef,
            Empath => Character::Empath,
            FortuneTeller => Character::FortuneTeller,
            Butler => Character::Butler,
        }
    }

    /// Evil wakes match true character; townsfolk/outsider info wakes match face.
    pub fn uses_true_character(self) -> bool {
        matches!(self, FirstNightSlot::Poisoner | FirstNightSlot::Spy)
    }
}

impl OtherNightSlot {
    pub fn character(self) -> Character {
        use OtherNightSlot::*;
        match self {
            Poisoner => Character::Poisoner,
            Monk => Character::Monk,
            Spy => Character::Spy,
            Imp => Character::Imp,
            Ravenkeeper => Character::Ravenkeeper,
            Undertaker => Character::Undertaker,
            Empath => Character::Empath,
            FortuneTeller => Character::FortuneTeller,
            Butler => Character::Butler,
        }
    }

    pub fn uses_true_character(self) -> bool {
        matches!(
            self,
            OtherNightSlot::Poisoner | OtherNightSlot::Spy | OtherNightSlot::Imp
        )
    }
}
