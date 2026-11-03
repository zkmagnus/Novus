//! Asset-denominated protocol taxes and fees held until an eligible miner finds a block.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};

use crate::transaction::{Amount, Currency, Race};

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TaxVault {
    balances: BTreeMap<(Race, Currency), Amount>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TaxVaultRollback {
    before: BTreeMap<(Race, Currency), Option<Amount>>,
}

impl TaxVault {
    pub fn balance(&self, race: Race, currency: Currency) -> Amount {
        self.balances
            .get(&(race, currency))
            .copied()
            .unwrap_or(Amount::ZERO)
    }

    pub fn balances(&self) -> &BTreeMap<(Race, Currency), Amount> {
        &self.balances
    }

    pub(crate) fn credit(
        &mut self,
        race: Race,
        taxes: &BTreeMap<Currency, Amount>,
    ) -> Option<TaxVaultRollback> {
        let mut staged = self.clone();
        let mut before = BTreeMap::new();
        for (&currency, &amount) in taxes {
            if amount.is_zero() {
                continue;
            }
            before
                .entry((race, currency))
                .or_insert_with(|| self.balances.get(&(race, currency)).copied());
            let total = staged.balance(race, currency).checked_add(amount)?;
            staged.balances.insert((race, currency), total);
        }
        *self = staged;
        Some(TaxVaultRollback { before })
    }

    /// Taxes are reserved for miners in the race that generated them, even if
    /// the taxed currency itself (such as Gold) is globally transferable.
    pub(crate) fn claim_for_race(
        &mut self,
        race: Race,
    ) -> (BTreeMap<Currency, Amount>, TaxVaultRollback) {
        let claimable = self
            .balances
            .iter()
            .filter(|((owner, _), _)| *owner == race)
            .map(|(&(_, currency), &amount)| (currency, amount))
            .collect::<BTreeMap<_, _>>();
        let before = claimable
            .keys()
            .map(|&currency| {
                (
                    (race, currency),
                    self.balances.get(&(race, currency)).copied(),
                )
            })
            .collect();
        for currency in claimable.keys() {
            self.balances.remove(&(race, *currency));
        }
        (claimable, TaxVaultRollback { before })
    }

    pub(crate) fn rollback(&mut self, journal: TaxVaultRollback) {
        for (currency, previous) in journal.before {
            match previous {
                Some(amount) => {
                    self.balances.insert(currency, amount);
                }
                None => {
                    self.balances.remove(&currency);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gold_tax_is_claimable_only_by_a_miner_from_its_origin_race() {
        let mut vault = TaxVault::default();
        let taxes = BTreeMap::from([(Currency::Gold, Amount::from_atomic(75))]);
        vault.credit(Race::Bellato, &taxes).expect("tax credit");

        let (accretia_claim, rollback) = vault.claim_for_race(Race::Accretia);
        assert!(accretia_claim.is_empty());
        vault.rollback(rollback);
        assert_eq!(
            vault.balance(Race::Bellato, Currency::Gold),
            Amount::from_atomic(75)
        );

        let (bellato_claim, _) = vault.claim_for_race(Race::Bellato);
        assert_eq!(
            bellato_claim.get(&Currency::Gold),
            Some(&Amount::from_atomic(75))
        );
        assert_eq!(vault.balance(Race::Bellato, Currency::Gold), Amount::ZERO);
    }

    #[test]
    fn race_currency_tax_stays_in_the_origin_race_vault() {
        let mut vault = TaxVault::default();
        let taxes = BTreeMap::from([(Currency::Dalant, Amount::from_atomic(23))]);
        vault.credit(Race::Bellato, &taxes).expect("tax credit");

        let (accretia_claim, _) = vault.claim_for_race(Race::Accretia);
        assert!(accretia_claim.is_empty());
        let (bellato_claim, _) = vault.claim_for_race(Race::Bellato);
        assert_eq!(
            bellato_claim.get(&Currency::Dalant),
            Some(&Amount::from_atomic(23))
        );
    }
}
