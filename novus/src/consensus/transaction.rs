use std::collections::BTreeMap;
use std::{error::Error as StdError, fmt};

use common::ChainContext;
use crypto::{Address, TransactionHash};

use super::{
    GOLD_TO_NOVUS_CONVERSION_TAX_PERCENT, GOLD_TO_RACE_CONVERSION_TAX_PERCENT,
    GOLD_TRANSFER_TAX_PERCENT, RACE_TO_GOLD_CONVERSION_TAX_PERCENT, TRANSFER_TAX_PERCENT,
};
use crate::transaction::{AccountIntent, Amount, Currency, IntentError, Race, Transaction};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedTransaction {
    pub intent: AccountIntent,
    pub txid: TransactionHash,
    pub taxes: BTreeMap<Currency, Amount>,
}

pub trait TransactionStateView {
    fn balance(&self, address: Address, currency: Currency) -> Amount;
    fn race(&self, _address: Address) -> Option<Race> {
        None
    }
    fn exchange_vault_balance(&self, _currency: Currency) -> Amount {
        Amount::ZERO
    }
    fn race_tax_rate(&self, _race: Race) -> u8 {
        crate::ledger::BASE_RACE_TAX_MODIFIER_PERCENT
    }
    fn race_exchange_rate(&self, _race: Race) -> u128 {
        crate::transaction::RACE_CURRENCY_PER_GOLD
    }
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
    if state
        .race(intent.sender)
        .is_some_and(|race| race != intent.race)
        || (state.race(intent.sender).is_none() && !intent.register_race)
    {
        return Err(TransactionConsensusError::SenderRaceMismatch);
    }
    if intent.register_race && state.race(intent.sender).is_some() {
        return Err(TransactionConsensusError::SenderRaceMismatch);
    }
    let taxes = calculate_transfer_taxes_with_policy(
        intent,
        |race| state.race_tax_rate(race),
        |race| state.race_exchange_rate(race),
    )?;
    let expected_tax = taxes.values().try_fold(Amount::ZERO, |sum, tax| {
        sum.checked_add(*tax)
            .ok_or(TransactionConsensusError::AmountOverflow)
    })?;
    if intent.tax != expected_tax {
        return Err(TransactionConsensusError::InvalidTaxAmount);
    }
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
    let active_rate = if (conversion.from, conversion.to) == (Currency::Gold, Currency::Novus) {
        crate::transaction::GOLD_PER_NOVUS
    } else {
        state.race_exchange_rate(intent.race)
    };
    if conversion.quote_rate != active_rate {
        return Err(TransactionConsensusError::StaleExchangeQuote);
    }
    if conversion
        .from
        .required_race()
        .is_some_and(|race| race != intent.race)
        || conversion
            .to
            .required_race()
            .is_some_and(|race| race != intent.race)
    {
        return Err(TransactionConsensusError::CurrencyNotOwnedBySenderRace);
    }
    let payout = crate::transaction::conversion_output_amount(
        conversion.amount.as_atomic(),
        conversion.from,
        conversion.to,
        active_rate,
    )
    .ok_or(TransactionConsensusError::InvalidConversionAmount)?;
    if payout == 0 {
        return Err(TransactionConsensusError::InvalidConversionAmount);
    }
    let mut required = BTreeMap::new();
    required.insert(intent.fee_currency(), intent.fee);
    let (vault_currency, vault_required) = match (conversion.from, conversion.to) {
        (Currency::Gold, Currency::Novus) => (Currency::Gold, Amount::ZERO),
        (Currency::Gold, _) => (conversion.to, Amount::from_atomic(payout)),
        (_, Currency::Gold) => (Currency::Gold, Amount::from_atomic(payout)),
        _ => return Err(TransactionConsensusError::InvalidConversionAmount),
    };
    let current = required
        .get(&conversion.from)
        .copied()
        .unwrap_or(Amount::ZERO);
    required.insert(
        conversion.from,
        current
            .checked_add(conversion.amount)
            .ok_or(TransactionConsensusError::AmountOverflow)?,
    );
    for (&currency, &tax) in taxes {
        let current = required.get(&currency).copied().unwrap_or(Amount::ZERO);
        required.insert(
            currency,
            current
                .checked_add(tax)
                .ok_or(TransactionConsensusError::AmountOverflow)?,
        );
    }
    for (currency, amount) in required {
        if state
            .balance(intent.sender, currency)
            .checked_sub(amount)
            .is_none()
        {
            return Err(TransactionConsensusError::InsufficientBalance);
        }
    }
    if state
        .exchange_vault_balance(vault_currency)
        .checked_sub(vault_required)
        .is_none()
    {
        return Err(TransactionConsensusError::InsufficientExchangeReserve);
    }
    Ok(())
}

pub fn calculate_transfer_taxes(
    intent: &AccountIntent,
) -> Result<BTreeMap<Currency, Amount>, TransactionConsensusError> {
    calculate_transfer_taxes_with_rates(intent, |_| crate::ledger::BASE_RACE_TAX_MODIFIER_PERCENT)
}

pub fn calculate_transfer_taxes_with_rates(
    intent: &AccountIntent,
    race_tax_rate: impl FnMut(Race) -> u8,
) -> Result<BTreeMap<Currency, Amount>, TransactionConsensusError> {
    calculate_transfer_taxes_with_policy(intent, race_tax_rate, |_| {
        crate::transaction::RACE_CURRENCY_PER_GOLD
    })
}

pub fn calculate_transfer_taxes_with_policy(
    intent: &AccountIntent,
    mut race_tax_rate: impl FnMut(Race) -> u8,
    _race_exchange_rate: impl FnMut(Race) -> u128,
) -> Result<BTreeMap<Currency, Amount>, TransactionConsensusError> {
    let mut taxes = BTreeMap::new();
    let (currency, amount, rate) = if let Some(conversion) = intent.conversion {
        let base_rate = match (conversion.from, conversion.to) {
            (Currency::Cp | Currency::Dalant | Currency::Disena, Currency::Gold) => {
                RACE_TO_GOLD_CONVERSION_TAX_PERCENT
            }
            (Currency::Gold, Currency::Novus) => GOLD_TO_NOVUS_CONVERSION_TAX_PERCENT,
            (Currency::Gold, Currency::Cp | Currency::Dalant | Currency::Disena) => {
                GOLD_TO_RACE_CONVERSION_TAX_PERCENT
            }
            _ => return Err(TransactionConsensusError::InvalidConversionAmount),
        };
        let rate = apply_race_tax_adjustment(base_rate, race_tax_rate(intent.race))?;
        (conversion.from, conversion.amount, rate)
    } else {
        let first = intent
            .transfers
            .first()
            .ok_or(TransactionConsensusError::InvalidConversionAmount)?;
        let base_rate = match first.currency {
            Currency::Cp | Currency::Dalant | Currency::Disena => TRANSFER_TAX_PERCENT,
            Currency::Gold => GOLD_TRANSFER_TAX_PERCENT,
            Currency::Novus => 0,
        };
        let rate = if base_rate == 0 {
            0
        } else {
            apply_race_tax_adjustment(base_rate, race_tax_rate(intent.race))?
        };
        let total = intent
            .transfers
            .iter()
            .try_fold(Amount::ZERO, |sum, transfer| {
                sum.checked_add(transfer.amount)
                    .ok_or(TransactionConsensusError::AmountOverflow)
            })?;
        (first.currency, total, rate)
    };
    if rate > 0 {
        let tax = percent_ceil(amount.as_atomic(), rate)?;
        if tax > 0 {
            taxes.insert(currency, Amount::from_atomic(tax));
        }
    }
    Ok(taxes)
}

fn apply_race_tax_adjustment(
    base_rate: u8,
    current_race_rate: u8,
) -> Result<u8, TransactionConsensusError> {
    if !(crate::ledger::MIN_RACE_TAX_MODIFIER_PERCENT
        ..=crate::ledger::MAX_RACE_TAX_MODIFIER_PERCENT)
        .contains(&current_race_rate)
    {
        return Err(TransactionConsensusError::InvalidTaxPolicy);
    }
    let adjusted = (i16::from(base_rate) + i16::from(current_race_rate)).clamp(
        i16::from(base_rate),
        i16::from(crate::ledger::MAX_EFFECTIVE_TAX_PERCENT),
    );
    Ok(adjusted as u8)
}

fn percent_ceil(amount: u128, percent: u8) -> Result<u128, TransactionConsensusError> {
    (amount / 100)
        .checked_mul(u128::from(percent))
        .and_then(|whole| whole.checked_add((amount % 100 * u128::from(percent)).div_ceil(100)))
        .ok_or(TransactionConsensusError::AmountOverflow)
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
        if transfer.currency == Currency::Gold
            && Race::from(transfer.recipient.race()) != intent.race
        {
            return Err(TransactionConsensusError::WrongCurrencyRecipientRace);
        }
        if transfer
            .currency
            .required_race()
            .is_some_and(|race| race != intent.race)
        {
            return Err(TransactionConsensusError::CurrencyNotOwnedBySenderRace);
        }
        if let Some(required_race) = transfer.currency.required_race()
            && Race::from(transfer.recipient.race()) != required_race
        {
            return Err(TransactionConsensusError::WrongCurrencyRecipientRace);
        }
    }
    let mut required = BTreeMap::<Currency, Amount>::new();
    required.insert(intent.fee_currency(), intent.fee);
    for transfer in &intent.transfers {
        let current = required
            .get(&transfer.currency)
            .copied()
            .unwrap_or(Amount::ZERO);
        required.insert(
            transfer.currency,
            current
                .checked_add(transfer.amount)
                .ok_or(TransactionConsensusError::AmountOverflow)?,
        );
    }
    for (&currency, &tax) in taxes {
        let current = required.get(&currency).copied().unwrap_or(Amount::ZERO);
        required.insert(
            currency,
            current
                .checked_add(tax)
                .ok_or(TransactionConsensusError::AmountOverflow)?,
        );
    }
    for (currency, amount) in required {
        if state
            .balance(intent.sender, currency)
            .checked_sub(amount)
            .is_none()
        {
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
    InvalidTaxAmount,
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
            Self::InsufficientExchangeReserve => {
                formatter.write_str("nation exchange vault has insufficient reserve")
            }
            Self::InvalidTaxPolicy => {
                formatter.write_str("race tax rate is outside the protocol range")
            }
            Self::InvalidTaxAmount => formatter.write_str(
                "signed tax does not match the active protocol tax; create a new transaction",
            ),
            Self::InvalidConversionAmount => formatter
                .write_str("conversion amount cannot produce an exact output at this quote"),
            Self::StaleExchangeQuote => {
                formatter.write_str("exchange quote changed; create a new conversion transaction")
            }
            Self::AmountOverflow => formatter.write_str("transaction amount overflow"),
            Self::SenderRaceMismatch => {
                formatter.write_str("sender address race does not match transaction race")
            }
            Self::CurrencyNotOwnedBySenderRace => {
                formatter.write_str("sender race cannot spend this race currency")
            }
            Self::WrongCurrencyRecipientRace => {
                formatter.write_str("race currency recipient belongs to another race")
            }
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
            tax: Amount::ZERO,
            message: None,
        };
        let taxes = BTreeMap::new();
        assert!(validate_account_transfer(&intent, &taxes, &state).is_ok());
        let mut invalid_nonce = intent.clone();
        invalid_nonce.nonce += 1;
        assert!(matches!(
            validate_account_transfer(&invalid_nonce, &taxes, &state),
            Err(TransactionConsensusError::InvalidNonce)
        ));
        let mut overdrawn = intent;
        overdrawn.transfers[0].amount = Amount::from_atomic(101);
        assert!(matches!(
            validate_account_transfer(&overdrawn, &taxes, &state),
            Err(TransactionConsensusError::InsufficientBalance)
        ));
    }

    #[test]
    fn fee_and_tax_follow_the_transferred_currency() {
        let sender = Address::new([7; crypto::ADDRESS_SIZE]);
        let amount = Amount::from_atomic(1_000);
        let mut intent = AccountIntent {
            sender,
            race: Race::Accretia,
            register_race: true,
            nonce: 0,
            transfers: vec![crate::transaction::Transfer::new(
                sender,
                Currency::Gold,
                amount,
            )],
            conversion: None,
            fee: Amount::from_atomic(10),
            tax: Amount::ZERO,
            message: None,
        };

        assert_eq!(intent.fee_currency(), Currency::Gold);
        let taxes = calculate_transfer_taxes(&intent).unwrap();
        assert_eq!(taxes.get(&Currency::Gold), Some(&Amount::from_atomic(20)));

        intent.transfers[0].currency = Currency::Cp;
        assert_eq!(intent.fee_currency(), Currency::Gold);
        let taxes = calculate_transfer_taxes(&intent).unwrap();
        assert_eq!(taxes.get(&Currency::Cp), Some(&Amount::from_atomic(10)));
    }

    #[test]
    fn transfer_intent_cannot_mix_fee_currencies() {
        let sender = Address::new([7; crypto::ADDRESS_SIZE]);
        let mut intent = AccountIntent {
            sender,
            race: Race::Accretia,
            register_race: true,
            nonce: 0,
            transfers: vec![
                crate::transaction::Transfer::new(sender, Currency::Gold, Amount::from_atomic(1)),
                crate::transaction::Transfer::new(sender, Currency::Cp, Amount::from_atomic(1)),
            ],
            conversion: None,
            fee: Amount::ZERO,
            tax: Amount::ZERO,
            message: None,
        };

        assert!(matches!(
            intent.validate_structure(),
            Err(IntentError::MixedTransferCurrencies)
        ));

        intent.transfers.pop();
        assert!(intent.validate_structure().is_ok());
    }
}
