//! Header-only validation used by synchronization.

use std::{error::Error as StdError, fmt};

use borsh::{BorshDeserialize, BorshSerialize};

use crate::{
    blockchain::{Header, MAX_BLOCK_SIZE},
    consensus::{
        ForkChoiceError, MEDIAN_TIME_PAST_WINDOW, TARGET_BITS_START, Work, block_work,
        next_asert_difficulty, unix_time_now, validate_timestamp,
    },
};
use common::Height;

use crypto::BlockHash;

pub const RECENT_HEADER_WINDOW: usize = MEDIAN_TIME_PAST_WINDOW;

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub struct HeaderAtHeight {
    pub height: Height,
    pub header: Header,
}

impl HeaderAtHeight {
    pub const fn new(height: Height, header: Header) -> Self {
        Self { height, header }
    }

    pub fn hash(&self) -> Result<BlockHash, HeaderChainError> {
        self.header
            .hash()
            .map_err(|_| HeaderChainError::Serialization)
    }
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct HeaderValidationState {
    pub height: Height,
    pub header: Header,
    pub cumulative_work: Work,
    pub difficulty_anchor: HeaderAtHeight,
    pub recent_headers: Vec<HeaderAtHeight>,
}

#[derive(Debug)]
pub enum HeaderChainError {
    EmptyHeaderChain,
    WrongGenesis,
    InvalidHeaderChain(ForkChoiceError),
    InvalidCommonAncestor,
    Serialization,
}

impl fmt::Display for HeaderChainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyHeaderChain => f.write_str("header chain is empty"),
            Self::WrongGenesis => f.write_str("header chain does not start at configured genesis"),
            Self::InvalidHeaderChain(error) => {
                write!(f, "header chain is invalid: {error}")
            }
            Self::InvalidCommonAncestor => f.write_str("header chain common ancestor is invalid"),
            Self::Serialization => f.write_str("header chain serialization failed"),
        }
    }
}

impl StdError for HeaderChainError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::InvalidHeaderChain(error) => Some(error),
            _ => None,
        }
    }
}

pub fn verify_header_chain(
    headers: &[HeaderAtHeight],
    expected_genesis: BlockHash,
) -> Result<(BlockHash, Work), HeaderChainError> {
    let first = headers.first().ok_or(HeaderChainError::EmptyHeaderChain)?;

    if first.height.0 != 0 || first.hash()? != expected_genesis {
        return Err(HeaderChainError::WrongGenesis);
    }

    let mut previous = first;
    let mut difficulty_anchor = first;
    let mut cumulative_work = Work::ZERO;
    let mut recent = vec![first.clone()];
    for current in &headers[1..] {
        validate_header_size(&current.header)?;

        if current.height.0 != previous.height.0.saturating_add(1)
            || BlockHash(current.header.previous_hash.0) != previous.hash()?
        {
            return Err(HeaderChainError::InvalidCommonAncestor);
        }

        validate_header_timestamp(&current.header, &recent)?;

        let expected = expected_header_difficulty(previous, difficulty_anchor)?;

        if current.header.target_bits != expected {
            return Err(HeaderChainError::InvalidHeaderChain(
                ForkChoiceError::InvalidDifficulty,
            ));
        }

        // Header-only sync cannot authenticate the miner's parent-state stake.
        // PoW and trusted work are deferred until the matching block body is
        // validated against the branch ledger state.
        let work = block_work(expected).ok_or(HeaderChainError::InvalidHeaderChain(
            ForkChoiceError::InvalidDifficulty,
        ))?;

        cumulative_work = cumulative_work.saturating_add(work);

        previous = current;
        if current.height.0 == 1 {
            difficulty_anchor = current;
        }
        recent.push(current.clone());

        if recent.len() > RECENT_HEADER_WINDOW {
            recent.remove(0);
        }
    }

    Ok((previous.hash()?, cumulative_work))
}

fn validate_header_size(header: &Header) -> Result<(), HeaderChainError> {
    if header.block_size == 0 || header.block_size as usize > MAX_BLOCK_SIZE {
        return Err(HeaderChainError::InvalidHeaderChain(
            ForkChoiceError::InvalidHeader,
        ));
    }

    Ok(())
}

fn validate_header_timestamp(
    header: &Header,
    previous_headers: &[HeaderAtHeight],
) -> Result<(), HeaderChainError> {
    let timestamps = previous_headers
        .iter()
        .rev()
        .take(MEDIAN_TIME_PAST_WINDOW)
        .map(|candidate| candidate.header.timestamp)
        .collect::<Vec<_>>();
    validate_timestamp(header.timestamp, &timestamps, unix_time_now()).map_err(|error| {
        HeaderChainError::InvalidHeaderChain(ForkChoiceError::InvalidTimestamp(error))
    })
}

fn expected_header_difficulty(
    previous: &HeaderAtHeight,
    difficulty_anchor: &HeaderAtHeight,
) -> Result<u32, HeaderChainError> {
    let next_height = previous.height.0.saturating_add(1);
    if next_height == 1 {
        return Ok(TARGET_BITS_START);
    }
    if difficulty_anchor.height.0 != 1 || previous.height.0 < difficulty_anchor.height.0 {
        return Err(HeaderChainError::InvalidHeaderChain(
            ForkChoiceError::InvalidDifficulty,
        ));
    }
    next_asert_difficulty(
        difficulty_anchor.header.timestamp,
        previous.header.timestamp,
        previous.height.0,
    )
    .ok_or(HeaderChainError::InvalidHeaderChain(
        ForkChoiceError::InvalidDifficulty,
    ))
}

pub fn header_validation_state(
    validated_headers: &[HeaderAtHeight],
    expected_genesis: BlockHash,
) -> Result<HeaderValidationState, HeaderChainError> {
    let (_, cumulative_work) = verify_header_chain(validated_headers, expected_genesis)?;

    let tip = validated_headers
        .last()
        .cloned()
        .ok_or(HeaderChainError::EmptyHeaderChain)?;

    let difficulty_anchor = if tip.height.0 == 0 {
        validated_headers[0].clone()
    } else {
        validated_headers
            .get(1)
            .filter(|header| header.height.0 == 1)
            .cloned()
            .ok_or(HeaderChainError::EmptyHeaderChain)?
    };

    let start = validated_headers.len().saturating_sub(RECENT_HEADER_WINDOW);

    Ok(HeaderValidationState {
        height: tip.height,
        header: tip.header,
        cumulative_work,
        difficulty_anchor,
        recent_headers: validated_headers[start..].to_vec(),
    })
}

pub fn verify_header_chain_extension(
    state: &HeaderValidationState,
    headers: &[HeaderAtHeight],
) -> Result<(BlockHash, Work), HeaderChainError> {
    let checkpoint_hash = state
        .header
        .hash()
        .map_err(|_| HeaderChainError::Serialization)?;

    if state.recent_headers.is_empty()
        || state.recent_headers.len() > RECENT_HEADER_WINDOW
        || state
            .recent_headers
            .last()
            .is_none_or(|tip| tip.header != state.header)
    {
        return Err(HeaderChainError::InvalidCommonAncestor);
    }

    let mut previous_height = state.height;
    let mut previous = state.header.clone();
    let mut previous_hash = checkpoint_hash;
    let mut cumulative_work = state.cumulative_work;
    let mut difficulty_anchor = state.difficulty_anchor.clone();
    let mut recent = state.recent_headers.clone();

    for chain_header in headers {
        let header = &chain_header.header;

        validate_header_size(header)?;

        if chain_header.height.0 != previous_height.0.saturating_add(1)
            || BlockHash(header.previous_hash.0) != previous_hash
        {
            return Err(HeaderChainError::InvalidCommonAncestor);
        }

        validate_header_timestamp(header, &recent)?;

        let previous_at_height = HeaderAtHeight::new(previous_height, previous.clone());
        let expected_difficulty =
            expected_header_difficulty(&previous_at_height, &difficulty_anchor)?;

        if header.target_bits != expected_difficulty {
            return Err(HeaderChainError::InvalidHeaderChain(
                ForkChoiceError::InvalidDifficulty,
            ));
        }

        // As above, this work is only a provisional header estimate.
        let work = block_work(expected_difficulty).ok_or(HeaderChainError::InvalidHeaderChain(
            ForkChoiceError::InvalidDifficulty,
        ))?;

        cumulative_work = cumulative_work.saturating_add(work);

        previous_hash = header.hash().map_err(|_| HeaderChainError::Serialization)?;

        previous_height = chain_header.height;
        previous = header.clone();
        if chain_header.height.0 == 1 {
            difficulty_anchor = chain_header.clone();
        }

        recent.push(chain_header.clone());

        if recent.len() > RECENT_HEADER_WINDOW {
            recent.remove(0);
        }
    }

    Ok((previous_hash, cumulative_work))
}

pub fn advance_header_validation_state(
    state: &HeaderValidationState,
    headers: &[HeaderAtHeight],
) -> Result<HeaderValidationState, HeaderChainError> {
    let (_, cumulative_work) = verify_header_chain_extension(state, headers)?;

    advanced_header_validation_state(state, headers, cumulative_work)
}

fn advanced_header_validation_state(
    state: &HeaderValidationState,
    headers: &[HeaderAtHeight],
    cumulative_work: Work,
) -> Result<HeaderValidationState, HeaderChainError> {
    let mut recent_headers = state.recent_headers.clone();

    recent_headers.extend_from_slice(headers);

    if recent_headers.len() > RECENT_HEADER_WINDOW {
        recent_headers = recent_headers[recent_headers.len() - RECENT_HEADER_WINDOW..].to_vec();
    }

    Ok(HeaderValidationState {
        height: headers
            .last()
            .map(|header| header.height)
            .unwrap_or(state.height),

        header: headers
            .last()
            .map(|header| header.header.clone())
            .unwrap_or_else(|| state.header.clone()),

        cumulative_work,

        difficulty_anchor: headers
            .iter()
            .find(|header| header.height.0 == 1)
            .cloned()
            .unwrap_or_else(|| state.difficulty_anchor.clone()),

        recent_headers,
    })
}
