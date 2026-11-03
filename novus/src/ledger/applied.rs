use crate::transaction::Amount;
use crypto::Address;

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
            self.accounts
                .credit_race_reward(recipient, currency, subsidy)?;
            for (tax_currency, amount) in &taxes {
                if tax_currency.required_race().is_some() {
                    self.accounts
                        .credit_race_reward(recipient, *tax_currency, *amount)?;
                } else {
                    self.accounts
                        .credit_currency(recipient, *tax_currency, *amount)?;
                }
            }
            Ok::<(), crate::ledger::AccountError>(())
        })();
        if let Err(error) = credit_result {
            self.accounts.rollback(accounts);
            self.tax_vault.rollback(tax_vault);
            return Err(error.into());
        }
        let Some(total_mined) = self.novus.total_mined.checked_add(subsidy) else {
            self.accounts.rollback(accounts);
            self.tax_vault.rollback(tax_vault);
            return Err(StateError::AmountOverflow);
        };
        let prior = self
            .novus
            .race_rewards
            .get(&currency)
            .copied()
            .unwrap_or(Amount::ZERO);
        let Some(total) = prior.checked_add(subsidy) else {
            self.accounts.rollback(accounts);
            self.tax_vault.rollback(tax_vault);
            return Err(StateError::AmountOverflow);
        };
        self.novus.total_mined = total_mined;
        self.novus.race_rewards.insert(currency, total);
        Ok(StateRollbackJournal {
            account: Some(AccountRollbackJournal {
                accounts,
                mined: subsidy,
                burned: Amount::ZERO,
                race_reward: Some((currency, subsidy)),
            }),
            tax_vault: Some(tax_vault),
            exchange_vault: None,
            chip_war: None,
            global_currency: None,
            staking: None,
        })
    }

    pub fn apply_validated_transaction(
        &mut self,
        transaction: &ValidatedTransaction,
    ) -> Result<StateRollbackJournal, StateError> {
        self.apply_validated_transaction_with_miner(transaction, None)
    }

    pub(crate) fn apply_validated_transaction_with_miner(
        &mut self,
        transaction: &ValidatedTransaction,
        miner: Option<Address>,
    ) -> Result<StateRollbackJournal, StateError> {
        if let Some(action) = transaction.intent.stake_action {
            return self.apply_stake_action(transaction, action);
        }
        let register_race = transaction
            .intent
            .register_race
            .then_some(transaction.intent.race);
        let mut exchange_vault_journal = None;
        let mut global_currency_journal = None;
        let account_result = if let Some(conversion) = transaction.intent.conversion {
            let rate = if (conversion.from, conversion.to)
                == (
                    crate::transaction::Currency::Gold,
                    crate::transaction::Currency::Novus,
                ) {
                crate::transaction::GOLD_PER_NOVUS
            } else {
                self.chip_war.exchange_rate(transaction.intent.race)
            };
            if conversion.quote_rate != rate {
                return Err(StateError::InvalidTransaction);
            }
            let output = crate::transaction::conversion_output_amount(
                conversion.amount.as_atomic(),
                conversion.from,
                conversion.to,
                rate,
            )
            .map(Amount::from_atomic)
            .ok_or(StateError::InvalidTransaction)?;
            let (to, output, additions, removals) =
                if conversion.from == crate::transaction::Currency::Gold {
                    let mut additions = std::collections::BTreeMap::new();
                    let mut removals = std::collections::BTreeMap::new();
                    if conversion.to != crate::transaction::Currency::Novus {
                        additions.insert(crate::transaction::Currency::Gold, conversion.amount);
                        removals.insert(conversion.to, output);
                    }
                    (conversion.to, output, additions, removals)
                } else {
                    let held_gold = self
                        .exchange_vault
                        .balance(crate::transaction::Currency::Gold)
                        .as_atomic()
                        .min(output.as_atomic());
                    let mut additions = std::collections::BTreeMap::new();
                    additions.insert(conversion.from, conversion.amount);
                    let mut removals = std::collections::BTreeMap::new();
                    if held_gold > 0 {
                        removals.insert(
                            crate::transaction::Currency::Gold,
                            Amount::from_atomic(held_gold),
                        );
                    }
                    (
                        crate::transaction::Currency::Gold,
                        output,
                        additions,
                        removals,
                    )
                };
            let vault_journal = self
                .exchange_vault
                .adjust(&additions, &removals)
                .ok_or(StateError::AmountOverflow)?;
            exchange_vault_journal = Some(vault_journal);
            if (conversion.from, conversion.to)
                == (
                    crate::transaction::Currency::Gold,
                    crate::transaction::Currency::Novus,
                )
            {
                let previous = self.global_currency.clone();
                let Some(total_burned) = previous.total_gold_burned.checked_add(conversion.amount)
                else {
                    self.exchange_vault
                        .rollback(exchange_vault_journal.take().expect("journal was set"));
                    return Err(StateError::AmountOverflow);
                };
                let Some(total_novus) = previous.total_novus_minted.checked_add(output) else {
                    self.exchange_vault
                        .rollback(exchange_vault_journal.take().expect("journal was set"));
                    return Err(StateError::AmountOverflow);
                };
                let Some(cycle_total) = previous
                    .gold_converted_this_cycle
                    .checked_add(conversion.amount)
                else {
                    self.exchange_vault
                        .rollback(exchange_vault_journal.take().expect("journal was set"));
                    return Err(StateError::AmountOverflow);
                };
                let cap = crate::ledger::GOLD_SUPPLY_CAP.as_atomic();
                let refills = cycle_total.as_atomic() / cap;
                let remainder = cycle_total.as_atomic() % cap;
                self.global_currency.total_gold_burned = total_burned;
                self.global_currency.total_novus_minted = total_novus;
                self.global_currency.gold_converted_this_cycle = Amount::from_atomic(remainder);
                if refills > 0 {
                    let Some(refill) = cap.checked_mul(refills).map(Amount::from_atomic) else {
                        self.exchange_vault
                            .rollback(exchange_vault_journal.take().expect("journal was set"));
                        self.global_currency = previous;
                        return Err(StateError::AmountOverflow);
                    };
                    let additions = std::collections::BTreeMap::from([(
                        crate::transaction::Currency::Gold,
                        refill,
                    )]);
                    let Some(refill_journal) =
                        self.exchange_vault.adjust(&additions, &Default::default())
                    else {
                        self.exchange_vault
                            .rollback(exchange_vault_journal.take().expect("journal was set"));
                        self.global_currency = previous;
                        return Err(StateError::AmountOverflow);
                    };
                    // Combine both vault mutations into one rollback record by preserving the
                    // original Gold balance from before conversion and refill.
                    let original = exchange_vault_journal.take().expect("journal was set");
                    let mut before = original.before;
                    for (currency, amount) in refill_journal.before {
                        before.entry(currency).or_insert(amount);
                    }
                    exchange_vault_journal = Some(crate::ledger::ExchangeVaultRollback { before });
                }
                global_currency_journal = Some(crate::ledger::GlobalCurrencyRollback { previous });
            }
            self.accounts.exchange_currency(
                transaction.intent.sender,
                transaction.intent.nonce,
                conversion.from,
                conversion.amount,
                &transaction.taxes,
                to,
                output,
                transaction.intent.fee,
                transaction.intent.fee_currency(),
                register_race,
            )
        } else {
            self.accounts.transfer(
                transaction.intent.sender,
                transaction.intent.nonce,
                &transaction.intent.transfers,
                &transaction.taxes,
                transaction.intent.fee,
                transaction.intent.fee_currency(),
                register_race,
            )
        };
        let mut accounts = match account_result {
            Ok(journal) => journal,
            Err(error) => {
                if let Some(journal) = exchange_vault_journal.take() {
                    self.exchange_vault.rollback(journal);
                }
                if let Some(journal) = global_currency_journal.take() {
                    self.global_currency = journal.previous;
                }
                return Err(error.into());
            }
        };
        if !transaction.intent.fee.is_zero() {
            let Some(miner) = miner else {
                self.accounts.rollback(accounts);
                if let Some(journal) = exchange_vault_journal.take() {
                    self.exchange_vault.rollback(journal);
                }
                if let Some(journal) = global_currency_journal.take() {
                    self.global_currency = journal.previous;
                }
                return Err(StateError::InvalidTransaction);
            };
            if let Err(error) = self.accounts.credit_currency_with_journal(
                miner,
                crate::transaction::Currency::Novus,
                transaction.intent.fee,
                &mut accounts,
            ) {
                self.accounts.rollback(accounts);
                if let Some(journal) = exchange_vault_journal.take() {
                    self.exchange_vault.rollback(journal);
                }
                if let Some(journal) = global_currency_journal.take() {
                    self.global_currency = journal.previous;
                }
                return Err(error.into());
            }
        }
        let vault_credit = transaction.taxes.clone();
        let tax_vault = match self
            .tax_vault
            .credit(transaction.intent.race, &vault_credit)
        {
            Some(journal) => journal,
            None => {
                self.accounts.rollback(accounts);
                if let Some(journal) = exchange_vault_journal.take() {
                    self.exchange_vault.rollback(journal);
                }
                if let Some(journal) = global_currency_journal.take() {
                    self.global_currency = journal.previous;
                }
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
            global_currency: global_currency_journal,
            staking: None,
        })
    }

    fn apply_stake_action(
        &mut self,
        transaction: &ValidatedTransaction,
        action: crate::transaction::StakeAction,
    ) -> Result<StateRollbackJournal, StateError> {
        use crate::transaction::{Currency, StakeAction};

        let sender = transaction.intent.sender;
        let race = transaction.intent.race;
        let currency = Currency::for_race_reward(race);
        let accounts = self.accounts.checkpoint([sender]);
        let previous_staking = self.staking.clone();

        let result = (|| {
            match action {
                StakeAction::Delegate { amount } => {
                    self.accounts.debit_currency(
                        sender,
                        currency,
                        amount,
                        transaction.intent.nonce,
                    )?;
                    self.staking
                        .schedule_delegate(race, sender, amount)
                        .ok_or(crate::ledger::AccountError::BalanceOverflow)?;
                }
                StakeAction::Undelegate { amount } => {
                    self.accounts.debit_currency(
                        sender,
                        currency,
                        Amount::ZERO,
                        transaction.intent.nonce,
                    )?;
                    self.staking
                        .schedule_undelegate(race, sender, amount)
                        .ok_or(crate::ledger::AccountError::InsufficientBalance)?;
                }
                StakeAction::Withdraw { amount } => {
                    self.accounts.debit_currency(
                        sender,
                        currency,
                        Amount::ZERO,
                        transaction.intent.nonce,
                    )?;
                    self.staking
                        .withdraw(race, sender, amount)
                        .ok_or(crate::ledger::AccountError::InsufficientBalance)?;
                    self.accounts.credit_currency(sender, currency, amount)?;
                }
                StakeAction::WarHeartbeat => {
                    self.accounts.debit_currency(
                        sender,
                        currency,
                        Amount::ZERO,
                        transaction.intent.nonce,
                    )?;
                    self.staking
                        .record_war_heartbeat(race, sender)
                        .ok_or(crate::ledger::AccountError::InsufficientBalance)?;
                }
            }
            Ok::<(), crate::ledger::AccountError>(())
        })();

        if let Err(error) = result {
            self.accounts.rollback(accounts);
            self.staking = previous_staking;
            return Err(error.into());
        }

        Ok(StateRollbackJournal {
            account: Some(AccountRollbackJournal {
                accounts,
                mined: Amount::ZERO,
                burned: Amount::ZERO,
                race_reward: None,
            }),
            staking: Some(previous_staking),
            ..StateRollbackJournal::default()
        })
    }

    pub(crate) fn record_chip_war_block(
        &mut self,
        height: u64,
        miner_race: crate::transaction::Race,
    ) -> Result<StateRollbackJournal, StateError> {
        let previous_staking = self.staking.clone();
        self.staking
            .record_war_block()
            .ok_or(StateError::AmountOverflow)?;
        let journal = match self.chip_war.record_block(height, miner_race) {
            Some(journal) => journal,
            None => {
                self.staking = previous_staking;
                return Err(StateError::AmountOverflow);
            }
        };
        if self.chip_war.completed_epochs > previous_staking.current_epoch()
            && self
                .staking
                .settle_contribution_epoch(previous_staking.current_epoch())
                .is_none()
        {
            self.chip_war.rollback(journal);
            self.staking = previous_staking;
            return Err(StateError::AmountOverflow);
        }
        if self
            .staking
            .advance_epoch(self.chip_war.completed_epochs)
            .is_none()
        {
            self.chip_war.rollback(journal);
            self.staking = previous_staking;
            return Err(StateError::AmountOverflow);
        }
        Ok(StateRollbackJournal {
            chip_war: Some(journal),
            staking: Some(previous_staking),
            ..StateRollbackJournal::default()
        })
    }

    pub(crate) fn rollback_state(
        &mut self,
        journal: StateRollbackJournal,
    ) -> Result<(), StateError> {
        if let Some(account) = journal.account {
            self.rollback_account(account)?;
        }
        if let Some(tax_vault) = journal.tax_vault {
            self.tax_vault.rollback(tax_vault);
        }
        if let Some(exchange_vault) = journal.exchange_vault {
            self.exchange_vault.rollback(exchange_vault);
        }
        if let Some(chip_war) = journal.chip_war {
            self.chip_war.rollback(chip_war);
        }
        if let Some(global_currency) = journal.global_currency {
            self.global_currency = global_currency.previous;
        }
        if let Some(staking) = journal.staking {
            self.staking = staking;
        }
        Ok(())
    }

    pub(crate) fn rollback_account(
        &mut self,
        journal: AccountRollbackJournal,
    ) -> Result<(), StateError> {
        let total_mined = self
            .novus
            .total_mined
            .checked_sub(journal.mined)
            .ok_or(StateError::AmountOverflow)?;
        let total_burned = self
            .novus
            .total_burned
            .checked_sub(journal.burned)
            .ok_or(StateError::AmountOverflow)?;
        let race_reward = journal
            .race_reward
            .map(|(currency, reward)| {
                self.novus
                    .race_rewards
                    .get(&currency)
                    .copied()
                    .unwrap_or(Amount::ZERO)
                    .checked_sub(reward)
                    .map(|remaining| (currency, remaining))
                    .ok_or(StateError::AmountOverflow)
            })
            .transpose()?;

        self.accounts.rollback(journal.accounts);
        self.novus.total_mined = total_mined;
        self.novus.total_burned = total_burned;
        if let Some((currency, remaining)) = race_reward {
            if remaining.is_zero() {
                self.novus.race_rewards.remove(&currency);
            } else {
                self.novus.race_rewards.insert(currency, remaining);
            }
        }
        Ok(())
    }
}
