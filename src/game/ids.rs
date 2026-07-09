//! Stable identifiers for games and circle seats.

/// Opaque game handle (host/store key).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GameId(pub u64);

/// Stable seat index in circle order (0..n-1). Neighbors wrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SeatId(pub u8);
