//! Seeded RNG with labeled substreams for deterministic evals.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

/// Master seeded RNG. Labeled [`Self::substream`]s are deterministic and independent of draw order.
///
/// Substreams mix **seed + secret_salt + label** so public labels alone cannot reproduce draws
/// without the host-only salt.
#[derive(Debug, Clone)]
pub struct SeededRng {
    seed: u64,
    /// Per-game secret; host-only. Mixed into every substream.
    salt: u64,
    /// Reserved for game-level draws that are not ability-labeled.
    #[allow(dead_code)]
    master: ChaCha8Rng,
}

impl SeededRng {
    /// Build with `salt = 0` (tests / direct bag sampling that only care about `seed`).
    pub fn from_seed(seed: u64) -> Self {
        Self::from_seed_and_salt(seed, 0)
    }

    /// Build with an explicit per-game secret salt.
    pub fn from_seed_and_salt(seed: u64, salt: u64) -> Self {
        Self {
            seed,
            salt,
            master: ChaCha8Rng::seed_from_u64(seed ^ salt.rotate_left(17)),
        }
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn salt(&self) -> u64 {
        self.salt
    }

    /// Derive a fresh `ChaCha8Rng` from `seed || salt || label` so ability draws are replay-stable
    /// for a given (seed, salt) pair but not from public labels alone.
    pub fn substream(&self, label: &str) -> ChaCha8Rng {
        let mut hasher = DefaultHasher::new();
        self.seed.hash(&mut hasher);
        self.salt.hash(&mut hasher);
        label.hash(&mut hasher);
        ChaCha8Rng::seed_from_u64(hasher.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

    #[test]
    fn same_seed_same_substream_bytes_unit() {
        let a = SeededRng::from_seed(7);
        let b = SeededRng::from_seed(7);
        let x: u64 = a.substream("setup").gen();
        let y: u64 = b.substream("setup").gen();
        assert_eq!(x, y);
    }

    #[test]
    fn salt_changes_substream_output() {
        let a = SeededRng::from_seed_and_salt(7, 0);
        let b = SeededRng::from_seed_and_salt(7, 1);
        let x: u64 = a.substream("setup").gen();
        let y: u64 = b.substream("setup").gen();
        assert_ne!(x, y);
    }
}
