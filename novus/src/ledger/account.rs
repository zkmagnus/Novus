//! Account balances used by the RF account-model ledger.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use crypto::Address;

use crate::transaction::{Currency, Amount};

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Account {
    pub balances: BTreeMap<Currency, Amount>,
    pub race: Option<crate::transaction::Race>,
    /// Next transaction nonce accepted from this address.
    pub nonce: u64,
}

impl Account {
    pub fn balance(&self) -> Amount {
        self.race.map(Currency::for_race_reward)
            .and_then(|currency| self.balances.get(&currency).copied())
            .unwrap_or(Amount::ZERO)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AccountState {
    accounts: BTreeMap<Address, Account>,
}

impl AccountState {
    pub fn checkpoint(&self, addresses: impl IntoIterator<Item = Address>) -> AccountRollback {
        let before = addresses
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(|address| (address, self.get(address)))
            .collect();
        AccountRollback { before }
    }
    pub fn get(&self, address: Address) -> Account {
        self.accounts.get(&address).cloned().unwrap_or_default()
    }

    pub fn balance(&self, address: Address) -> Amount {
        self.balance_of(address, Currency::for_race_reward(crate::transaction::Race::from(address.race())))
    }

    pub fn balance_of(&self, address: Address, currency: Currency) -> Amount {
        self.get(address).balances.get(&currency).copied().unwrap_or(Amount::ZERO)
    }

    pub fn nonce(&self, address: Address) -> u64 {
        self.get(address).nonce
    }

    pub fn iter(&self) -> impl Iterator<Item = (Address, Account)> + '_ {
        self.accounts
            .iter()
            .map(|(&address, account)| (address, account.clone()))
    }

    pub fn set(&mut self, address: Address, account: Account) {
        if account == Account::default() {
            self.accounts.remove(&address);
        } else {
            self.accounts.insert(address, account);
        }
    }

    pub fn credit(&mut self, address: Address, amount: Amount) -> Result<(), AccountError> {
        self.credit_race_reward(
            address,
            Currency::for_race_reward(crate::transaction::Race::from(address.race())),
            amount,
        )
    }

    pub fn credit_currency(&mut self, address: Address, currency: Currency, amount: Amount) -> Result<(), AccountError> {
        let mut account = self.get(address);
        if let Some(required_race) = currency.required_race() {
            if account.race != Some(required_race) { return Err(AccountError::RaceMismatch); }
        }
        let balance = account.balances.get(&currency).copied().unwrap_or(Amount::ZERO)
            .checked_add(amount)
            .ok_or(AccountError::BalanceOverflow)?;
        if balance.is_zero() { account.balances.remove(&currency); }
        else { account.balances.insert(currency, balance); }
        self.set(address, account);
        Ok(())
    }

    pub fn credit_race_reward(
        &mut self,
        address: Address,
        currency: Currency,
        amount: Amount,
    ) -> Result<(), AccountError> {
        let race = currency.required_race().ok_or(AccountError::RaceMismatch)?;
        if crate::transaction::Race::from(address.race()) != race {
            return Err(AccountError::RaceMismatch);
        }
        let mut account = self.get(address);
        if account.race.is_some_and(|assigned| assigned != race) {
            return Err(AccountError::RaceMismatch);
        }
        account.race = Some(race);
        self.set(address, account);
        self.credit_currency(address, currency, amount)
    }

    pub fn transfer(
        &mut self,
        sender: Address,
        nonce: u64,
        transfers: &[crate::transaction::Transfer],
        taxes: &BTreeMap<Currency, Amount>,
        fee: Amount,
        register_race: Option<crate::transaction::Race>,
    ) -> Result<AccountRollback, AccountError> {
        let mut debits = BTreeMap::<Currency, Amount>::new();
        let fee_currency = Currency::for_race_reward(crate::transaction::Race::from(sender.race()));
        debits.insert(fee_currency, fee);
        for transfer in transfers {
            let current = debits.get(&transfer.currency).copied().unwrap_or(Amount::ZERO);
            debits.insert(transfer.currency, current.checked_add(transfer.amount).ok_or(AccountError::BalanceOverflow)?);
        }
        for (&currency, &amount) in taxes {
            let current = debits.get(&currency).copied().unwrap_or(Amount::ZERO);
            debits.insert(currency, current.checked_add(amount).ok_or(AccountError::BalanceOverflow)?);
        }
        let mut staged = self.clone();

        let before = std::iter::once(sender)
            .chain(transfers.iter().map(|transfer| transfer.recipient))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(|address| (address, self.get(address)))
            .collect::<Vec<_>>();

        let mut sender_account = staged.get(sender);
        if let Some(race) = register_race {
            if sender_account.race.is_some() { return Err(AccountError::RaceAlreadyRegistered); }
            sender_account.race = Some(race);
        } else if sender_account.race.is_none() {
            return Err(AccountError::RaceNotRegistered);
        }
        if sender_account.nonce != nonce { return Err(AccountError::InvalidNonce); }
        if let Some(race) = sender_account.race
            && debits.iter().any(|(currency, amount)| !amount.is_zero() && currency.required_race().is_some_and(|required| required != race))
        { return Err(AccountError::RaceMismatch); }
        for (&currency, &amount) in &debits {
            let balance = sender_account.balances.get(&currency).copied().unwrap_or(Amount::ZERO)
                .checked_sub(amount)
                .ok_or(AccountError::InsufficientBalance)?;
            if balance.is_zero() { sender_account.balances.remove(&currency); }
            else { sender_account.balances.insert(currency, balance); }
        }
        sender_account.nonce = sender_account.nonce.checked_add(1).ok_or(AccountError::NonceOverflow)?;
        staged.set(sender, sender_account);
        for transfer in transfers {
            if let Some(required_race) = transfer.currency.required_race() {
                let mut recipient_account = staged.get(transfer.recipient);
                if recipient_account.race.is_none() {
                    recipient_account.race = Some(required_race);
                    staged.set(transfer.recipient, recipient_account);
                }
                if staged.get(transfer.recipient).race != Some(required_race) {
                    return Err(AccountError::RaceMismatch);
                }
            }
            staged.credit_currency(transfer.recipient, transfer.currency, transfer.amount)?;
        }
        *self = staged;
        Ok(AccountRollback { before })
    }

    pub fn exchange_currency(
        &mut self,
        sender: Address,
        nonce: u64,
        from: Currency,
        amount: Amount,
        taxes: &BTreeMap<Currency, Amount>,
        to: Currency,
        output: Amount,
        fee: Amount,
        register_race: Option<crate::transaction::Race>,
    ) -> Result<AccountRollback, AccountError> {
        let race = crate::transaction::Race::from(sender.race());
        if from.required_race().is_some_and(|required| required != race)
            || to.required_race().is_some_and(|required| required != race)
            || !matches!((from, to), (Currency::Cp | Currency::Dalant | Currency::Disena, Currency::Gold) | (Currency::Gold, Currency::Cp | Currency::Dalant | Currency::Disena))
        {
            return Err(AccountError::RaceMismatch);
        }
        let mut staged = self.clone();
        let sender_before = self.get(sender);
        let mut account = staged.get(sender);
        if let Some(register_race) = register_race {
            if account.race.is_some() { return Err(AccountError::RaceAlreadyRegistered); }
            if register_race != race { return Err(AccountError::RaceMismatch); }
            account.race = Some(race);
        } else if account.race != Some(race) {
            return Err(AccountError::RaceNotRegistered);
        }
        if account.nonce != nonce { return Err(AccountError::InvalidNonce); }
        let fee_currency = Currency::for_race_reward(race);
        let mut debits = BTreeMap::<Currency, Amount>::new();
        debits.insert(from, amount);
        for (&currency, &tax) in taxes {
            let current = debits.get(&currency).copied().unwrap_or(Amount::ZERO);
            debits.insert(currency, current.checked_add(tax).ok_or(AccountError::BalanceOverflow)?);
        }
        let current = debits.get(&fee_currency).copied().unwrap_or(Amount::ZERO);
        debits.insert(fee_currency, current.checked_add(fee).ok_or(AccountError::BalanceOverflow)?);
        for (currency, debit) in debits {
            let balance = account.balances.get(&currency).copied().unwrap_or(Amount::ZERO)
                .checked_sub(debit).ok_or(AccountError::InsufficientBalance)?;
            if balance.is_zero() { account.balances.remove(&currency); }
            else { account.balances.insert(currency, balance); }
        }
        account.nonce = account.nonce.checked_add(1).ok_or(AccountError::NonceOverflow)?;
        staged.set(sender, account);
        staged.credit_currency(sender, to, output)?;
        *self = staged;
        Ok(AccountRollback { before: vec![(sender, sender_before)] })
    }

    pub fn rollback(&mut self, journal: AccountRollback) {
        for (address, account) in journal.before {
            self.set(address, account);
        }
    }

    /// Debits an account and advances its nonce as one checked operation.
    pub fn debit(
        &mut self,
        address: Address,
        amount: Amount,
        nonce: u64,
    ) -> Result<(), AccountError> {
        self.debit_currency(address, Currency::for_race_reward(crate::transaction::Race::from(address.race())), amount, nonce)
    }

    pub fn debit_currency(&mut self, address: Address, currency: Currency, amount: Amount, nonce: u64) -> Result<(), AccountError> {
        let mut account = self.get(address);
        if account.nonce != nonce {
            return Err(AccountError::InvalidNonce);
        }
        let balance = account.balances.get(&currency).copied().unwrap_or(Amount::ZERO)
            .checked_sub(amount)
            .ok_or(AccountError::InsufficientBalance)?;
        if balance.is_zero() { account.balances.remove(&currency); }
        else { account.balances.insert(currency, balance); }
        account.nonce = account
            .nonce
            .checked_add(1)
            .ok_or(AccountError::NonceOverflow)?;
        self.set(address, account);
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AccountRollback {
    before: Vec<(Address, Account)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountError {
    BalanceOverflow,
    InsufficientBalance,
    InvalidNonce,
    NonceOverflow,
    RaceMismatch,
    RaceAlreadyRegistered,
    RaceNotRegistered,
}
