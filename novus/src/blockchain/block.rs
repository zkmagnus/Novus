use std::{
    collections::HashSet,
    io::{Error as IoError, ErrorKind, Read},
};

use borsh::{BorshDeserialize, BorshSerialize};

use crypto::{
    Address, BlockHash, Hash, HashDomain, MerkleHash, PreviousHash, StateRoot, canonical_bytes,
    domain,
};

use crate::{
    blockchain::merkle::{MerkleInclusionProof, merkle_root},
    error::{BlockError, CodecError},
    transaction::{Amount, Transaction},
};
use common::{Height, Nonce};

pub const MAX_BLOCK_SIZE: usize = 5 * 1024 * 1024;
pub const GENESIS_TARGET_BITS: u32 = 0x207f_ffff;
pub const GENESIS_TIMESTAMP: u64 = 0;

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Header {
    pub previous_hash: PreviousHash,
    pub merkle_root: MerkleHash,
    pub state_root: StateRoot,
    pub target_bits: u32,
    /// Unix timestamp in seconds; validated against median-time-past and future drift.
    pub timestamp: u64,
    /// Exact canonical serialized block size in bytes.
    pub block_size: u32,
    pub nonce: Nonce,
}

impl Header {
    pub const fn new(
        previous_hash: PreviousHash,
        merkle_root: MerkleHash,
        state_root: StateRoot,
        target_bits: u32,
        timestamp: u64,
        block_size: u32,
        nonce: Nonce,
    ) -> Self {
        Self {
            previous_hash,
            merkle_root,
            state_root,
            target_bits,
            timestamp,
            block_size,
            nonce,
        }
    }

    pub fn hash(&self) -> Result<BlockHash, CodecError> {
        block_header_hash(self)
    }
}

#[derive(BorshSerialize, Clone, Debug, PartialEq, Eq)]
pub struct Body {
    pub emission: Option<Emission>,
    pub transactions: Vec<Transaction>,
}

#[derive(BorshSerialize, Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub header: Header,
    pub height: Height,
    pub body: Body,
}

// Bounded decoding avoids trusting a serialized Vec length before the block-size
// limit and block-local invariants have been checked.
impl BorshDeserialize for Block {
    fn deserialize_reader<R: Read>(reader: &mut R) -> std::io::Result<Self> {
        let header = Header::deserialize_reader(reader)?;
        let height = Height::deserialize_reader(reader)?;
        let emission = Option::<Emission>::deserialize_reader(reader)?;
        let transactions = deserialize_block_transactions(reader)?;

        Ok(Self {
            header,
            height,
            body: Body {
                emission,
                transactions,
            },
        })
    }
}

fn deserialize_block_transactions<R: Read>(reader: &mut R) -> std::io::Result<Vec<Transaction>> {
    let length = u32::deserialize_reader(reader)? as usize;

    let mut transactions = Vec::new();

    transactions
        .try_reserve(length.min(64))
        .map_err(|_| IoError::new(ErrorKind::OutOfMemory, "block allocation failed"))?;

    for _ in 0..length {
        transactions.push(Transaction::deserialize_reader(reader)?);
    }

    Ok(transactions)
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Emission {
    pub to: Address,
    pub subsidy: Amount,
}

impl Emission {
    pub const fn new(to: Address, subsidy: Amount) -> Self {
        Self { to, subsidy }
    }

    pub fn hash(&self) -> Result<Hash, CodecError> {
        let bytes = canonical_bytes(self).map_err(|_| CodecError::EncodeFailed)?;
        Ok(domain(HashDomain::Emission, &bytes))
    }
}

impl Block {
    pub fn emission(&self) -> Option<&Emission> {
        self.body.emission.as_ref()
    }

    pub fn transactions(&self) -> &[Transaction] {
        &self.body.transactions
    }

    pub fn genesis() -> Result<Self, CodecError> {
        Self::from_protocol_transactions(
            Height(0),
            PreviousHash::ZERO,
            GENESIS_TARGET_BITS,
            GENESIS_TIMESTAMP,
            Nonce(0),
            None,
            vec![],
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from_protocol_transactions(
        height: Height,
        previous_hash: impl Into<PreviousHash>,
        target_bits: u32,
        timestamp: u64,
        nonce: Nonce,
        emission: Option<Emission>,
        transactions: Vec<Transaction>,
    ) -> Result<Self, CodecError> {
        let previous_hash = previous_hash.into();

        #[cfg(any(test, feature = "test-utils"))]
        let mut emission = emission;

        #[cfg(not(any(test, feature = "test-utils")))]
        let emission = emission;

        #[cfg(any(test, feature = "test-utils"))]
        if let Some(emission) = emission.as_mut() {
            emission.subsidy = crate::consensus::reward_for_previous_hash(previous_hash.0);
        }

        let merkle_root = calculate_merkle_root(emission.as_ref(), &transactions)?;

        let mut block = Self {
            header: Header::new(
                previous_hash,
                merkle_root,
                StateRoot::ZERO,
                target_bits,
                timestamp,
                0,
                nonce,
            ),
            height,
            body: Body {
                emission,
                transactions,
            },
        };

        block.refresh_block_size()?;
        Ok(block)
    }

    /// Validates only deterministic rules that depend on the block itself.
    /// Signatures, values, and state root execution remain
    /// consensus/ledger responsibilities.
    pub fn validate_structure(&self) -> Result<(), BlockError> {
        if self.is_genesis() {
            if self.body.emission.is_some() {
                return Err(BlockError::UnexpectedEmission);
            }
            if self.transaction_count() != 0 {
                return Err(BlockError::InvalidTransaction);
            }
        } else if self.body.emission.is_none() {
            return Err(BlockError::MissingEmission);
        }

        if has_duplicate_transactions(&self.body.transactions)? {
            return Err(BlockError::DuplicateTransaction);
        }

        let serialized_size = self.size()?;
        if serialized_size > MAX_BLOCK_SIZE || self.header.block_size as usize > MAX_BLOCK_SIZE {
            return Err(BlockError::BlockTooLarge);
        }
        if (self.header.block_size as usize) < serialized_size {
            return Err(BlockError::InvalidBlockSize);
        }

        if !transactions_are_structurally_valid(&self.body.transactions) {
            return Err(BlockError::InvalidTransaction);
        }

        if self.header.merkle_root
            != calculate_merkle_root(self.body.emission.as_ref(), &self.body.transactions)?
        {
            return Err(BlockError::InvalidMerkleRoot);
        }

        Ok(())
    }

    pub fn hash(&self) -> Result<BlockHash, CodecError> {
        self.header.hash()
    }

    pub const fn height(&self) -> Height {
        self.height
    }

    pub const fn previous_hash(&self) -> PreviousHash {
        self.header.previous_hash
    }

    pub fn miner_address(&self) -> Address {
        self.body
            .emission
            .as_ref()
            .map(|emission| emission.to)
            .unwrap_or(Address::new([0; crypto::ADDRESS_SIZE]))
    }

    pub const fn state_root(&self) -> StateRoot {
        self.header.state_root
    }

    pub fn set_state_root(&mut self, state_root: impl Into<StateRoot>) {
        self.header.state_root = state_root.into();
    }

    pub fn set_block_size(&mut self, block_size: u32) {
        self.header.block_size = block_size;
    }

    pub const fn target_bits(&self) -> u32 {
        self.header.target_bits
    }

    pub const fn timestamp(&self) -> u64 {
        self.header.timestamp
    }

    pub const fn block_size(&self) -> u32 {
        self.header.block_size
    }

    pub fn transaction_count(&self) -> usize {
        self.body.transactions.len()
    }

    pub fn is_genesis(&self) -> bool {
        self.height.0 == 0
    }

    pub fn size(&self) -> Result<usize, CodecError> {
        Ok(self.to_bytes()?.len())
    }

    pub fn refresh_block_size(&mut self) -> Result<(), CodecError> {
        self.header.block_size = 0;
        let size = self.size()?;
        self.header.block_size = u32::try_from(size).map_err(|_| CodecError::EncodeFailed)?;
        Ok(())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, CodecError> {
        block_bytes(self)
    }

    pub fn calculate_merkle_root(&self) -> Result<MerkleHash, CodecError> {
        calculate_merkle_root(self.body.emission.as_ref(), &self.body.transactions)
    }

    pub fn transaction_inclusion_proof(
        &self,
        transaction_index: usize,
    ) -> Result<MerkleInclusionProof, CodecError> {
        if transaction_index >= self.body.transactions.len() {
            return Err(CodecError::InvalidBlock);
        }

        let leaves = merkle_leaves(self.body.emission.as_ref(), &self.body.transactions)?;
        let leaf_index = usize::from(self.body.emission.is_some()) + transaction_index;

        MerkleInclusionProof::create(&leaves, leaf_index, HashDomain::MerkleNode)
            .ok_or(CodecError::InvalidBlock)
    }

    pub fn refresh_commitments(&mut self) -> Result<(), CodecError> {
        self.header.merkle_root = self.calculate_merkle_root()?;
        self.refresh_block_size()?;
        Ok(())
    }

    pub fn push_transaction(&mut self, transaction: Transaction) -> Result<(), CodecError> {
        self.body.transactions.push(transaction);
        self.refresh_commitments()
    }
}

fn merkle_leaves(
    emission: Option<&Emission>,
    transactions: &[Transaction],
) -> Result<Vec<Hash>, CodecError> {
    let mut leaves = Vec::with_capacity(usize::from(emission.is_some()) + transactions.len());

    if let Some(emission) = emission {
        leaves.push(emission.hash()?);
    }

    for transaction in transactions {
        leaves.push(Hash(
            transaction.id().map_err(|_| CodecError::EncodeFailed)?,
        ));
    }

    Ok(leaves)
}

fn calculate_merkle_root(
    emission: Option<&Emission>,
    transactions: &[Transaction],
) -> Result<MerkleHash, CodecError> {
    if emission.is_none() && transactions.is_empty() {
        return Ok(MerkleHash::ZERO);
    }

    let leaves = merkle_leaves(emission, transactions)?;
    merkle_root(&leaves, HashDomain::MerkleNode)
        .map(|root| MerkleHash(root.into_bytes()))
        .ok_or(CodecError::InvalidBlock)
}

fn has_duplicate_transactions(transactions: &[Transaction]) -> Result<bool, CodecError> {
    let mut seen = HashSet::with_capacity(transactions.len());

    for transaction in transactions {
        let id = Hash(transaction.id().map_err(|_| CodecError::EncodeFailed)?);
        if !seen.insert(id) {
            return Ok(true);
        }
    }

    Ok(false)
}

fn transactions_are_structurally_valid(transactions: &[Transaction]) -> bool {
    transactions
        .iter()
        .all(|transaction| transaction.intent.validate_structure().is_ok())
}

pub fn block_header_bytes(header: &Header) -> Result<Vec<u8>, CodecError> {
    canonical_bytes(header).map_err(|_| CodecError::EncodeFailed)
}

pub fn block_bytes(block: &Block) -> Result<Vec<u8>, CodecError> {
    canonical_bytes(block).map_err(|_| CodecError::EncodeFailed)
}

pub fn block_header_hash(header: &Header) -> Result<BlockHash, CodecError> {
    Ok(BlockHash(
        domain(HashDomain::Header, &block_header_bytes(header)?).into_bytes(),
    ))
}

pub fn decode_block(bytes: &[u8]) -> Result<Block, CodecError> {
    if bytes.len() > MAX_BLOCK_SIZE {
        return Err(CodecError::InvalidBlock);
    }

    let block = Block::try_from_slice(bytes).map_err(|_| CodecError::InvalidBlock)?;
    block
        .validate_structure()
        .map_err(|_| CodecError::InvalidBlock)?;
    Ok(block)
}

#[cfg(test)]
mod p3e_replay_tests {
    use super::*;

    use crypto::{SignatureScheme, SigningSeed, address_from_public_key};

    use crate::transaction::{
        AccountIntent, Amount, Transaction, TransactionAuthorization, Transfer,
    };
    use common::ChainContext;

    fn duplicate_fixture() -> Transaction {
        let seed = SigningSeed::new(SignatureScheme::Schnorr, Box::new([0x51; 32]));

        let signer = address_from_public_key(&seed.public_key());
        let chain = ChainContext::new([0x71; crypto::HASH_SIZE]);

        let intent = AccountIntent {
            sender: signer,
            race: crate::transaction::Race::Accretia,
            register_race: true,
            nonce: 0,
            transfers: vec![Transfer::new(
                signer,
                crate::transaction::Currency::Gold,
                Amount::from_atomic(1),
            )],
            conversion: None,
            fee: Amount::ZERO,
            tax: Amount::ZERO,
            message: None,
        };

        let commitment = intent
            .authorization_commitment(chain)
            .expect("authorization commitment");

        Transaction {
            intent,
            authorization: TransactionAuthorization {
                public_key: seed.public_key(),
                signature: seed.sign(commitment.as_bytes()),
            },
        }
    }

    #[test]
    fn duplicate_transaction_id_is_rejected_by_block_structure() {
        let transaction = duplicate_fixture();

        assert_eq!(
            transaction.transaction_id().unwrap(),
            transaction.clone().transaction_id().unwrap()
        );

        let miner = Address::new([0x22; crypto::ADDRESS_SIZE]);

        let block = Block::from_protocol_transactions(
            Height(1),
            PreviousHash::ZERO,
            GENESIS_TARGET_BITS,
            GENESIS_TIMESTAMP + 60,
            Nonce(0),
            Some(Emission::new(miner, Amount::from_atomic(1))),
            vec![transaction.clone(), transaction],
        )
        .expect("block construction itself may contain duplicate txs");

        assert!(matches!(
            block.validate_structure(),
            Err(BlockError::DuplicateTransaction)
        ));
    }
}
