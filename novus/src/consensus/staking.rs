//! Fixed-point primitives for race-native stake-supported proof of work.
//!
//! This module only defines the deterministic economic calculations. Stake
//! positions and their activation schedule are applied by the ledger before
//! these functions may be used to validate a block.

use crate::{consensus::PoWTarget, transaction::Amount};

/// Minimum amount for one wallet to register as a war participant.
/// Additional stake has no protocol maximum.
pub const MIN_WAR_STAKE_ATOMIC: u128 = 1_000 * Amount::ATOMIC_UNITS_PER_COIN;

/// Reference stake for full mining boost: 100,000 whole race-native coins.
pub const STAKE_REFERENCE_ATOMIC: u128 = 100_000 * Amount::ATOMIC_UNITS_PER_COIN;
/// Fixed-point scale for boost and reward allocation.
pub const STAKE_BOOST_SCALE: u32 = 1_000_000;
/// Maximum stake multiplier: 2x target, equivalent to a 100% boost.
pub const STAKE_MAX_BOOST_PPM: u32 = STAKE_BOOST_SCALE;
/// Absolute representable hash target. This is intentionally above the
/// network difficulty ceiling so stake can still boost miners at launch.
pub fn stake_target_limit() -> PoWTarget {
    PoWTarget::from_bytes([u8::MAX; 32]).expect("all-ones is a valid nonzero PoW target")
}

/// Stake boost in ppm, linearly capped at [`STAKE_MAX_BOOST_PPM`].
pub fn boost_ppm(active_stake: Amount) -> u32 {
    let stake = active_stake.as_atomic().min(STAKE_REFERENCE_ATOMIC);
    let scaled = stake.saturating_mul(u128::from(STAKE_BOOST_SCALE)) / STAKE_REFERENCE_ATOMIC;
    u32::try_from(scaled).unwrap_or(STAKE_MAX_BOOST_PPM)
}

/// Applies the active stake multiplier to a network PoW target.
///
/// The resulting target is capped by the network's configured PoW limit.
pub fn effective_target(
    network_target: PoWTarget,
    active_stake: Amount,
    pow_limit: PoWTarget,
) -> Option<PoWTarget> {
    let boost = boost_ppm(active_stake);
    let numerator = STAKE_BOOST_SCALE.checked_add(boost)?;
    let adjusted = network_target.scale_ratio(numerator, STAKE_BOOST_SCALE)?;
    let capped = adjusted.min(pow_limit);
    // Headers encode compact targets, so mining and validation must use the
    // exact value represented on the wire.
    PoWTarget::from_compact(capped.to_compact())
}

/// Stake-aware block work, preserving the standard target-to-work formula.
pub fn stake_adjusted_work(target: PoWTarget) -> Option<super::Work> {
    let compact = target.to_compact();
    super::block_work(compact)
}
