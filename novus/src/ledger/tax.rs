//! Protocol taxes and race-native transaction fees held until an eligible miner finds a block.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};

use crate::transaction::{Currency, Amount, Race};

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TaxVault {
    balances: BTreeMap<Currency, Amount>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TaxVaultRollback {
    before: BTreeMap<Currency, Option<Amount>>,
}

impl TaxVault {
    pub fn balance(&self, currency: Currency) -> Amount {
        self.balances.get(&currency).copied().unwrap_or(Amount::ZERO)
    }

    pub fn balances(&self) -> &BTreeMap<Currency, Amount> {
        &self.balances
    }

    pub(crate) fn credit(
        &mut self,
        taxes: &BTreeMap<Currency, Amount>,
    ) -> Option<TaxVaultRollback> {
        let mut staged = self.clone();
        let mut before = BTreeMap::new();
        for (&currency, &amount) in taxes {
            if amount.is_zero() {
                continue;
            }
            before.entry(currency).or_insert_with(|| self.balances.get(&currency).copied());
            let total = staged.balance(currency).checked_add(amount)?;
            staged.balances.insert(currency, total);
        }
        *self = staged;
        Some(TaxVaultRollback { before })
    }

    /// Race currencies stay in their nation's vault; universal currencies are
    /// claimable by any race's block miner.
    pub(crate) fn claim_for_race(
        &mut self,
        race: Race,
    ) -> (BTreeMap<Currency, Amount>, TaxVaultRollback) {
        let claimable = self.balances.iter()
            .filter(|(currency, _)| currency.required_race().is_none_or(|owner| owner == race))
            .map(|(&currency, &amount)| (currency, amount))
            .collect::<BTreeMap<_, _>>();
        let before = claimable.keys()
            .map(|&currency| (currency, self.balances.get(&currency).copied()))
            .collect();
        for currency in claimable.keys() {
            self.balances.remove(currency);
        }
        (claimable, TaxVaultRollback { before })
    }

    pub(crate) fn rollback(&mut self, journal: TaxVaultRollback) {
        for (currency, previous) in journal.before {
            match previous {
                Some(amount) => { self.balances.insert(currency, amount); }
                None => { self.balances.remove(&currency); }
            }
        }
    }
}
