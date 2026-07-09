//! Seeded RNG with labeled substreams for deterministic evals.

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

/// Master seeded RNG. Labeled [`Self::substream`]s are deterministic and independent of draw order.
///
/// Substreams mix **seed + secret_salt + label** so public labels alone cannot reproduce draws
/// without the host-only salt.
///
/// Mixing uses a **version-stable FNV-1a** (not `std::hash::DefaultHasher`, which is not stable
/// across Rust versions).
#[derive(Debug, Clone)]
pub struct SeededRng {
    seed: u64,
    /// Per-game secret; host-only. Mixed into every substream.
    salt: u64,
    /// Reserved for game-level draws that are not ability-labeled.
    #[allow(dead_code)]
    master: ChaCha8Rng,
}

/// FNV-1a 64-bit offset basis.
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
/// FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x100000001b3;

/// Version-stable mixer: FNV-1a over little-endian seed, salt, then label bytes.
///
/// Used for substream derivation so evals replay across compiler versions.
pub fn mix(seed: u64, salt: u64, label: &str) -> u64 {
    let mut hash = FNV_OFFSET;
    for b in seed.to_le_bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    for b in salt.to_le_bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    for b in label.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
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
        ChaCha8Rng::seed_from_u64(mix(self.seed, self.salt, label))
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

    #[test]
    fn mix_is_stable_known_vector() {
        // Fixed golden so a future accidental hasher swap fails loudly.
        // mix(1, 2, "setup") = FNV-1a over LE seed, salt, then label bytes.
        let h = mix(1, 2, "setup");
        assert_eq!(h, 0x7351_1a5b_7da1_f833);
        assert_eq!(h, mix(1, 2, "setup"));
        assert_ne!(h, mix(1, 3, "setup"));
        assert_ne!(h, mix(1, 2, "setup2"));
        assert_ne!(h, 0);
        assert_ne!(h, FNV_OFFSET);
    }
}
