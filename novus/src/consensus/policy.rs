use std::{error::Error as StdError, fmt};

use crate::{blockchain::Block, consensus::PoWTarget, transaction::Amount};
use common::Height;

use crypto::{Address, Hash, HashDomain, canonical_bytes, domain};

pub const TARGET_BLOCK_TIME_SECONDS: u64 = 30;
pub const FINALITY_DEPTH_BLOCKS: u64 = 10_080;
pub const DIFFICULTY_ADJUSTMENT_WINDOW: u64 = 2_500;
pub const DIFFICULTY_TARGET_TIMESPAN_SECONDS: u64 =
    TARGET_BLOCK_TIME_SECONDS * DIFFICULTY_ADJUSTMENT_WINDOW;
pub const DIFFICULTY_TIMESPAN_CLAMP_FACTOR: u64 = 2;
pub const DIFFICULTY_ALGORITHM: &str = "bitcoin-style-target-bits-retarget";
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

pub const BLOCK_EMISSION: u128 = 10_000_000_000;
pub const TRANSFER_TAX_PERCENT: u8 = 5;

pub fn expected_emission_for_height(_height: Height) -> Amount {
    Amount::from_atomic(BLOCK_EMISSION)
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
            Self::InvalidMinerAddress => f.write_str("block miner address has an invalid race code"),
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

    let expected = expected_emission_for_height(block.height());

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
