//! Bitcoin-style double-SHA-256 proof of work.

use crate::{
    blockchain::{Header, block_header_bytes},
    consensus::{ConsensusError, PoWTarget},
};
use crypto::{PoWHash, bitcoin_double_sha256};

pub const POW_ALGORITHM: &str = "bitcoin-sha256d-header-target-v1";

pub fn calculate_work(header: &Header) -> Result<PoWHash, ConsensusError> {
    let bytes = block_header_bytes(header).map_err(|_| ConsensusError::PoWHashFailed)?;
    Ok(bitcoin_double_sha256(&bytes))
}

pub fn verify_pow(header: &Header, expected_difficulty: u32) -> Result<(), ConsensusError> {
    if PoWTarget::from_compact(expected_difficulty).is_none() {
        return Err(ConsensusError::InvalidDifficulty);
    }

    if header.target_bits != expected_difficulty {
        return Err(ConsensusError::UnexpectedDifficulty);
    }

    let target =
        PoWTarget::from_compact(expected_difficulty).ok_or(ConsensusError::InvalidDifficulty)?;
    if target.meets(&calculate_work(header)?) {
        Ok(())
    } else {
        Err(ConsensusError::InsufficientPoW)
    }
}
