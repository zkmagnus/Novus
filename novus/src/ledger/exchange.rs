//! Balances reserved for protocol currency exchange, separate from tax vaults.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};

use crate::transaction::{Currency, Amount};

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ExchangeVault {
    balances: BTreeMap<Currency, Amount>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ExchangeVaultRollback {
    before: BTreeMap<Currency, Option<Amount>>,
}

impl ExchangeVault {
    pub fn balance(&self, currency: Currency) -> Amount {
        self.balances.get(&currency).copied().unwrap_or(Amount::ZERO)
    }

    pub fn balances(&self) -> &BTreeMap<Currency, Amount> { &self.balances }

    pub(crate) fn adjust(
        &mut self,
        additions: &BTreeMap<Currency, Amount>,
        removals: &BTreeMap<Currency, Amount>,
    ) -> Option<ExchangeVaultRollback> {
        let mut staged = self.clone();
        let mut before = BTreeMap::new();
        for (&currency, &amount) in additions {
            if amount.is_zero() { continue; }
            before.entry(currency).or_insert_with(|| self.balances.get(&currency).copied());
            let next = staged.balance(currency).checked_add(amount)?;
            staged.balances.insert(currency, next);
        }
        for (&currency, &amount) in removals {
            if amount.is_zero() { continue; }
            before.entry(currency).or_insert_with(|| self.balances.get(&currency).copied());
            let next = staged.balance(currency).checked_sub(amount)?;
            if next.is_zero() { staged.balances.remove(&currency); }
            else { staged.balances.insert(currency, next); }
        }
        *self = staged;
        Some(ExchangeVaultRollback { before })
    }

    pub(crate) fn rollback(&mut self, journal: ExchangeVaultRollback) {
        for (currency, previous) in journal.before {
            match previous {
                Some(amount) => { self.balances.insert(currency, amount); }
                None => { self.balances.remove(&currency); }
            }
        }
    }
}
