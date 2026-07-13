//! Public gameplay rules docs (not player secrets). Allowlisted markdown under `docs/`.

use crate::error::ToolError;
use crate::roles::{
    all_demons, all_minions, all_outsiders, all_townsfolk, Character, CharacterType, Team,
};
use std::fs;
use std::path::PathBuf;

/// One public rules topic players may retrieve individually.
#[derive(Debug, Clone, Copy)]
pub struct RulesTopic {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// Path relative to crate root.
    pub path: &'static str,
}

/// Allowlisted topics (no architecture / MCP / design internals).
pub const RULES_TOPICS: &[RulesTopic] = &[
    RulesTopic {
        id: "overview",
        title: "Overview",
        description: "What Trouble Brewing is, teams, and basic player rules",
        path: "docs/overview.md",
    },
    RulesTopic {
        id: "gameplay_loop",
        title: "Gameplay loop",
        description: "Night and day structure for a full game",
        path: "docs/gameplay-loop.md",
    },
    RulesTopic {
        id: "setup",
        title: "Setup",
        description: "Player counts, bag composition, and setup steps",
        path: "docs/setup.md",
    },
    RulesTopic {
        id: "night_order",
        title: "Night order",
        description: "First night and other-night wake order",
        path: "docs/night-order.md",
    },
    RulesTopic {
        id: "voting",
        title: "Voting and nominations",
        description: "Nominations, vote thresholds, execution, ghost votes",
        path: "docs/voting-and-nominations.md",
    },
    RulesTopic {
        id: "win_conditions",
        title: "Win conditions",
        description: "How Good and Evil win",
        path: "docs/win-conditions.md",
    },
    RulesTopic {
        id: "death_and_ghosts",
        title: "Death and ghost votes",
        description: "What dead players can do",
        path: "docs/death-and-ghosts.md",
    },
    RulesTopic {
        id: "states",
        title: "States",
        description: "Drunk, poisoned, registration, and related states",
        path: "docs/states.md",
    },
    RulesTopic {
        id: "abilities",
        title: "Ability resolution",
        description: "How abilities resolve and Storyteller vs player choices",
        path: "docs/abilities-rules.md",
    },
    RulesTopic {
        id: "characters",
        title: "Character pool",
        description: "Index of all Trouble Brewing characters and doc paths",
        path: "docs/characters.md",
    },
    RulesTopic {
        id: "character_types",
        title: "Character types",
        description: "Townsfolk, Outsiders, Minions, Demon (no individual abilities)",
        path: "docs/roles/character-types.md",
    },
    RulesTopic {
        id: "glossary",
        title: "Glossary",
        description: "Common terms",
        path: "docs/glossary.md",
    },
    RulesTopic {
        id: "storyteller",
        title: "Storyteller",
        description: "What the Storyteller (moderator) does; public knowledge",
        path: "docs/storyteller.md",
    },
];

pub fn list_rules_topics() -> Vec<&'static RulesTopic> {
    RULES_TOPICS.iter().collect()
}

pub fn find_rules_topic(id: &str) -> Option<&'static RulesTopic> {
    let key = id.trim().to_ascii_lowercase().replace('-', "_");
    RULES_TOPICS
        .iter()
        .find(|t| t.id == key || t.id.replace('_', "") == key.replace('_', ""))
}

fn docs_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Load one allowlisted rules topic by id.
pub fn load_rules_topic(id: &str) -> Result<(&'static RulesTopic, String), ToolError> {
    let topic = find_rules_topic(id).ok_or(ToolError::BadRequest(
        "unknown rules topic; call list_rules_topics for ids",
    ))?;
    let text = fs::read_to_string(docs_path(topic.path))
        .map_err(|_| ToolError::BadRequest("rules topic file missing"))?;
    Ok((topic, text))
}

#[derive(Debug, Clone)]
pub struct CharacterListEntry {
    pub name: &'static str,
    pub character_type: &'static str,
    pub team: &'static str,
    pub rules_path: &'static str,
}

fn type_label(t: CharacterType) -> &'static str {
    match t {
        CharacterType::Townsfolk => "Townsfolk",
        CharacterType::Outsider => "Outsider",
        CharacterType::Minion => "Minion",
        CharacterType::Demon => "Demon",
    }
}

fn team_label(t: Team) -> &'static str {
    match t {
        Team::Good => "Good",
        Team::Evil => "Evil",
    }
}

fn entry(c: Character) -> CharacterListEntry {
    CharacterListEntry {
        name: c.display_name(),
        character_type: type_label(c.character_type()),
        team: team_label(c.team()),
        rules_path: c.rules_doc_path(),
    }
}

/// Full Trouble Brewing character pool (public knowledge).
pub fn list_characters() -> Vec<CharacterListEntry> {
    let mut out = Vec::new();
    for c in all_townsfolk() {
        out.push(entry(*c));
    }
    for c in all_outsiders() {
        out.push(entry(*c));
    }
    for c in all_minions() {
        out.push(entry(*c));
    }
    for c in all_demons() {
        out.push(entry(*c));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gameplay_loop_topic_loads() {
        let (t, text) = load_rules_topic("gameplay_loop").expect("topic");
        assert_eq!(t.id, "gameplay_loop");
        assert!(text.to_lowercase().contains("night") || text.contains("Day"));
    }

    #[test]
    fn unknown_topic_errors() {
        assert!(load_rules_topic("not_a_real_topic").is_err());
    }

    #[test]
    fn list_characters_covers_tb_pool() {
        let list = list_characters();
        assert!(list.len() >= 20);
        assert!(list.iter().any(|c| c.name == "Empath"));
        assert!(list.iter().any(|c| c.name == "Imp"));
    }
}
