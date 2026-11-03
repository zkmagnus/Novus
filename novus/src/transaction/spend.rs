use borsh::{BorshDeserialize, BorshSerialize};
use crypto::Address;

use crate::error::IntentError;

pub const DECIMALS: u8 = 6;
pub const MAX_SPEND_MESSAGE_BYTES: usize = 256;
/// Base race currency units per Gold before Chip War adjustments.
pub const RACE_CURRENCY_PER_GOLD: u128 = 10_000;
/// Whole Gold units required to mint one whole Novus during one-way conversion.
pub const GOLD_PER_NOVUS: u128 = 1_000;
/// Integer count of the smallest representable unit of any currency.
pub type AtomicAmount = u128;

/// Convert at the active quote, rounding output down to the smallest unit.
pub fn exchange_output_amount(
    input: AtomicAmount,
    from_gold: bool,
    rate: u128,
) -> Option<AtomicAmount> {
    if from_gold {
        input.checked_mul(rate)
    } else {
        input.checked_div(rate)
    }
}

pub fn conversion_output_amount(
    input: AtomicAmount,
    from: Currency,
    to: Currency,
    race_rate: u128,
) -> Option<AtomicAmount> {
    match (from, to) {
        (Currency::Gold, Currency::Novus) => input.checked_div(GOLD_PER_NOVUS),
        (Currency::Gold, Currency::Cp | Currency::Dalant | Currency::Disena) => {
            exchange_output_amount(input, true, race_rate)
        }
        (Currency::Cp | Currency::Dalant | Currency::Disena, Currency::Gold) => {
            exchange_output_amount(input, false, race_rate)
        }
        _ => None,
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize,
)]
pub enum Currency {
    Cp,
    Dalant,
    Disena,
    Gold,
    Novus,
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    BorshSerialize,
    BorshDeserialize,
)]
pub enum Race {
    #[default]
    Accretia,
    Bellato,
    Cora,
}

impl From<crypto::Race> for Race {
    fn from(race: crypto::Race) -> Self {
        match race {
            crypto::Race::Accretia => Self::Accretia,
            crypto::Race::Bellato => Self::Bellato,
            crypto::Race::Cora => Self::Cora,
        }
    }
}

impl From<Race> for crypto::Race {
    fn from(race: Race) -> Self {
        match race {
            Race::Accretia => Self::Accretia,
            Race::Bellato => Self::Bellato,
            Race::Cora => Self::Cora,
        }
    }
}

impl Race {
    pub const fn all() -> [Self; 3] {
        [Self::Accretia, Self::Bellato, Self::Cora]
    }
}

impl Currency {
    pub const fn for_race_reward(race: Race) -> Self {
        match race {
            Race::Accretia => Self::Cp,
            Race::Bellato => Self::Dalant,
            Race::Cora => Self::Disena,
        }
    }

    pub const fn all() -> [Self; 5] {
        [
            Self::Cp,
            Self::Dalant,
            Self::Disena,
            Self::Gold,
            Self::Novus,
        ]
    }

    pub const fn required_race(self) -> Option<Race> {
        match self {
            Self::Cp => Some(Race::Accretia),
            Self::Dalant => Some(Race::Bellato),
            Self::Disena => Some(Race::Cora),
            Self::Gold | Self::Novus => None,
        }
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    BorshSerialize,
    BorshDeserialize,
)]
/// A currency quantity stored as a count of atomic units.
pub struct Amount(AtomicAmount);

impl Amount {
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(1);
    pub const ATOMIC_UNITS_PER_COIN: AtomicAmount = 10u128.pow(DECIMALS as u32);
    pub const fn from_atomic(value: AtomicAmount) -> Self {
        Self(value)
    }
    pub const fn as_atomic(self) -> AtomicAmount {
        self.0
    }
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }
    pub const fn checked_add(self, rhs: Self) -> Option<Self> {
        match self.0.checked_add(rhs.0) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }
    pub const fn checked_sub(self, rhs: Self) -> Option<Self> {
        match self.0.checked_sub(rhs.0) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Transfer {
    pub recipient: Address,
    pub currency: Currency,
    pub amount: Amount,
}

impl Transfer {
    pub const fn new(recipient: Address, currency: Currency, amount: Amount) -> Self {
        Self {
            recipient,
            currency,
            amount,
        }
    }

    pub const fn in_currency(recipient: Address, currency: Currency, amount: Amount) -> Self {
        Self::new(recipient, currency, amount)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AccountIntent {
    pub sender: Address,
    pub race: Race,
    pub register_race: bool,
    pub nonce: u64,
    pub transfers: Vec<Transfer>,
    /// Converts `from` into `to` at the signed protocol quote.
    pub conversion: Option<CurrencyConversion>,
    /// Optional race-native stake operation signed as part of this intent.
    pub stake_action: Option<StakeAction>,
    /// Relay fee, permitted only for Novus transfers and paid to the including miner.
    pub fee: Amount,
    /// Protocol-calculated tax in the source currency, signed for visibility and checked by consensus.
    pub tax: Amount,
    pub message: Option<String>,
}

/// Race-native mining-pool stake action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum StakeAction {
    Delegate { amount: Amount },
    Undelegate { amount: Amount },
    Withdraw { amount: Amount },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct CurrencyConversion {
    pub from: Currency,
    pub to: Currency,
    pub amount: Amount,
    /// Signed rate for this conversion; stale race quotes are rejected.
    pub quote_rate: u128,
}

impl AccountIntent {
    /// Novus transfers pay fees in Novus; other transaction types have no fee currency.
    pub fn fee_currency(&self) -> Currency {
        if self.conversion.is_none()
            && self
                .transfers
                .first()
                .is_some_and(|transfer| transfer.currency == Currency::Novus)
        {
            Currency::Novus
        } else {
            Currency::Gold
        }
    }

    pub fn validate_structure(&self) -> Result<(), IntentError> {
        let has_spend = !self.transfers.is_empty() || self.conversion.is_some();
        let operation_count = usize::from(has_spend) + usize::from(self.stake_action.is_some());
        if operation_count != 1 {
            return Err(IntentError::EmptyOutputs);
        }
        if let Some(action) = self.stake_action {
            let amount = match action {
                StakeAction::Delegate { amount, .. }
                | StakeAction::Undelegate { amount, .. }
                | StakeAction::Withdraw { amount } => amount,
            };
            if amount.is_zero() {
                return Err(IntentError::ZeroAmount);
            }
            if !self.transfers.is_empty()
                || self.conversion.is_some()
                || !self.fee.is_zero()
                || !self.tax.is_zero()
                || self.message.is_some()
                || self.register_race
            {
                return Err(IntentError::InvalidStakeAction);
            }
        }
        if self.message.as_ref().is_some_and(|m| {
            m.is_empty() || m.len() > MAX_SPEND_MESSAGE_BYTES || m.chars().any(char::is_control)
        }) {
            return Err(IntentError::InvalidMessage);
        }
        if self
            .transfers
            .iter()
            .any(|transfer| transfer.amount.is_zero())
        {
            return Err(IntentError::ZeroAmount);
        }
        if let Some(first) = self.transfers.first()
            && self
                .transfers
                .iter()
                .any(|transfer| transfer.currency != first.currency)
        {
            return Err(IntentError::MixedTransferCurrencies);
        }
        if let Some(conversion) = self.conversion {
            if conversion.amount.is_zero() || !self.transfers.is_empty() {
                return Err(IntentError::ZeroAmount);
            }
            if conversion.quote_rate == 0 {
                return Err(IntentError::InvalidConversionAmount);
            }
            if conversion_output_amount(
                conversion.amount.as_atomic(),
                conversion.from,
                conversion.to,
                conversion.quote_rate,
            )
            .is_none()
            {
                return Err(IntentError::InvalidConversionAmount);
            }
        }
        Ok(())
    }
}
