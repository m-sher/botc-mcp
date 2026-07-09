//! Enumerations and static tables for the Trouble Brewing pool.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Team {
    Good,
    Evil,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CharacterType {
    Townsfolk,
    Outsider,
    Minion,
    Demon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Character {
    // Townsfolk
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
    // Outsiders
    Butler,
    Drunk,
    Recluse,
    Saint,
    // Minions
    Poisoner,
    Spy,
    ScarletWoman,
    Baron,
    // Demon
    Imp,
}

pub fn display_name(c: Character) -> &'static str {
    use Character::*;
    match c {
        Washerwoman => "Washerwoman",
        Librarian => "Librarian",
        Investigator => "Investigator",
        Chef => "Chef",
        Empath => "Empath",
        FortuneTeller => "Fortune Teller",
        Undertaker => "Undertaker",
        Monk => "Monk",
        Ravenkeeper => "Ravenkeeper",
        Virgin => "Virgin",
        Slayer => "Slayer",
        Soldier => "Soldier",
        Mayor => "Mayor",
        Butler => "Butler",
        Drunk => "Drunk",
        Recluse => "Recluse",
        Saint => "Saint",
        Poisoner => "Poisoner",
        Spy => "Spy",
        ScarletWoman => "Scarlet Woman",
        Baron => "Baron",
        Imp => "Imp",
    }
}

pub fn rules_path(c: Character) -> &'static str {
    use Character::*;
    match c {
        Washerwoman => "docs/roles/townsfolk/washerwoman.md",
        Librarian => "docs/roles/townsfolk/librarian.md",
        Investigator => "docs/roles/townsfolk/investigator.md",
        Chef => "docs/roles/townsfolk/chef.md",
        Empath => "docs/roles/townsfolk/empath.md",
        FortuneTeller => "docs/roles/townsfolk/fortune-teller.md",
        Undertaker => "docs/roles/townsfolk/undertaker.md",
        Monk => "docs/roles/townsfolk/monk.md",
        Ravenkeeper => "docs/roles/townsfolk/ravenkeeper.md",
        Virgin => "docs/roles/townsfolk/virgin.md",
        Slayer => "docs/roles/townsfolk/slayer.md",
        Soldier => "docs/roles/townsfolk/soldier.md",
        Mayor => "docs/roles/townsfolk/mayor.md",
        Butler => "docs/roles/outsiders/butler.md",
        Drunk => "docs/roles/outsiders/drunk.md",
        Recluse => "docs/roles/outsiders/recluse.md",
        Saint => "docs/roles/outsiders/saint.md",
        Poisoner => "docs/roles/minions/poisoner.md",
        Spy => "docs/roles/minions/spy.md",
        ScarletWoman => "docs/roles/minions/scarlet-woman.md",
        Baron => "docs/roles/minions/baron.md",
        Imp => "docs/roles/demons/imp.md",
    }
}
