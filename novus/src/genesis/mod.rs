//! Construction and identity of the canonical chain root.

use std::{error::Error, fmt};

use crate::{
    blockchain::{Block, GENESIS_TARGET_BITS, MAX_BLOCK_SIZE},
    consensus::{
        BLOCK_EMISSION, DIFFICULTY_ADJUSTMENT_WINDOW, DIFFICULTY_ALGORITHM,
        DIFFICULTY_TARGET_TIMESPAN_SECONDS, DIFFICULTY_TIMESPAN_CLAMP_FACTOR,
        FINALITY_DEPTH_BLOCKS, POW_ALGORITHM, POW_ARGON2_ITERATIONS, POW_ARGON2_LANES,
        POW_ARGON2_MEMORY_KIB, TARGET_BITS_START, TARGET_BLOCK_TIME_SECONDS,
    },
    ledger::{Ledger, LedgerError},
};
use common::{ChainContext, Nonce};

use borsh::BorshSerialize;

use crypto::{ADDRESS_SIZE, BlockHash, HASH_SIZE, Hash, HashDomain, domain_hash};

// -----------------------------------------------------------------------------
// Mainnet
// -----------------------------------------------------------------------------

#[cfg(feature = "mainnet")]
pub const GENESIS_NONCE: u64 = 4;

#[cfg(feature = "mainnet")]
pub const EXPECTED_GENESIS_HASH: BlockHash = BlockHash([
    0x9b, 0x34, 0xca, 0x33, 0xce, 0x78, 0x30, 0x7e, 0x56, 0x9d, 0x18, 0xf2, 0x29, 0x4d, 0x7d, 0x61,
    0x81, 0x6b, 0xa4, 0x53, 0x00, 0x10, 0x2e, 0x97, 0xe3, 0xaa, 0x42, 0x84, 0x16, 0x0c, 0xec, 0x25,
]);

// -----------------------------------------------------------------------------
// Chain specification identity
// -----------------------------------------------------------------------------

/// Development baseline for the chain specification structure.
pub const CHAIN_SPEC_VERSION: u32 = 14;

#[derive(BorshSerialize)]
struct ChainSpecIdentity<'a> {
    version: u32,

    // Genesis
    genesis_hash: [u8; HASH_SIZE],

    // Proof of work
    pow_algorithm: &'a str,
    pow_memory_kib: u32,
    pow_iterations: u32,
    pow_lanes: u32,

    // Difficulty
    target_algorithm: &'a str,
    genesis_target_bits: u32,
    target_bits_start: u32,

    difficulty_adjustment_window: u64,
    target_block_time_seconds: u64,
    difficulty_target_timespan_seconds: u64,
    difficulty_timespan_clamp_factor: u64,

    // Monetary policy
    block_emission: u128,
    finality_depth_blocks: u64,
    transfer_tax_percent: u8,
    chip_war_epoch_blocks: u64,
    race_tax_min_percent: u8,
    race_tax_max_percent: u8,
    race_tax_adjustment_percentage_points: u8,
    exchange_rate_base: u128,
    exchange_rate_adjustment_percent: i8,
    exchange_rate_max_deviation_percent: i8,
    exchange_rate_policy: &'a str,
    exchange_rounding_policy: &'a str,
    chip_war_outcome_policy: &'a str,
    race_tax_application_policy: &'a str,
    block_reward_currency_policy: &'a str,
    fee_tax_vault_policy: &'a str,
    currency_decimals: u8,

    // Block / address / hash
    max_block_size: u64,
    address_size: u32,
    address_encoding: &'a str,
    hash_size: u32,

    // Native protocol identity
    transaction_format: &'a str,
}

pub fn chain_spec_hash() -> Result<Hash, GenesisError> {
    let identity = ChainSpecIdentity {
        version: CHAIN_SPEC_VERSION,

        genesis_hash: EXPECTED_GENESIS_HASH.into_bytes(),

        // Proof of work
        pow_algorithm: POW_ALGORITHM,
        pow_memory_kib: POW_ARGON2_MEMORY_KIB,
        pow_iterations: POW_ARGON2_ITERATIONS,
        pow_lanes: POW_ARGON2_LANES,

        // Difficulty
        target_algorithm: DIFFICULTY_ALGORITHM,
        genesis_target_bits: GENESIS_TARGET_BITS,
        target_bits_start: TARGET_BITS_START,

        difficulty_adjustment_window: DIFFICULTY_ADJUSTMENT_WINDOW,

        target_block_time_seconds: TARGET_BLOCK_TIME_SECONDS,

        difficulty_target_timespan_seconds: DIFFICULTY_TARGET_TIMESPAN_SECONDS,

        difficulty_timespan_clamp_factor: DIFFICULTY_TIMESPAN_CLAMP_FACTOR,

        // Emission
        block_emission: BLOCK_EMISSION,
        finality_depth_blocks: FINALITY_DEPTH_BLOCKS,
        transfer_tax_percent: crate::consensus::TRANSFER_TAX_PERCENT,
        chip_war_epoch_blocks: crate::ledger::CHIP_WAR_EPOCH_BLOCKS,
        race_tax_min_percent: crate::ledger::BASE_RACE_TAX_PERCENT,
        race_tax_max_percent: crate::ledger::MAX_RACE_TAX_PERCENT,
        race_tax_adjustment_percentage_points: 1,
        exchange_rate_base: crate::transaction::RACE_CURRENCY_PER_GOLD,
        exchange_rate_adjustment_percent: crate::ledger::EXCHANGE_RATE_STEP_PERCENT,
        exchange_rate_max_deviation_percent: crate::ledger::MAX_EXCHANGE_RATE_DEVIATION_PERCENT,
        exchange_rate_policy: "chip-war-winner-lower-loser-raise-failure-unchanged-clamp-v1",
        exchange_rounding_policy: "rational-floor-to-smallest-unit-v1",
        chip_war_outcome_policy: "unique-scores-high-wins-low-loses-middle-failure-any-tie-all-failure-v1",
        race_tax_application_policy: "race-native-and-conversion-effective-tax-universal-currency-base-tax-v1",
        block_reward_currency_policy: "miner-race-native-currency-v1",
        fee_tax_vault_policy: "sender-race-native-fee-added-to-tax-vault-claimed-by-matching-race-miner-v1",
        currency_decimals: crate::transaction::DECIMALS,

        max_block_size: MAX_BLOCK_SIZE as u64,
        address_size: ADDRESS_SIZE as u32,

        address_encoding: "novus-race-bound-v2-base62-sha3-checksum",

        hash_size: HASH_SIZE as u32,

        transaction_format: "account-nonce-four-currencies-amount-atomic-decimals-3-race-native-fee-conversion-quote-v1",
    };

    let bytes = crypto::canonical_bytes(&identity).map_err(GenesisError::Encoding)?;

    Ok(domain_hash(HashDomain::ChainSpec, &bytes))
}

// -----------------------------------------------------------------------------
// Genesis
// -----------------------------------------------------------------------------

pub fn genesis_block() -> Result<Block, GenesisError> {
    let mut block = Block::genesis().map_err(GenesisError::Encoding)?;

    block.header.nonce = Nonce(GENESIS_NONCE);

    if block.hash().map_err(GenesisError::Encoding)? != EXPECTED_GENESIS_HASH {
        return Err(GenesisError::HashMismatch);
    }

    Ok(block)
}

pub fn genesis_hash() -> Result<BlockHash, GenesisError> {
    genesis_block()?.hash().map_err(GenesisError::Encoding)
}

pub fn chain_context() -> Result<ChainContext, GenesisError> {
    Ok(ChainContext::new(genesis_hash()?.into_bytes()))
}

pub fn genesis_ledger() -> Result<Ledger, GenesisError> {
    let mut ledger = Ledger::new();
    let block = genesis_block()?;

    crate::consensus::apply_genesis(&mut ledger, block, EXPECTED_GENESIS_HASH)
        .map_err(GenesisError::Ledger)?;

    Ok(ledger)
}

// -----------------------------------------------------------------------------
// Errors
// -----------------------------------------------------------------------------

#[derive(Debug)]
pub enum GenesisError {
    Encoding(crypto::CodecError),
    HashMismatch,
    Ledger(LedgerError),
}

impl fmt::Display for GenesisError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encoding(error) => {
                write!(formatter, "genesis encoding failed: {error}")
            }

            Self::HashMismatch => {
                formatter.write_str("constructed genesis does not match frozen chain identity")
            }

            Self::Ledger(error) => {
                write!(formatter, "genesis ledger failed: {error}")
            }
        }
    }
}

impl Error for GenesisError {}
