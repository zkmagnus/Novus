use borsh::{BorshDeserialize, BorshSerialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize)]
pub struct ChainContext {
    pub genesis_hash: [u8; 32],
}

impl ChainContext {
    pub const fn new(genesis_hash: [u8; 32]) -> Self {
        Self { genesis_hash }
    }
}
