use novus::{
    blockchain::Block,
    common::Nonce,
    consensus::{ConsensusError, PoWTarget, calculate_work},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MiningRange {
    pub start_nonce: u64,
    pub attempts: u64,
}

pub fn mine_range(block: &mut Block, range: MiningRange) -> Result<Option<Nonce>, ConsensusError> {
    let target =
        PoWTarget::from_compact(block.target_bits()).ok_or(ConsensusError::InvalidDifficulty)?;

    mine_range_at_target(block, range, target)
}

pub fn mine_range_at_target(
    block: &mut Block,
    range: MiningRange,
    target: PoWTarget,
) -> Result<Option<Nonce>, ConsensusError> {
    for offset in 0..range.attempts {
        let nonce = Nonce(range.start_nonce.wrapping_add(offset));

        block.header.nonce = nonce;

        let hash = calculate_work(&block.header)?;

        if target.meets(&hash) {
            return Ok(Some(nonce));
        }
    }

    Ok(None)
}
