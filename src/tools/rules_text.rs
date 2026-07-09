//! Load public character rules markdown from the crate docs tree.

use crate::error::ToolError;
use crate::roles::Character;
use std::fs;
use std::path::PathBuf;

/// Absolute path to the repo `docs/roles/...` markdown for `character`.
///
/// Uses `CARGO_MANIFEST_DIR` so tests and the binary resolve the same tree
/// whether run from the crate root or another working directory.
pub fn rules_markdown_path(character: Character) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(character.rules_doc_path())
}

/// Read the full rules markdown for a character (public sheet text).
pub fn load_character_rules_text(character: Character) -> Result<String, ToolError> {
    let path = rules_markdown_path(character);
    fs::read_to_string(&path).map_err(|_| ToolError::BadRequest("character rules file missing"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roles::Character;

    #[test]
    fn empath_rules_markdown_loads() {
        let text = load_character_rules_text(Character::Empath).expect("empath.md present");
        assert!(text.contains("Empath"), "got: {text}");
        assert!(text.contains("Townsfolk") || text.contains("Good"));
    }
}
