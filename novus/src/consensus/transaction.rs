use std::{error::Error as StdError, fmt};
use std::collections::BTreeMap;

use common::ChainContext;
use crypto::{Address, TransactionHash};

use crate::transaction::{AccountIntent, Currency, IntentError, Amount, Race, Transaction};
use super::TRANSFER_TAX_PERCENT;
use crate::ledger::{BASE_RACE_TAX_PERCENT, MAX_RACE_TAX_PERCENT};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedTransaction {
    pub intent: AccountIntent,
    pub txid: TransactionHash,
    pub taxes: BTreeMap<Currency, Amount>,
}

pub trait TransactionStateView {
    fn balance(&self, address: Address, currency: Currency) -> Amount;
    fn race(&self, _address: Address) -> Option<Race> { None }
    fn exchange_vault_balance(&self, _currency: Currency) -> Amount { Amount::ZERO }
    fn race_tax_rate(&self, _race: Race) -> u8 { BASE_RACE_TAX_PERCENT }
    fn race_exchange_rate(&self, _race: Race) -> u128 { crate::transaction::RACE_CURRENCY_PER_GOLD }
    fn nonce(&self, address: Address) -> u64;
}

pub fn validate_transaction(
    transaction: Transaction,
    chain: ChainContext,
    _current_height: u64,
    state: &impl TransactionStateView,
) -> Result<ValidatedTransaction, TransactionConsensusError> {
    transaction
        .intent
        .validate_structure()
        .map_err(TransactionConsensusError::Intent)?;
    if !transaction
        .verify_authorization(chain)
        .map_err(|_| TransactionConsensusError::Encoding)?
    {
        return Err(TransactionConsensusError::InvalidAuthorization);
    }

    let intent = &transaction.intent;
    if intent.race != intent.sender.race().into() {
        return Err(TransactionConsensusError::SenderRaceMismatch);
    }
    if state.race(intent.sender).is_some_and(|race| race != intent.race)
        || (state.race(intent.sender).is_none() && !intent.register_race)
    {
        return Err(TransactionConsensusError::SenderRaceMismatch);
    }
    if intent.register_race && state.race(intent.sender).is_some() {
        return Err(TransactionConsensusError::SenderRaceMismatch);
    }
    let taxes = calculate_transfer_taxes_with_rates(intent, |race| state.race_tax_rate(race))?;
    if let Some(conversion) = intent.conversion {
        validate_conversion(intent, conversion, &taxes, state)?;
    } else {
        validate_account_transfer(intent, &taxes, state)?;
    }

    let txid = transaction
        .transaction_id()
        .map_err(|_| TransactionConsensusError::Encoding)?;
    Ok(ValidatedTransaction {
        intent: transaction.intent,
        txid,
        taxes,
    })
}

fn validate_conversion(
    intent: &AccountIntent,
    conversion: crate::transaction::CurrencyConversion,
    taxes: &BTreeMap<Currency, Amount>,
    state: &impl TransactionStateView,
) -> Result<(), TransactionConsensusError> {
    if state.nonce(intent.sender) != intent.nonce {
        return Err(TransactionConsensusError::InvalidNonce);
    }
    let active_rate = state.race_exchange_rate(intent.race);
    if conversion.quote_rate != active_rate {
        return Err(TransactionConsensusError::StaleExchangeQuote);
    }
    let payout = crate::transaction::exchange_output_amount(
        conversion.amount.as_atomic(),
        conversion.from == Currency::Gold,
        active_rate,
    ).ok_or(TransactionConsensusError::InvalidConversionAmount)?;
    let mut required = BTreeMap::new();
    required.insert(Currency::for_race_reward(intent.race), intent.fee);
    let (vault_currency, vault_required) = if conversion.from == Currency::Gold {
        let target = Currency::for_race_reward(intent.race);
        (target, Amount::from_atomic(payout))
    } else {
        if conversion.from.required_race() != Some(intent.race) {
            return Err(TransactionConsensusError::CurrencyNotOwnedBySenderRace);
        }
        (Currency::Gold, Amount::ZERO)
    };
    let current = required.get(&conversion.from).copied().unwrap_or(Amount::ZERO);
    required.insert(conversion.from, current.checked_add(conversion.amount).ok_or(TransactionConsensusError::AmountOverflow)?);
    for (&currency, &tax) in taxes {
        let current = required.get(&currency).copied().unwrap_or(Amount::ZERO);
        required.insert(currency, current.checked_add(tax).ok_or(TransactionConsensusError::AmountOverflow)?);
    }
    for (currency, amount) in required {
        if state.balance(intent.sender, currency).checked_sub(amount).is_none() {
            return Err(TransactionConsensusError::InsufficientBalance);
        }
    }
    if state.exchange_vault_balance(vault_currency).checked_sub(vault_required).is_none() {
        return Err(TransactionConsensusError::InsufficientExchangeReserve);
    }
    Ok(())
}

pub fn calculate_transfer_taxes(
    intent: &AccountIntent,
) -> Result<BTreeMap<Currency, Amount>, TransactionConsensusError> {
    calculate_transfer_taxes_with_rates(intent, |_| TRANSFER_TAX_PERCENT)
}

pub fn calculate_transfer_taxes_with_rates(
    intent: &AccountIntent,
    mut race_rate: impl FnMut(Race) -> u8,
) -> Result<BTreeMap<Currency, Amount>, TransactionConsensusError> {
    let mut totals = BTreeMap::<Currency, Amount>::new();
    for transfer in &intent.transfers {
        let total = totals.get(&transfer.currency).copied().unwrap_or(Amount::ZERO)
            .checked_add(transfer.amount)
            .ok_or(TransactionConsensusError::AmountOverflow)?;
        totals.insert(transfer.currency, total);
    }
    if let Some(conversion) = intent.conversion {
        let total = totals.get(&conversion.from).copied().unwrap_or(Amount::ZERO)
            .checked_add(conversion.amount)
            .ok_or(TransactionConsensusError::AmountOverflow)?;
        totals.insert(conversion.from, total);
    }
    let mut taxes = BTreeMap::new();
    for (currency, total) in totals {
        let rate = currency.required_race().map(&mut race_rate)
            .unwrap_or_else(|| if intent.conversion.is_some_and(|conversion| conversion.from == Currency::Gold) { race_rate(intent.race) } else { TRANSFER_TAX_PERCENT });
        if !(BASE_RACE_TAX_PERCENT..=MAX_RACE_TAX_PERCENT).contains(&rate) {
            return Err(TransactionConsensusError::InvalidTaxPolicy);
        }
        let units = total.as_atomic();
        let whole = (units / 100)
            .checked_mul(u128::from(rate))
            .ok_or(TransactionConsensusError::AmountOverflow)?;
        let remainder = (units % 100) * u128::from(rate);
        let rounded = (remainder + 99) / 100;
        let tax = whole.checked_add(rounded).ok_or(TransactionConsensusError::AmountOverflow)?;
        if tax > 0 {
            taxes.insert(currency, Amount::from_atomic(tax));
        }
    }
    Ok(taxes)
}

fn validate_account_transfer(
    intent: &AccountIntent,
    taxes: &BTreeMap<Currency, Amount>,
    state: &impl TransactionStateView,
) -> Result<(), TransactionConsensusError> {
    if state.nonce(intent.sender) != intent.nonce {
        return Err(TransactionConsensusError::InvalidNonce);
    }
    for transfer in &intent.transfers {
        if transfer.currency.required_race().is_some_and(|race| race != intent.race) {
            return Err(TransactionConsensusError::CurrencyNotOwnedBySenderRace);
        }
        if let Some(required_race) = transfer.currency.required_race()
            && Race::from(transfer.recipient.race()) != required_race
        {
            return Err(TransactionConsensusError::WrongCurrencyRecipientRace);
        }
    }
    let mut required = BTreeMap::<Currency, Amount>::new();
    required.insert(Currency::for_race_reward(intent.race), intent.fee);
    for transfer in &intent.transfers {
        let current = required.get(&transfer.currency).copied().unwrap_or(Amount::ZERO);
        required.insert(transfer.currency, current.checked_add(transfer.amount).ok_or(TransactionConsensusError::AmountOverflow)?);
    }
    for (&currency, &tax) in taxes {
        let current = required.get(&currency).copied().unwrap_or(Amount::ZERO);
        required.insert(currency, current.checked_add(tax).ok_or(TransactionConsensusError::AmountOverflow)?);
    }
    for (currency, amount) in required {
        if state.balance(intent.sender, currency).checked_sub(amount).is_none() {
            return Err(TransactionConsensusError::InsufficientBalance);
        }
    }
    Ok(())
}

#[derive(Debug)]
pub enum TransactionConsensusError {
    Encoding,
    Intent(IntentError),
    InvalidAuthorization,
    InvalidNonce,
    InsufficientBalance,
    InsufficientExchangeReserve,
    InvalidTaxPolicy,
    InvalidConversionAmount,
    StaleExchangeQuote,
    AmountOverflow,
    SenderRaceMismatch,
    CurrencyNotOwnedBySenderRace,
    WrongCurrencyRecipientRace,
}

impl fmt::Display for TransactionConsensusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encoding => formatter.write_str("transaction encoding failed"),
            Self::Intent(error) => write!(formatter, "invalid transaction intent: {error}"),
            Self::InvalidAuthorization => {
                formatter.write_str("transaction authorization is invalid")
            }
            Self::InvalidNonce => {
                formatter.write_str("transaction nonce does not match account state")
            }
            Self::InsufficientBalance => formatter.write_str("account balance is insufficient"),
            Self::InsufficientExchangeReserve => formatter.write_str("nation exchange vault has insufficient reserve"),
            Self::InvalidTaxPolicy => formatter.write_str("race tax rate is outside the protocol range"),
            Self::InvalidConversionAmount => formatter.write_str("conversion amount cannot produce an exact output at this quote"),
            Self::StaleExchangeQuote => formatter.write_str("exchange quote changed; create a new conversion transaction"),
            Self::AmountOverflow => formatter.write_str("transaction amount overflow"),
            Self::SenderRaceMismatch => formatter.write_str("sender address race does not match transaction race"),
            Self::CurrencyNotOwnedBySenderRace => formatter.write_str("sender race cannot spend this race currency"),
            Self::WrongCurrencyRecipientRace => formatter.write_str("race currency recipient belongs to another race"),
        }
    }
}

impl StdError for TransactionConsensusError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Intent(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TestState {
        balance: Amount,
        nonce: u64,
    }
    impl TransactionStateView for TestState {
        fn balance(&self, _address: Address, _currency: Currency) -> Amount {
            self.balance
        }
        fn nonce(&self, _address: Address) -> u64 {
            self.nonce
        }
    }

    #[test]
    fn account_transfer_checks_nonce_and_available_balance() {
        let sender = Address::new([7; crypto::ADDRESS_SIZE]);
        let state = TestState {
            balance: Amount::from_atomic(100),
            nonce: 4,
        };
        let intent = AccountIntent {
            sender,
            race: Race::Accretia,
            register_race: true,
            nonce: 4,
            transfers: vec![crate::transaction::Transfer::new(
                sender,
                Currency::Gold,
                Amount::from_atomic(90),
            )],
            conversion: None,
            fee: Amount::from_atomic(10),
            message: None,
        };
        assert!(validate_account_transfer(&intent, &state).is_ok());
        let mut invalid_nonce = intent.clone();
        invalid_nonce.nonce += 1;
        assert!(matches!(
            validate_account_transfer(&invalid_nonce, &state),
            Err(TransactionConsensusError::InvalidNonce)
        ));
        let mut overdrawn = intent;
        overdrawn.transfers[0].amount = Amount::from_atomic(91);
        assert!(matches!(
            validate_account_transfer(&overdrawn, &state),
            Err(TransactionConsensusError::InsufficientBalance)
        ));
    }
}
