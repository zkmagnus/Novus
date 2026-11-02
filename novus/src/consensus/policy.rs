use std::time::{SystemTime, UNIX_EPOCH};
use std::{error::Error as StdError, fmt};

use crate::{blockchain::Block, consensus::PoWTarget, transaction::Amount};
use crypto::{Address, Hash, HashDomain, canonical_bytes, domain, hash_bytes};

pub const TARGET_BLOCK_TIME_SECONDS: u64 = 30;
pub const FINALITY_DEPTH_BLOCKS: u64 = 10_080;
/// Difficulty retargets at the first block after each completed Chip War epoch.
pub const DIFFICULTY_ADJUSTMENT_WINDOW: u64 = crate::ledger::CHIP_WAR_EPOCH_BLOCKS;
pub const DIFFICULTY_TARGET_TIMESPAN_SECONDS: u64 =
    TARGET_BLOCK_TIME_SECONDS * DIFFICULTY_ADJUSTMENT_WINDOW;
pub const DIFFICULTY_TIMESPAN_CLAMP_FACTOR: u64 = 2;
pub const DIFFICULTY_ALGORITHM: &str = "chip-war-epoch-bitcoin-style-target-bits-retarget";
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

pub const fn is_difficulty_adjustment_boundary(next_height: u64) -> bool {
    next_height > 1 && (next_height - 1).is_multiple_of(DIFFICULTY_ADJUSTMENT_WINDOW)
}

pub fn next_difficulty_from_timestamps(
    previous_target_bits: u32,
    first_timestamp: u64,
    last_timestamp: u64,
) -> Option<u32> {
    let previous = PoWTarget::from_compact(previous_target_bits)?;
    let pow_limit = PoWTarget::from_compact(crate::consensus::TARGET_BITS_START)?;
    let expected = DIFFICULTY_TARGET_TIMESPAN_SECONDS;
    let minimum = expected / DIFFICULTY_TIMESPAN_CLAMP_FACTOR;
    let maximum = expected.checked_mul(DIFFICULTY_TIMESPAN_CLAMP_FACTOR)?;
    let actual = last_timestamp
        .saturating_sub(first_timestamp)
        .clamp(minimum, maximum);

    let numerator = u32::try_from(actual).ok()?;
    let denominator = u32::try_from(expected).ok()?;
    let next = previous.scale_ratio(numerator, denominator)?;
    let next = if next > pow_limit { pow_limit } else { next };

    Some(next.to_compact())
}

pub fn expected_difficulty_from_timestamps(
    next_height: u64,
    parent_difficulty: u32,
    first_timestamp: u64,
    last_timestamp: u64,
) -> Option<u32> {
    if next_height == 1 {
        return Some(crate::consensus::TARGET_BITS_START);
    }

    if !is_difficulty_adjustment_boundary(next_height) {
        return Some(parent_difficulty);
    }

    next_difficulty_from_timestamps(parent_difficulty, first_timestamp, last_timestamp)
}

pub fn expected_difficulty_for_height<E>(
    next_height: u64,
    parent_difficulty: u32,
    mut timestamp_at: impl FnMut(u64) -> Result<u64, E>,
) -> Result<Option<u32>, E> {
    if next_height == 1 {
        return Ok(Some(crate::consensus::TARGET_BITS_START));
    }

    if !is_difficulty_adjustment_boundary(next_height) {
        return Ok(Some(parent_difficulty));
    }

    let first_height = next_height - DIFFICULTY_ADJUSTMENT_WINDOW;

    let last_height = next_height - 1;

    let first_timestamp = timestamp_at(first_height)?;

    let last_timestamp = timestamp_at(last_height)?;

    Ok(next_difficulty_from_timestamps(
        parent_difficulty,
        first_timestamp,
        last_timestamp,
    ))
}

pub const MIN_BLOCK_REWARD_COINS: u128 = 100;
pub const MAX_BLOCK_REWARD_COINS: u128 = 1_000;
pub const MIN_BLOCK_EMISSION: u128 = MIN_BLOCK_REWARD_COINS * Amount::ATOMIC_UNITS_PER_COIN;
pub const BLOCK_EMISSION: u128 = MAX_BLOCK_REWARD_COINS * Amount::ATOMIC_UNITS_PER_COIN;
pub const TRANSFER_TAX_PERCENT: u8 = 1;
pub const RACE_TO_GOLD_CONVERSION_TAX_PERCENT: u8 = 5;
pub const GOLD_TRANSFER_TAX_PERCENT: u8 = 2;
pub const GOLD_TO_RACE_CONVERSION_TAX_PERCENT: u8 = 5;
pub const GOLD_TO_NOVUS_CONVERSION_TAX_PERCENT: u8 = 10;

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
mod difficulty_epoch_tests {
    use super::*;

    #[test]
    fn difficulty_retargets_at_the_start_of_each_new_chip_war_epoch() {
        let epoch = crate::ledger::CHIP_WAR_EPOCH_BLOCKS;

        assert_eq!(DIFFICULTY_ADJUSTMENT_WINDOW, epoch);
        assert_eq!(DIFFICULTY_TARGET_TIMESPAN_SECONDS, 3 * 24 * 60 * 60);
        assert!(!is_difficulty_adjustment_boundary(epoch));
        assert!(is_difficulty_adjustment_boundary(epoch + 1));
        assert!(!is_difficulty_adjustment_boundary(epoch + 2));
        assert!(is_difficulty_adjustment_boundary(epoch * 2 + 1));
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
