//! Seeded RNG with labeled substreams for deterministic evals.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

/// Master seeded RNG. Labeled [`Self::substream`]s are deterministic and independent of draw order.
#[derive(Debug, Clone)]
pub struct SeededRng {
    seed: u64,
    /// Reserved for game-level draws that are not ability-labeled.
    #[allow(dead_code)]
    master: ChaCha8Rng,
}

impl SeededRng {
    pub fn from_seed(seed: u64) -> Self {
        Self {
            seed,
            master: ChaCha8Rng::seed_from_u64(seed),
        }
    }

    /// Derive a fresh `ChaCha8Rng` from `seed || label` so ability draws are replay-stable.
    pub fn substream(&self, label: &str) -> ChaCha8Rng {
        let mut hasher = DefaultHasher::new();
        self.seed.hash(&mut hasher);
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
}
