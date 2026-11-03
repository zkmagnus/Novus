//! Canonical ledger state and rollback journal types.

use super::account::AccountState;
use super::chip_war::{ChipWarRollback, ChipWarState};
use super::exchange::{ExchangeVault, ExchangeVaultRollback};
use super::tax::{TaxVault, TaxVaultRollback};
use super::staking::StakeState;
use crate::transaction::{Amount, Currency};
use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};

#[derive(BorshSerialize, BorshDeserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct LedgerState {
    pub accounts: AccountState,
    pub novus: NovusRecord,
    pub tax_vault: TaxVault,
    pub exchange_vault: ExchangeVault,
    pub chip_war: ChipWarState,
    pub global_currency: GlobalCurrencyState,
    pub staking: StakeState,
}

#[derive(BorshSerialize, BorshDeserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalCurrencyState {
    /// Gold converted into Novus since the last Gold refill.
    pub gold_converted_this_cycle: Amount,
    /// Total Gold burned by Gold-to-Novus conversions.
    pub total_gold_burned: Amount,
    /// Total Novus minted by Gold-to-Novus conversions.
    pub total_novus_minted: Amount,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct NovusRecord {
    pub total_mined: Amount,
    pub total_burned: Amount,
    pub race_rewards: BTreeMap<Currency, Amount>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AccountRollbackJournal {
    pub(crate) accounts: super::account::AccountRollback,
    pub(crate) mined: Amount,
    pub(crate) burned: Amount,
    pub(crate) race_reward: Option<(Currency, Amount)>,
}

#[derive(BorshSerialize, BorshDeserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct StateRollbackJournal {
    pub account: Option<AccountRollbackJournal>,
    pub tax_vault: Option<TaxVaultRollback>,
    pub exchange_vault: Option<ExchangeVaultRollback>,
    pub chip_war: Option<ChipWarRollback>,
    pub global_currency: Option<GlobalCurrencyRollback>,
    pub staking: Option<StakeState>,
}

#[derive(BorshSerialize, BorshDeserialize, Debug, Clone, PartialEq, Eq)]
pub struct GlobalCurrencyRollback {
    pub previous: GlobalCurrencyState,
}
