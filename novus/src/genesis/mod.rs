//! Construction and identity of the canonical chain root.

use std::{error::Error, fmt};

use crate::{
    blockchain::{Block, GENESIS_TARGET_BITS, MAX_BLOCK_SIZE},
    consensus::{
        BLOCK_EMISSION, DIFFICULTY_ADJUSTMENT_WINDOW, DIFFICULTY_ALGORITHM,
        DIFFICULTY_TARGET_TIMESPAN_SECONDS, DIFFICULTY_TIMESPAN_CLAMP_FACTOR,
        FINALITY_DEPTH_BLOCKS, MIN_BLOCK_EMISSION, POW_ALGORITHM, TARGET_BITS_START,
        TARGET_BLOCK_TIME_SECONDS,
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
    0x06, 0x5d, 0x5c, 0xf7, 0x31, 0x03, 0x02, 0x40, 0x95, 0x62, 0xba, 0x9e, 0xeb, 0x2d, 0xf0, 0x98,
    0xd2, 0xa7, 0xc9, 0x63, 0xe5, 0x87, 0xe4, 0x6f, 0x78, 0x16, 0x70, 0xc3, 0x88, 0x8d, 0xcc, 0xf4,
]);

// -----------------------------------------------------------------------------
// Chain specification identity
// -----------------------------------------------------------------------------

/// Development baseline for the chain specification structure.
pub const CHAIN_SPEC_VERSION: u32 = 1;

#[derive(BorshSerialize)]
struct ChainSpecIdentity<'a> {
    version: u32,

    // Genesis
    genesis_hash: [u8; HASH_SIZE],

    // Proof of work
    pow_algorithm: &'a str,
    // Difficulty
    target_algorithm: &'a str,
    genesis_target_bits: u32,
    target_bits_start: u32,

    difficulty_adjustment_window: u64,
    target_block_time_seconds: u64,
    difficulty_target_timespan_seconds: u64,
    difficulty_timespan_clamp_factor: u64,
    max_difficulty_multiplier: u32,
    median_time_past_window: u8,
    max_future_timestamp_drift_seconds: u64,
    timestamp_policy: &'a str,

    // Monetary policy
    min_block_emission: u128,
    block_emission: u128,
    reward_policy: &'a str,
    finality_depth_blocks: u64,
    race_transfer_tax_percent: u8,
    race_to_gold_conversion_tax_percent: u8,
    gold_transfer_tax_percent: u8,
    gold_to_race_conversion_tax_percent: u8,
    gold_to_novus_conversion_tax_percent: u8,
    chip_war_epoch_blocks: u64,
    race_tax_modifier_min_percent_points: u8,
    race_tax_modifier_max_percent_points: u8,
    race_tax_effective_max_percent: u8,
    race_tax_modifier_initial_percent_points: u8,
    race_tax_loss_increase_percentage_points: u8,
    race_tax_failure_increase_percentage_points: u8,
    race_tax_win_decrease_percentage_points: u8,
    exchange_rate_base: u128,
    exchange_rate_adjustment_percent: i8,
    exchange_rate_max_deviation_percent: i8,
    exchange_rate_policy: &'a str,
    exchange_rounding_policy: &'a str,
    chip_war_outcome_policy: &'a str,
    race_tax_application_policy: &'a str,
    block_reward_currency_policy: &'a str,
    fee_tax_vault_policy: &'a str,
    gold_transfer_policy: &'a str,
    currency_conversion_policy: &'a str,
    fee_denomination_policy: &'a str,
    tax_denomination_policy: &'a str,
    gold_supply_cap: u128,
    gold_refill_policy: &'a str,
    gold_per_novus_conversion_rate: u128,
    currency_decimals: u8,

    // Block / address / hash
    max_block_size: u64,
    address_size: u32,
    address_encoding: &'a str,
    hash_size: u32,

    // Native protocol identity
    transaction_format: &'a str,
    stake_epoch_policy: &'a str,
    stake_reference_atomic: u128,
    stake_boost_scale: u32,
    stake_max_boost_ppm: u32,
    stake_pow_policy: &'a str,
}

pub fn chain_spec_hash() -> Result<Hash, GenesisError> {
    let identity = ChainSpecIdentity {
        version: CHAIN_SPEC_VERSION,

        genesis_hash: EXPECTED_GENESIS_HASH.into_bytes(),

        // Proof of work
        pow_algorithm: POW_ALGORITHM,

        // Difficulty
        target_algorithm: DIFFICULTY_ALGORITHM,
        genesis_target_bits: GENESIS_TARGET_BITS,
        target_bits_start: TARGET_BITS_START,

        difficulty_adjustment_window: DIFFICULTY_ADJUSTMENT_WINDOW,

        target_block_time_seconds: TARGET_BLOCK_TIME_SECONDS,

        difficulty_target_timespan_seconds: DIFFICULTY_TARGET_TIMESPAN_SECONDS,

        difficulty_timespan_clamp_factor: DIFFICULTY_TIMESPAN_CLAMP_FACTOR,
        max_difficulty_multiplier: crate::consensus::MAX_DIFFICULTY_MULTIPLIER,
        median_time_past_window: crate::consensus::MEDIAN_TIME_PAST_WINDOW as u8,
        max_future_timestamp_drift_seconds: crate::consensus::MAX_FUTURE_TIMESTAMP_DRIFT_SECONDS,
        timestamp_policy: "median-time-past-11-consensus-max-future-2h-admission-v1",

        // Emission
        min_block_emission: MIN_BLOCK_EMISSION,
        block_emission: BLOCK_EMISSION,
        reward_policy: "previous-block-hash-derived-100-to-1000-whole-coins-v1",
        finality_depth_blocks: FINALITY_DEPTH_BLOCKS,
        race_transfer_tax_percent: crate::consensus::TRANSFER_TAX_PERCENT,
        race_to_gold_conversion_tax_percent: crate::consensus::RACE_TO_GOLD_CONVERSION_TAX_PERCENT,
        gold_transfer_tax_percent: crate::consensus::GOLD_TRANSFER_TAX_PERCENT,
        gold_to_race_conversion_tax_percent: crate::consensus::GOLD_TO_RACE_CONVERSION_TAX_PERCENT,
        gold_to_novus_conversion_tax_percent:
            crate::consensus::GOLD_TO_NOVUS_CONVERSION_TAX_PERCENT,
        chip_war_epoch_blocks: crate::ledger::CHIP_WAR_EPOCH_BLOCKS,
        race_tax_modifier_min_percent_points: crate::ledger::MIN_RACE_TAX_MODIFIER_PERCENT,
        race_tax_modifier_max_percent_points: crate::ledger::MAX_RACE_TAX_MODIFIER_PERCENT,
        race_tax_effective_max_percent: crate::ledger::MAX_EFFECTIVE_TAX_PERCENT,
        race_tax_modifier_initial_percent_points: crate::ledger::BASE_RACE_TAX_MODIFIER_PERCENT,
        race_tax_loss_increase_percentage_points: crate::ledger::RACE_TAX_LOSS_INCREASE_PERCENT,
        race_tax_failure_increase_percentage_points:
            crate::ledger::RACE_TAX_FAILURE_INCREASE_PERCENT,
        race_tax_win_decrease_percentage_points: crate::ledger::RACE_TAX_WIN_DECREASE_PERCENT,
        exchange_rate_base: crate::transaction::RACE_CURRENCY_PER_GOLD,
        exchange_rate_adjustment_percent: crate::ledger::EXCHANGE_RATE_STEP_PERCENT,
        exchange_rate_max_deviation_percent: crate::ledger::MAX_EXCHANGE_RATE_DEVIATION_PERCENT,
        exchange_rate_policy: "chip-war-winner-lower-loser-raise-failure-unchanged-clamp-base-10000-v2",
        exchange_rounding_policy: "rational-floor-to-smallest-unit-v1",
        chip_war_outcome_policy: "unique-scores-high-wins-low-loses-middle-failure-any-tie-all-failure-v1",
        race_tax_application_policy: "uniform-base1-percent-on-all-taxable-types;loss-plus1-failure-plus1-shared-cumulative-modifier-cap6;win-minus1-floor-zero;effective-tax-cap7;novus-transfer-exempt-v8",
        block_reward_currency_policy: "miner-race-native-currency-v1",
        fee_tax_vault_policy: "all-protocol-taxes-keyed-by-origin-race-and-currency;only-origin-race-miners-claim-tax-vault;novus-transfer-fee-direct-to-including-block-miner-any-race;novus-tax-exempt-v3",
        gold_transfer_policy: "gold-transfer-sender-race-only;novus-transfer-global-v1",
        currency_conversion_policy: "one-way-1000-gold-to-1-novus;chip-war-race-currency-gold-exchange-v2",
        fee_denomination_policy: "novus-transfers-pay-size-based-fee-in-novus-v1;all-other-transactions-no-fee",
        tax_denomination_policy: "all-taxes-denominated-in-source-currency-and-held-in-origin-race-vault;novus-transfer-tax-exempt-v3",
        gold_supply_cap: crate::ledger::GOLD_SUPPLY_CAP.as_atomic(),
        gold_refill_policy: "burn-gold-for-novus-refill-100m-after-each-100m-cumulative-conversion-to-global-exchange-vault-v1",
        gold_per_novus_conversion_rate: crate::transaction::GOLD_PER_NOVUS,
        currency_decimals: crate::transaction::DECIMALS,

        max_block_size: MAX_BLOCK_SIZE as u64,
        address_size: ADDRESS_SIZE as u32,

        address_encoding: "novus-race-bound-v2-base62-sha3-checksum",

        hash_size: HASH_SIZE as u32,

        transaction_format: "account-nonce-single-currency-transfer-five-currencies-amount-atomic-decimals-6-explicit-signed-tax-novus-transfer-fee-only-one-way-conversion-race-pool-stake-delegate-undelegate-withdraw-war-heartbeat-v10",
        stake_epoch_policy: "delegate-minimum-1000-race-coins-per-wallet-no-maximum;delegate-activates-after-one-completed-chip-war-epoch;undelegate-withdrawable-after-one-completed-chip-war-epoch;chip-war-epoch-3-days-v2;one-cumulative-contribution-point-per-participating-wallet-per-epoch;active-stake-or-one-signed-node-heartbeat-per-epoch;race-specific-cumulative-eight-tier-rank-v2",
        stake_reference_atomic: crate::consensus::STAKE_REFERENCE_ATOMIC,
        stake_boost_scale: crate::consensus::STAKE_BOOST_SCALE,
        stake_max_boost_ppm: crate::consensus::STAKE_MAX_BOOST_PPM,
        stake_pow_policy: "network-difficulty-in-header;parent-state-total-active-race-pool-stake-boosts-every-miner-in-race-to-maximum-2x;stake-has-no-subsidy-or-tax-reward;absolute-target-limit-uint256-max;chainwork-from-effective-target;header-work-provisional-until-full-block-state-validation-v4",
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
