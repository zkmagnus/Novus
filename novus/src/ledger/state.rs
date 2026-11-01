//! Canonical ledger state and rollback journal types.

use super::account::AccountState;
use super::tax::{TaxVault, TaxVaultRollback};
use super::exchange::{ExchangeVault, ExchangeVaultRollback};
use super::chip_war::{ChipWarRollback, ChipWarState};
use crate::transaction::{Currency, Amount};
use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};

#[derive(BorshSerialize, BorshDeserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct LedgerState {
    pub accounts: AccountState,
    pub novus: NovusRecord,
    pub tax_vault: TaxVault,
    pub exchange_vault: ExchangeVault,
    pub chip_war: ChipWarState,
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
}
