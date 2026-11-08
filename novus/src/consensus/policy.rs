use std::time::{SystemTime, UNIX_EPOCH};
use std::{error::Error as StdError, fmt};

use crate::{blockchain::Block, consensus::PoWTarget, transaction::Amount};
use crypto::{Address, Hash, HashDomain, canonical_bytes, domain, hash_bytes};

pub const TARGET_BLOCK_TIME_SECONDS: u64 = 30;
pub const FINALITY_DEPTH_BLOCKS: u64 = 10_080;
/// ASERT half-life. The first trial uses one hour for a 30-second target block.
pub const DIFFICULTY_ASERT_HALF_LIFE_SECONDS: u64 = 60 * 60;
/// Network difficulty cannot exceed 16 times the starting difficulty.
pub const MAX_DIFFICULTY_MULTIPLIER: u32 = 16;
pub const DIFFICULTY_ALGORITHM: &str =
    "asert-half-life-3600-seconds-anchor-height-1-capped-at-16x-start";
pub const MEDIAN_TIME_PAST_WINDOW: usize = 11;
pub const MAX_FUTURE_TIMESTAMP_DRIFT_SECONDS: u64 = 2 * 60 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimestampError {
    NotAfterMedianTimePast,
    TooFarInFuture,
}

impl fmt::Display for TimestampError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAfterMedianTimePast => {
                f.write_str("block timestamp is not later than median-time-past")
            }
            Self::TooFarInFuture => f.write_str("block timestamp is over two hours in the future"),
        }
    }
}

impl StdError for TimestampError {}

pub fn unix_time_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Validates median-time-past (consensus) and future-time drift (admission).
pub fn validate_timestamp(
    timestamp: u64,
    previous_timestamps: &[u64],
    now: u64,
) -> Result<(), TimestampError> {
    if !previous_timestamps.is_empty() {
        let mut timestamps = previous_timestamps.to_vec();
        timestamps.sort_unstable();
        let median = timestamps[timestamps.len() / 2];
        if timestamp <= median {
            return Err(TimestampError::NotAfterMedianTimePast);
        }
    }

    if timestamp > now.saturating_add(MAX_FUTURE_TIMESTAMP_DRIFT_SECONDS) {
        return Err(TimestampError::TooFarInFuture);
    }

    Ok(())
}

/// Lowest allowed target, corresponding to at most 16 times starting difficulty.
pub fn minimum_difficulty_target_bits() -> Option<u32> {
    let start = PoWTarget::from_compact(crate::consensus::TARGET_BITS_START)?;
    let floor = start.scale_ratio(1, MAX_DIFFICULTY_MULTIPLIER)?;
    Some(floor.to_compact())
}

/// Computes the next target using ASERT, anchored at the first non-genesis
/// block. Integer fixed-point arithmetic keeps the result deterministic.
pub fn next_asert_difficulty(
    anchor_timestamp: u64,
    parent_timestamp: u64,
    parent_height: u64,
) -> Option<u32> {
    const FRACTIONAL_BITS: i128 = 1 << 16;
    const ASERT_COEFFICIENT_1: u128 = 195_766_423_245_049;
    const ASERT_COEFFICIENT_2: u128 = 971_821_376;
    const ASERT_COEFFICIENT_3: u128 = 5_127;

    // Parent height 1 is the fixed anchor. The candidate block is one
    // expected interval beyond the parent, hence parent_height - 1 + 1.
    let expected_intervals = parent_height;
    let time_delta = i128::from(parent_timestamp) - i128::from(anchor_timestamp);
    let schedule_delta =
        i128::from(TARGET_BLOCK_TIME_SECONDS).checked_mul(i128::from(expected_intervals))?;
    let exponent = time_delta
        .checked_sub(schedule_delta)?
        .checked_mul(FRACTIONAL_BITS)?
        / i128::from(DIFFICULTY_ASERT_HALF_LIFE_SECONDS);

    // The allowed target range is only 1/16 .. 1 of the initial target.
    // Saturate outside that range before evaluating the polynomial.
    if exponent <= -4 * FRACTIONAL_BITS {
        return Some(minimum_difficulty_target_bits()?);
    }
    if exponent >= 0 {
        return Some(crate::consensus::TARGET_BITS_START);
    }

    let shifts = exponent.div_euclid(FRACTIONAL_BITS);
    let fraction = u128::try_from(exponent.rem_euclid(FRACTIONAL_BITS)).ok()?;
    let fraction_squared = fraction.checked_mul(fraction)?;
    let fraction_cubed = fraction_squared.checked_mul(fraction)?;
    let polynomial = ASERT_COEFFICIENT_1
        .checked_mul(fraction)?
        .checked_add(ASERT_COEFFICIENT_2.checked_mul(fraction_squared)?)?
        .checked_add(ASERT_COEFFICIENT_3.checked_mul(fraction_cubed)?)?
        .checked_add(1_u128 << 47)?;
    let factor = u32::try_from((polynomial >> 48).checked_add(1 << 16)?).ok()?;

    let start = PoWTarget::from_compact(crate::consensus::TARGET_BITS_START)?;
    let difficulty_floor = PoWTarget::from_compact(minimum_difficulty_target_bits()?)?;
    let scaled = start.scale_ratio(factor, 1 << 16)?;
    let shift = u32::try_from(-shifts).ok()?;
    let shifted = shift_target_right(scaled, shift);
    Some(shifted.max(difficulty_floor).to_compact())
}

fn shift_target_right(target: PoWTarget, shift: u32) -> PoWTarget {
    let mut bytes = *target.as_bytes();
    if shift >= 256 {
        return PoWTarget::from_compact(minimum_difficulty_target_bits().expect("valid floor"))
            .expect("valid floor target");
    }

    for _ in 0..shift {
        let mut carry = 0_u8;
        for byte in &mut bytes {
            let next_carry = *byte & 1;
            *byte = (*byte >> 1) | (carry << 7);
            carry = next_carry;
        }
    }

    PoWTarget::from_bytes(bytes).unwrap_or_else(|| {
        PoWTarget::from_compact(minimum_difficulty_target_bits().expect("valid floor"))
            .expect("valid floor target")
    })
}

pub fn expected_difficulty_for_height<E>(
    next_height: u64,
    _parent_difficulty: u32,
    mut sample_at: impl FnMut(u64) -> Result<(u64, u32), E>,
) -> Result<Option<u32>, E> {
    if next_height == 1 {
        return Ok(Some(crate::consensus::TARGET_BITS_START));
    }

    let (anchor_timestamp, _) = sample_at(1)?;
    let (parent_timestamp, _) = sample_at(next_height - 1)?;
    Ok(next_asert_difficulty(
        anchor_timestamp,
        parent_timestamp,
        next_height - 1,
    ))
}

pub const MIN_BLOCK_REWARD_COINS: u128 = 100;
pub const MAX_BLOCK_REWARD_COINS: u128 = 1_000;
pub const MIN_BLOCK_EMISSION: u128 = MIN_BLOCK_REWARD_COINS * Amount::ATOMIC_UNITS_PER_COIN;
pub const BLOCK_EMISSION: u128 = MAX_BLOCK_REWARD_COINS * Amount::ATOMIC_UNITS_PER_COIN;
pub const TRANSFER_TAX_PERCENT: u8 = 1;
pub const RACE_TO_GOLD_CONVERSION_TAX_PERCENT: u8 = 3;
pub const GOLD_TRANSFER_TAX_PERCENT: u8 = 1;
pub const GOLD_TO_RACE_CONVERSION_TAX_PERCENT: u8 = 1;
pub const GOLD_TO_NOVUS_CONVERSION_TAX_PERCENT: u8 = 5;

/// Derives the next block subsidy deterministically from its parent block hash.
/// This follows Dogecoin's original reward approach; miners can grind candidate
/// parent hashes, so the result is verifiable but not manipulation-resistant.
pub fn reward_for_previous_hash(previous_hash: [u8; 32]) -> Amount {
    const RANGE: u64 = (MAX_BLOCK_REWARD_COINS - MIN_BLOCK_REWARD_COINS + 1) as u64;
    let limit = u64::MAX - u64::MAX % RANGE;
    let mut input = [0_u8; 40];
    input[..32].copy_from_slice(&previous_hash);
    let mut counter = 0_u64;

    loop {
        let digest = hash_bytes(&input);
        let value = u64::from_be_bytes(digest.0[..8].try_into().expect("fixed hash size"));
        if value < limit {
            let coins = MIN_BLOCK_REWARD_COINS + u128::from(value % RANGE);
            return Amount::from_atomic(coins * Amount::ATOMIC_UNITS_PER_COIN);
        }

        input = [0; 40];
        input[..32].copy_from_slice(digest.as_bytes());
        input[32..].copy_from_slice(&counter.to_be_bytes());
        counter = counter.wrapping_add(1);
    }
}

#[cfg(test)]
mod timestamp_tests {
    use super::*;

    #[test]
    fn timestamp_must_be_strictly_after_median_of_previous_blocks() {
        let previous = [100, 110, 120, 130, 140, 150, 160, 170, 180, 190, 200];

        assert_eq!(
            validate_timestamp(150, &previous, 1_000),
            Err(TimestampError::NotAfterMedianTimePast)
        );
        assert_eq!(validate_timestamp(151, &previous, 1_000), Ok(()));
    }

    #[test]
    fn timestamp_cannot_exceed_two_hours_ahead_of_local_time() {
        let now = 1_000_000;

        assert_eq!(
            validate_timestamp(now + MAX_FUTURE_TIMESTAMP_DRIFT_SECONDS + 1, &[], now),
            Err(TimestampError::TooFarInFuture)
        );
        assert_eq!(
            validate_timestamp(now + MAX_FUTURE_TIMESTAMP_DRIFT_SECONDS, &[], now),
            Ok(())
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValidatedEmission {
    recipient: Address,
    currency: crate::transaction::Currency,
    subsidy: Amount,
    origin: Hash,
}

impl ValidatedEmission {
    pub const fn recipient(self) -> Address {
        self.recipient
    }

    pub const fn subsidy(self) -> Amount {
        self.subsidy
    }

    pub const fn currency(self) -> crate::transaction::Currency {
        self.currency
    }

    pub const fn origin(self) -> Hash {
        self.origin
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmissionError {
    MissingEmission,
    InvalidSubsidy,
    InvalidMinerAddress,
    Serialization,
}

impl fmt::Display for EmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingEmission => f.write_str("block emission is missing"),
            Self::InvalidSubsidy => f.write_str("block emission subsidy is invalid"),
            Self::InvalidMinerAddress => {
                f.write_str("block miner address has an invalid race code")
            }
            Self::Serialization => f.write_str("emission encoding failed"),
        }
    }
}

impl StdError for EmissionError {}

pub fn validate_emission(block: &Block) -> Result<ValidatedEmission, EmissionError> {
    authorize_emission(block)
}

pub(crate) fn authorize_emission(block: &Block) -> Result<ValidatedEmission, EmissionError> {
    let emission = block.emission().ok_or(EmissionError::MissingEmission)?;
    if !emission.to.has_valid_race_code() {
        return Err(EmissionError::InvalidMinerAddress);
    }

    let expected = reward_for_previous_hash(block.previous_hash().0);

    if emission.subsidy != expected {
        return Err(EmissionError::InvalidSubsidy);
    }

    let bytes = canonical_bytes(&(
        b"emission",
        block.previous_hash(),
        block.height(),
        emission.to,
        emission.subsidy,
    ))
    .map_err(|_| EmissionError::Serialization)?;

    let origin = domain(HashDomain::Emission, &bytes);

    Ok(ValidatedEmission {
        recipient: emission.to,
        currency: crate::transaction::Currency::for_race_reward(emission.to.race().into()),
        subsidy: emission.subsidy,
        origin,
    })
}

#[cfg(test)]
mod reward_tests {
    use super::*;

    #[test]
    fn previous_hash_reward_is_deterministic_and_within_inclusive_range() {
        for seed in 0_u8..=u8::MAX {
            let parent_hash = [seed; 32];
            let reward = reward_for_previous_hash(parent_hash);

            assert_eq!(reward, reward_for_previous_hash(parent_hash));
            assert!((MIN_BLOCK_EMISSION..=BLOCK_EMISSION).contains(&reward.as_atomic()));
            assert_eq!(reward.as_atomic() % Amount::ATOMIC_UNITS_PER_COIN, 0);
        }
    }
}
