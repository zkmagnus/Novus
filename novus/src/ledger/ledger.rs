use std::{collections::BTreeMap, error::Error as StdError, fmt};

use borsh::{BorshDeserialize, BorshSerialize};

use crypto::{BlockHash, HashDomain, StateRoot, canonical_bytes, domain};

use common::{ChainContext, Height};

use crate::{
    blockchain::{Block, Chain, ChainError},
    consensus::{
        ApplyBlockState, ConsensusError, EmissionError, FINALITY_DEPTH_BLOCKS, PoWTarget,
        TransactionConsensusError, TransactionStateView, ValidatedBlock, Work, validate_emission,
        validate_transaction,
    },
    ledger::{LedgerState, StateError, StateRollbackJournal},
};

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Ledger {
    pub chain: Chain,
    pub state: LedgerState,

    journals: BTreeMap<Height, Vec<StateRollbackJournal>>,
    block_works: BTreeMap<Height, Work>,
    cumulative_work: Work,

    chain_context: Option<ChainContext>,
    finalized_height: u64,
}

struct ExecutedBlock {
    journals: Vec<StateRollbackJournal>,
    state_root: StateRoot,
    block_size: u32,
    block_work: Work,
    chain_context: ChainContext,
}

impl Ledger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tip_height(&self) -> Option<Height> {
        self.chain.tip_height()
    }

    pub fn tip_hash(&self) -> Option<BlockHash> {
        self.chain.tip_hash()
    }

    pub fn finalized_height(&self) -> Height {
        Height(self.finalized_height)
    }

    pub const fn cumulative_work(&self) -> Work {
        self.cumulative_work
    }

    pub fn work_at(&self, height: Height) -> Option<Work> {
        self.block_works.get(&height).copied()
    }

    pub fn state(&self) -> &LedgerState {
        &self.state
    }

    pub fn state_root(&self) -> Result<StateRoot, LedgerError> {
        self.state.application_state_root()
    }

    pub fn preview_block_commitments(
        &self,
        block: &Block,
    ) -> Result<(StateRoot, u32), LedgerError> {
        let chain_context = self.chain_context_for(block)?;
        let mut staged_state = self.state.clone();
        let executed = Self::execute_block(&mut staged_state, block, chain_context, false)?;
        Ok((executed.state_root, executed.block_size))
    }

    fn chain_context_for(&self, block: &Block) -> Result<ChainContext, LedgerError> {
        match self.chain_context {
            Some(context) => Ok(context),
            None if block.is_genesis() => Ok(ChainContext::new(block.hash()?.into_bytes())),
            None => Err(LedgerError::EmptyChain),
        }
    }

    fn execute_block(
        state: &mut LedgerState,
        block: &Block,
        chain_context: ChainContext,
        enforce_pow: bool,
    ) -> Result<ExecutedBlock, LedgerError> {
        let mut journals = Vec::new();
        let block_size = u32::try_from(block.size()?).map_err(|_| LedgerError::InvalidBlockSize)?;
        let height = block.height();
        let mut block_work = Work::ZERO;

        let result = (|| {
            if !block.is_genesis() {
                let race = crate::transaction::Race::from(block.miner_address().race());
                let active_stake = state
                    .staking
                    .active_for_race(race)
                    .ok_or(StateError::AmountOverflow)?;
                let network_target = PoWTarget::from_compact(block.target_bits())
                    .ok_or(ConsensusError::InvalidDifficulty)?;
                let pow_limit = crate::consensus::stake_target_limit();
                let effective_target =
                    crate::consensus::effective_target(network_target, active_stake, pow_limit)
                        .ok_or(ConsensusError::InvalidDifficulty)?;
                if enforce_pow {
                    crate::consensus::verify_pow_at_target(&block.header, effective_target)?;
                }
                block_work = crate::consensus::stake_adjusted_work(effective_target)
                    .ok_or(ConsensusError::InvalidDifficulty)?;

                let emission = validate_emission(block)?;
                journals.push(state.apply_emission(
                    emission.recipient(),
                    emission.currency(),
                    emission.subsidy(),
                )?);
            }

            for transaction in block.transactions() {
                let validated =
                    validate_transaction(transaction.clone(), chain_context, height.0, state)?;
                journals.push(state.apply_validated_transaction_with_miner(
                    &validated,
                    Some(block.miner_address()),
                )?);
            }

            if !block.is_genesis() {
                journals.push(state.record_chip_war_block(
                    height.0,
                    crate::transaction::Race::from(block.miner_address().race()),
                )?);
            }

            state.application_state_root()
        })();

        let state_root = match result {
            Ok(state_root) => state_root,
            Err(error) => {
                for journal in journals.into_iter().rev() {
                    state.rollback_state(journal)?;
                }
                return Err(error);
            }
        };
        Ok(ExecutedBlock {
            journals,
            state_root,
            block_size,
            block_work,
            chain_context,
        })
    }

    pub fn rollback_tip(&mut self) -> Result<Block, LedgerError> {
        let height = self.chain.tip_height().ok_or(LedgerError::EmptyChain)?;
        if height.0 <= self.finalized_height {
            return Err(LedgerError::FinalizedBlock);
        }

        let hash = self.chain.tip_hash().ok_or(LedgerError::EmptyChain)?;

        let journals = self
            .journals
            .get(&height)
            .cloned()
            .ok_or(LedgerError::MissingRollbackJournal)?;

        for journal in journals.into_iter().rev() {
            self.state.rollback_state(journal)?;
        }

        let block_work = self
            .block_works
            .remove(&height)
            .ok_or(LedgerError::MissingRollbackJournal)?;
        self.cumulative_work = self
            .cumulative_work
            .checked_sub(block_work)
            .ok_or(LedgerError::State(StateError::AmountOverflow))?;

        let block = self.chain.remove_tip(hash)?;

        self.journals.remove(&height);

        if self.chain.tip_height().is_none() {
            self.chain_context = None;
        }

        Ok(block)
    }

    fn apply_validated_block(&mut self, validated: ValidatedBlock) -> Result<(), LedgerError> {
        let block = validated.block();
        let height = block.height();
        self.chain.validate_next_block(block)?;
        let chain_context = self.chain_context_for(block)?;
        let executed = Self::execute_block(&mut self.state, block, chain_context, true)?;

        if executed.block_size != block.block_size() {
            for journal in executed.journals.into_iter().rev() {
                self.state.rollback_state(journal)?;
            }
            return Err(LedgerError::InvalidBlockSize);
        }
        if block.state_root() != executed.state_root {
            for journal in executed.journals.into_iter().rev() {
                self.state.rollback_state(journal)?;
            }
            return Err(LedgerError::InvalidStateRoot);
        }
        if let Err(error) = self.chain.insert_block(validated.into_block()) {
            for journal in executed.journals.into_iter().rev() {
                self.state.rollback_state(journal)?;
            }
            return Err(error.into());
        }
        self.block_works.insert(height, executed.block_work);
        self.cumulative_work = self.cumulative_work.saturating_add(executed.block_work);
        self.chain_context = Some(executed.chain_context);
        self.journals.insert(height, executed.journals);
        self.finalized_height = self
            .finalized_height
            .max(height.0.saturating_sub(FINALITY_DEPTH_BLOCKS));
        Ok(())
    }
}

//
// Consensus state interface
//

impl ApplyBlockState for Ledger {
    type Error = LedgerError;

    fn consensus_chain(&self) -> &Chain {
        &self.chain
    }

    fn validate_block_pow(&self, block: &Block) -> Result<(), ConsensusError> {
        if block.is_genesis() {
            return Ok(());
        }
        let race = crate::transaction::Race::from(block.miner_address().race());
        let active_stake = self
            .state
            .staking
            .active_for_race(race)
            .ok_or(ConsensusError::InvalidDifficulty)?;
        let network_target = PoWTarget::from_compact(block.target_bits())
            .ok_or(ConsensusError::InvalidDifficulty)?;
        let pow_limit = crate::consensus::stake_target_limit();
        let target = crate::consensus::effective_target(network_target, active_stake, pow_limit)
            .ok_or(ConsensusError::InvalidDifficulty)?;
        crate::consensus::verify_pow_at_target(&block.header, target)
    }

    fn commit_validated_block(&mut self, block: ValidatedBlock) -> Result<(), Self::Error> {
        self.apply_validated_block(block)
    }
}

//
// Transaction state view
//

impl TransactionStateView for LedgerState {
    fn balance(
        &self,
        address: crypto::Address,
        currency: crate::transaction::Currency,
    ) -> crate::transaction::Amount {
        self.accounts.balance_of(address, currency)
    }
    fn nonce(&self, address: crypto::Address) -> u64 {
        self.accounts.nonce(address)
    }
    fn race(&self, address: crypto::Address) -> Option<crate::transaction::Race> {
        self.accounts.get(address).race
    }
    fn exchange_vault_balance(
        &self,
        currency: crate::transaction::Currency,
    ) -> crate::transaction::Amount {
        self.exchange_vault.balance(currency)
    }
    fn race_tax_rate(&self, race: crate::transaction::Race) -> u8 {
        self.chip_war.tax_rate(race)
    }
    fn race_exchange_rate(&self, race: crate::transaction::Race) -> u128 {
        self.chip_war.exchange_rate(race)
    }
    fn locked_stake(
        &self,
        race: crate::transaction::Race,
        delegator: crypto::Address,
    ) -> crate::transaction::Amount {
        self.staking.locked_by(race, delegator)
    }
    fn active_stake(
        &self,
        race: crate::transaction::Race,
        delegator: crypto::Address,
    ) -> crate::transaction::Amount {
        self.staking.active_delegation(race, delegator)
    }
    fn withdrawable_stake(
        &self,
        race: crate::transaction::Race,
        delegator: crypto::Address,
    ) -> crate::transaction::Amount {
        self.staking.withdrawable_by(race, delegator)
    }
    fn has_war_heartbeat(
        &self,
        race: crate::transaction::Race,
        delegator: crypto::Address,
    ) -> bool {
        self.staking.has_war_heartbeat(race, delegator)
    }
}

//
// Canonical application state root
//

impl LedgerState {
    pub(crate) fn application_state_root(&self) -> Result<StateRoot, LedgerError> {
        if self.accounts.iter().next().is_none()
            && self.novus.total_mined.is_zero()
            && self.novus.total_burned.is_zero()
            && self.novus.race_rewards.is_empty()
            && self.tax_vault.balances().is_empty()
            && self.exchange_vault == crate::ledger::ExchangeVault::default()
            && self.chip_war == crate::ledger::ChipWarState::default()
            && self.global_currency == crate::ledger::GlobalCurrencyState::default()
            && self.staking.is_empty()
        {
            return Ok(StateRoot::ZERO);
        }

        let state = if self.staking.is_empty() {
            canonical_bytes(&(
                &self.accounts,
                &self.novus,
                &self.tax_vault,
                &self.exchange_vault,
                &self.chip_war,
                &self.global_currency,
            ))?
        } else {
            canonical_bytes(&(
                &self.accounts,
                &self.novus,
                &self.tax_vault,
                &self.exchange_vault,
                &self.chip_war,
                &self.global_currency,
                &self.staking,
            ))?
        };

        Ok(StateRoot(
            domain(HashDomain::ProtocolState, &state).into_bytes(),
        ))
    }
}

//
// Ledger errors
//

#[derive(Debug)]
pub enum LedgerError {
    Consensus(ConsensusError),

    Transaction(TransactionConsensusError),

    State(StateError),

    Chain(ChainError),

    Emission(EmissionError),

    EmptyChain,

    MissingParentEmission,

    MissingRollbackJournal,

    InvalidStateRoot,

    InvalidBlockSize,
    FinalizedBlock,
}

impl fmt::Display for LedgerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Consensus(error) => {
                write!(formatter, "consensus validation failed: {error}")
            }

            Self::Transaction(error) => {
                write!(formatter, "transaction validation failed: {error}")
            }

            Self::State(error) => {
                write!(formatter, "ledger state transition failed: {error}")
            }

            Self::Chain(error) => {
                write!(formatter, "chain transition failed: {error}")
            }

            Self::Emission(error) => {
                write!(formatter, "emission validation failed: {error}")
            }

            Self::EmptyChain => formatter.write_str("ledger chain is empty"),

            Self::MissingParentEmission => formatter.write_str("parent emission is missing"),

            Self::MissingRollbackJournal => formatter.write_str("rollback journal is missing"),

            Self::InvalidStateRoot => formatter.write_str("block state root does not match ledger"),

            Self::InvalidBlockSize => {
                formatter.write_str("block execution size does not match ledger")
            }
            Self::FinalizedBlock => formatter.write_str("cannot roll back a finalized block"),
        }
    }
}

impl StdError for LedgerError {}

impl From<ConsensusError> for LedgerError {
    fn from(error: ConsensusError) -> Self {
        Self::Consensus(error)
    }
}

impl From<TransactionConsensusError> for LedgerError {
    fn from(error: TransactionConsensusError) -> Self {
        Self::Transaction(error)
    }
}

impl From<EmissionError> for LedgerError {
    fn from(error: EmissionError) -> Self {
        Self::Emission(error)
    }
}

impl From<StateError> for LedgerError {
    fn from(error: StateError) -> Self {
        Self::State(error)
    }
}

impl From<ChainError> for LedgerError {
    fn from(error: ChainError) -> Self {
        Self::Chain(error)
    }
}

impl From<crypto::CodecError> for LedgerError {
    fn from(_error: crypto::CodecError) -> Self {
        Self::Consensus(ConsensusError::Serialization)
    }
}

#[cfg(test)]
mod p3e_block_atomicity_tests {
    use super::*;

    use crate::{
        blockchain::{Block, Emission},
        consensus::{
            ConsensusError, expected_next_difficulty, reward_for_previous_hash,
            validate_candidate_for_apply,
        },
        genesis,
    };
    use common::Nonce;

    #[test]
    fn full_subsidy_is_mined() {
        let mut ledger = genesis::genesis_ledger().expect("genesis ledger");
        let miner = crypto::Address::new([0x31; crypto::ADDRESS_SIZE]);
        let subsidy = reward_for_previous_hash(ledger.tip_hash().expect("genesis tip").0);
        commit_empty_block(&mut ledger, miner);
        assert_eq!(ledger.state.novus.total_mined, subsidy);
        assert_eq!(ledger.state.accounts.balance(miner), subsidy);
    }

    #[test]
    fn finalized_boundary_cannot_be_rolled_back() {
        let mut ledger = genesis::genesis_ledger().expect("genesis ledger");
        let miner = crypto::Address::new([0x32; crypto::ADDRESS_SIZE]);
        commit_empty_block(&mut ledger, miner);
        ledger.finalized_height = 1;
        assert!(matches!(
            ledger.rollback_tip(),
            Err(LedgerError::FinalizedBlock)
        ));
        assert_eq!(ledger.tip_height(), Some(Height(1)));
    }

    fn ledger_bytes(ledger: &Ledger) -> Vec<u8> {
        borsh::to_vec(ledger).expect("ledger must serialize canonically")
    }

    fn empty_next_candidate(ledger: &Ledger, miner: crypto::Address) -> Block {
        let height = Height(
            ledger
                .tip_height()
                .map_or(0, |height| height.0.saturating_add(1)),
        );

        let previous = ledger.tip_hash().expect("canonical tip");

        let target_bits = expected_next_difficulty(&ledger.chain).expect("next target bits");

        let subsidy = reward_for_previous_hash(previous.0);

        Block::from_protocol_transactions(
            height,
            previous,
            target_bits,
            60 * height.0,
            Nonce(0),
            Some(Emission::new(miner, subsidy)),
            vec![],
        )
        .expect("empty candidate")
    }

    fn commit_empty_block(ledger: &mut Ledger, miner: crypto::Address) -> Block {
        let mut block = empty_next_candidate(ledger, miner);

        let (state_root, block_size) = ledger
            .preview_block_commitments(&block)
            .expect("preview commitments");

        block.set_state_root(state_root);
        block.set_block_size(block_size);

        let validated =
            validate_candidate_for_apply(&block, &ledger.chain).expect("valid candidate");

        ledger
            .apply_validated_block(validated)
            .expect("commit block");

        block
    }

    fn empty_height_one_candidate(ledger: &Ledger, miner: crypto::Address) -> Block {
        let previous = ledger.tip_hash().expect("genesis tip");

        let target_bits = expected_next_difficulty(&ledger.chain).expect("next target bits");

        Block::from_protocol_transactions(
            Height(1),
            previous,
            target_bits,
            60,
            Nonce(0),
            Some(Emission::new(miner, reward_for_previous_hash(previous.0))),
            vec![],
        )
        .expect("height-one candidate")
    }

    #[test]
    fn invalid_state_root_after_staging_does_not_mutate_ledger() {
        let mut ledger = genesis::genesis_ledger().expect("genesis ledger");

        let miner = crypto::Address::new([0x41; crypto::ADDRESS_SIZE]);

        let before = ledger_bytes(&ledger);

        let before_tip = ledger.tip_hash();

        let block = empty_height_one_candidate(&ledger, miner);

        let validated = validate_candidate_for_apply(&block, &ledger.chain)
            .expect("candidate must pass pre-application consensus");

        assert!(matches!(
            ledger.apply_validated_block(validated),
            Err(LedgerError::InvalidStateRoot)
        ));

        assert_eq!(ledger_bytes(&ledger), before);

        assert_eq!(ledger.tip_hash(), before_tip);

        assert_eq!(ledger.tip_height(), Some(Height(0)));
    }

    #[test]
    fn committed_block_then_rollback_restores_entire_ledger_byte_for_byte() {
        let mut ledger = genesis::genesis_ledger().expect("genesis ledger");

        let miner = crypto::Address::new([0x42; crypto::ADDRESS_SIZE]);

        let before = ledger_bytes(&ledger);

        let before_tip = ledger.tip_hash();

        let mut block = empty_height_one_candidate(&ledger, miner);

        let (state_root, block_size) = ledger
            .preview_block_commitments(&block)
            .expect("preview commitments");

        block.set_state_root(state_root);
        block.set_block_size(block_size);

        let validated =
            validate_candidate_for_apply(&block, &ledger.chain).expect("valid candidate");

        ledger
            .apply_validated_block(validated)
            .expect("commit block");

        assert_eq!(ledger.tip_height(), Some(Height(1)));

        assert_ne!(ledger_bytes(&ledger), before);

        let removed = ledger.rollback_tip().expect("rollback tip");

        assert_eq!(removed, block);

        assert_eq!(ledger.tip_hash(), before_tip);

        assert_eq!(ledger.tip_height(), Some(Height(0)));

        assert_eq!(ledger_bytes(&ledger), before);
    }

    #[test]
    fn two_committed_blocks_then_two_rollbacks_restore_genesis_byte_for_byte() {
        let mut ledger = genesis::genesis_ledger().expect("genesis ledger");

        let genesis_bytes = ledger_bytes(&ledger);

        let genesis_tip = ledger.tip_hash();

        let miner_one = crypto::Address::new([0x51; crypto::ADDRESS_SIZE]);

        let miner_two = crypto::Address::new([0x52; crypto::ADDRESS_SIZE]);

        let block_one = commit_empty_block(&mut ledger, miner_one);

        assert_eq!(ledger.tip_height(), Some(Height(1)));

        let height_one_bytes = ledger_bytes(&ledger);

        let height_one_tip = ledger.tip_hash();

        let block_two = commit_empty_block(&mut ledger, miner_two);

        assert_eq!(ledger.tip_height(), Some(Height(2)));

        assert_ne!(ledger_bytes(&ledger), height_one_bytes);

        let removed_two = ledger.rollback_tip().expect("rollback height two");

        assert_eq!(removed_two, block_two);

        assert_eq!(ledger.tip_height(), Some(Height(1)));

        assert_eq!(ledger.tip_hash(), height_one_tip);

        assert_eq!(ledger_bytes(&ledger), height_one_bytes);

        let removed_one = ledger.rollback_tip().expect("rollback height one");

        assert_eq!(removed_one, block_one);

        assert_eq!(ledger.tip_height(), Some(Height(0)));

        assert_eq!(ledger.tip_hash(), genesis_tip);

        assert_eq!(ledger_bytes(&ledger), genesis_bytes);
    }

    #[test]
    fn failed_second_block_does_not_mutate_committed_first_block() {
        let mut ledger = genesis::genesis_ledger().expect("genesis ledger");

        let miner_one = crypto::Address::new([0x61; crypto::ADDRESS_SIZE]);

        let miner_two = crypto::Address::new([0x62; crypto::ADDRESS_SIZE]);

        let block_one = commit_empty_block(&mut ledger, miner_one);

        assert_eq!(ledger.tip_height(), Some(Height(1)));

        let before = ledger_bytes(&ledger);

        let before_tip = ledger.tip_hash();

        let block_two = empty_next_candidate(&ledger, miner_two);

        let validated = validate_candidate_for_apply(&block_two, &ledger.chain)
            .expect("candidate must pass pre-application consensus");

        assert!(matches!(
            ledger.apply_validated_block(validated),
            Err(LedgerError::InvalidStateRoot)
        ));

        assert_eq!(ledger.tip_height(), Some(Height(1)));

        assert_eq!(ledger.tip_hash(), before_tip);

        assert_eq!(ledger_bytes(&ledger), before);

        let removed = ledger.rollback_tip().expect("height-one rollback");

        assert_eq!(removed, block_one);

        assert_eq!(ledger.tip_height(), Some(Height(0)));
    }

    #[test]
    fn invalid_block_size_after_staging_does_not_mutate_ledger() {
        let mut ledger = genesis::genesis_ledger().expect("genesis ledger");

        let miner = crypto::Address::new([0x71; crypto::ADDRESS_SIZE]);

        let before = ledger_bytes(&ledger);
        let before_tip = ledger.tip_hash();

        let mut block = empty_next_candidate(&ledger, miner);

        let (state_root, block_size) = ledger
            .preview_block_commitments(&block)
            .expect("preview commitments");

        block.set_state_root(state_root);

        //
        // Keep the size structurally plausible, but make it
        // different from the canonical execution size.
        //
        block.set_block_size(
            block_size
                .checked_add(1)
                .expect("fixture block size overflow"),
        );

        //
        // The malformed size is still large enough to satisfy
        // block-local structural validation.
        //
        let validated = validate_candidate_for_apply(&block, &ledger.chain)
            .expect("candidate must pass pre-application consensus");

        assert!(matches!(
            ledger.apply_validated_block(validated),
            Err(LedgerError::InvalidBlockSize)
        ));

        assert_eq!(ledger_bytes(&ledger), before);

        assert_eq!(ledger.tip_hash(), before_tip);

        assert_eq!(ledger.tip_height(), Some(Height(0)));
    }

    #[test]
    fn invalid_next_height_is_rejected_without_mutating_ledger() {
        let ledger = genesis::genesis_ledger().expect("genesis ledger");

        let miner = crypto::Address::new([0x72; crypto::ADDRESS_SIZE]);

        let before = ledger_bytes(&ledger);
        let before_tip = ledger.tip_hash();

        let mut block = empty_next_candidate(&ledger, miner);

        //
        // Genesis tip is height 0, therefore the only valid
        // next block is height 1.
        //
        block.height = Height(2);

        assert!(matches!(
            validate_candidate_for_apply(&block, &ledger.chain,),
            Err(ConsensusError::InvalidHeight)
        ));

        assert_eq!(ledger_bytes(&ledger), before);

        assert_eq!(ledger.tip_hash(), before_tip);

        assert_eq!(ledger.tip_height(), Some(Height(0)));
    }
    #[test]
    fn invalid_previous_hash_is_rejected_without_mutating_ledger() {
        let ledger = genesis::genesis_ledger().expect("genesis ledger");

        let miner = crypto::Address::new([0x73; crypto::ADDRESS_SIZE]);

        let before = ledger_bytes(&ledger);
        let before_tip = ledger.tip_hash();

        let mut block = empty_next_candidate(&ledger, miner);

        let mut wrong_previous = ledger.tip_hash().expect("genesis tip").0;

        wrong_previous[0] ^= 0xff;

        block.header.previous_hash = crypto::PreviousHash(wrong_previous);

        assert!(matches!(
            validate_candidate_for_apply(&block, &ledger.chain,),
            Err(ConsensusError::InvalidPreviousHash)
        ));

        assert_eq!(ledger_bytes(&ledger), before);

        assert_eq!(ledger.tip_hash(), before_tip);

        assert_eq!(ledger.tip_height(), Some(Height(0)));
    }
}
