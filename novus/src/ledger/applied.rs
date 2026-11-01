use crypto::Address;
use crate::transaction::Amount;

use crate::{
    consensus::ValidatedTransaction,
    ledger::{AccountRollbackJournal, LedgerState, StateError, StateRollbackJournal},
};

impl LedgerState {
    pub(crate) fn apply_emission(
        &mut self,
        recipient: Address,
        currency: crate::transaction::Currency,
        subsidy: Amount,
    ) -> Result<StateRollbackJournal, StateError> {
        let race = crate::transaction::Race::from(recipient.race());
        if currency != crate::transaction::Currency::for_race_reward(race) {
            return Err(StateError::InvalidTransaction);
        }
        let (taxes, tax_vault) = self.tax_vault.claim_for_race(race);
        let accounts = self.accounts.checkpoint([recipient]);
        let credit_result = (|| {
            self.accounts.credit_race_reward(recipient, currency, subsidy)?;
            for (tax_currency, amount) in &taxes {
                if tax_currency.required_race().is_some() {
                    self.accounts.credit_race_reward(recipient, *tax_currency, *amount)?;
                } else {
                    self.accounts.credit_currency(recipient, *tax_currency, *amount)?;
                }
            }
            Ok::<(), crate::ledger::AccountError>(())
        })();
        if let Err(error) = credit_result {
            self.accounts.rollback(accounts);
            self.tax_vault.rollback(tax_vault);
            return Err(error.into());
        }
        let prior = self.novus.race_rewards.get(&currency).copied().unwrap_or(Amount::ZERO);
        let Some(total) = prior.checked_add(subsidy) else {
            self.accounts.rollback(accounts);
            self.tax_vault.rollback(tax_vault);
            return Err(StateError::AmountOverflow);
        };
        self.novus.race_rewards.insert(currency, total);
        Ok(StateRollbackJournal {
            account: Some(AccountRollbackJournal {
                accounts,
                mined: Amount::ZERO,
                burned: Amount::ZERO,
                race_reward: Some((currency, subsidy)),
            }),
            tax_vault: Some(tax_vault),
            exchange_vault: None,
            chip_war: None,
        })
    }

    pub fn apply_validated_transaction(
        &mut self,
        transaction: &ValidatedTransaction,
    ) -> Result<StateRollbackJournal, StateError> {
        let register_race = transaction.intent.register_race.then_some(transaction.intent.race);
        let mut exchange_vault_journal = None;
        let account_result = if let Some(conversion) = transaction.intent.conversion {
            let rate = self.chip_war.exchange_rate(transaction.intent.race);
            if conversion.quote_rate != rate { return Err(StateError::InvalidTransaction); }
            let output = crate::transaction::exchange_output_amount(
                conversion.amount.as_atomic(),
                conversion.from == crate::transaction::Currency::Gold,
                rate,
            ).map(Amount::from_atomic).ok_or(StateError::InvalidTransaction)?;
            let (to, output, additions, removals) = if conversion.from == crate::transaction::Currency::Gold {
                let to = crate::transaction::Currency::for_race_reward(transaction.intent.race);
                let mut additions = std::collections::BTreeMap::new();
                additions.insert(crate::transaction::Currency::Gold, conversion.amount);
                let mut removals = std::collections::BTreeMap::new();
                removals.insert(to, output);
                (to, output, additions, removals)
            } else {
                let held_gold = self.exchange_vault.balance(crate::transaction::Currency::Gold).as_atomic().min(output.as_atomic());
                let mut additions = std::collections::BTreeMap::new();
                additions.insert(conversion.from, conversion.amount);
                let mut removals = std::collections::BTreeMap::new();
                if held_gold > 0 { removals.insert(crate::transaction::Currency::Gold, Amount::from_atomic(held_gold)); }
                (crate::transaction::Currency::Gold, output, additions, removals)
            };
            let vault_journal = self.exchange_vault.adjust(&additions, &removals)
                .ok_or(StateError::AmountOverflow)?;
            exchange_vault_journal = Some(vault_journal);
            self.accounts.exchange_currency(
                transaction.intent.sender,
                transaction.intent.nonce,
                conversion.from,
                conversion.amount,
                &transaction.taxes,
                to,
                output,
                transaction.intent.fee,
                register_race,
            )
        } else {
            self.accounts.transfer(
                transaction.intent.sender,
                transaction.intent.nonce,
                &transaction.intent.transfers,
                &transaction.taxes,
                transaction.intent.fee,
                register_race,
            )
        };
        let accounts = match account_result {
            Ok(journal) => journal,
            Err(error) => {
                if let Some(journal) = exchange_vault_journal.take() { self.exchange_vault.rollback(journal); }
                return Err(error.into());
            }
        };
        let mut vault_credit = transaction.taxes.clone();
        let fee_currency = crate::transaction::Currency::for_race_reward(transaction.intent.race);
        let current_fee_currency_tax = vault_credit.get(&fee_currency).copied().unwrap_or(Amount::ZERO);
        let total_fee_currency_tax = current_fee_currency_tax.checked_add(transaction.intent.fee)
            .ok_or_else(|| {
                self.accounts.rollback(accounts.clone());
                if let Some(journal) = exchange_vault_journal.take() { self.exchange_vault.rollback(journal); }
                StateError::AmountOverflow
            })?;
        if !total_fee_currency_tax.is_zero() { vault_credit.insert(fee_currency, total_fee_currency_tax); }
        let tax_vault = match self.tax_vault.credit(&vault_credit) {
            Some(journal) => journal,
            None => {
                self.accounts.rollback(accounts);
                if let Some(journal) = exchange_vault_journal.take() { self.exchange_vault.rollback(journal); }
                return Err(StateError::AmountOverflow);
            }
        };
        let journal = AccountRollbackJournal {
            accounts,
            mined: Default::default(),
            burned: Amount::ZERO,
            race_reward: None,
        };
        Ok(StateRollbackJournal {
            account: Some(journal),
            tax_vault: Some(tax_vault),
            exchange_vault: exchange_vault_journal,
            chip_war: None,
        })
    }

    pub(crate) fn record_chip_war_block(
        &mut self,
        height: u64,
        miner_race: crate::transaction::Race,
    ) -> Result<StateRollbackJournal, StateError> {
        let journal = self.chip_war.record_block(height, miner_race)
            .ok_or(StateError::AmountOverflow)?;
        Ok(StateRollbackJournal { chip_war: Some(journal), ..StateRollbackJournal::default() })
    }

    pub(crate) fn rollback_state(
        &mut self,
        journal: StateRollbackJournal,
    ) -> Result<(), StateError> {
        if let Some(tax_vault) = journal.tax_vault {
            self.tax_vault.rollback(tax_vault);
        }
        if let Some(exchange_vault) = journal.exchange_vault {
            self.exchange_vault.rollback(exchange_vault);
        }
        if let Some(chip_war) = journal.chip_war {
            self.chip_war.rollback(chip_war);
        }
        if let Some(account) = journal.account {
            self.rollback_account(account)?;
        }
        Ok(())
    }

    pub(crate) fn rollback_account(
        &mut self,
        journal: AccountRollbackJournal,
    ) -> Result<(), StateError> {
        self.accounts.rollback(journal.accounts);
        self.novus.total_mined = self
            .novus
            .total_mined
            .checked_sub(journal.mined)
            .ok_or(StateError::AmountOverflow)?;
        self.novus.total_burned = self
            .novus
            .total_burned
            .checked_sub(journal.burned)
            .ok_or(StateError::AmountOverflow)?;
        if let Some((currency, reward)) = journal.race_reward {
            let remaining = self.novus.race_rewards.get(&currency).copied()
                .unwrap_or(Amount::ZERO)
                .checked_sub(reward)
                .ok_or(StateError::AmountOverflow)?;
            if remaining.is_zero() {
                self.novus.race_rewards.remove(&currency);
            } else {
                self.novus.race_rewards.insert(currency, remaining);
            }
        }
        Ok(())
    }
}
