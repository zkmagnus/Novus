use borsh::{BorshDeserialize, BorshSerialize};
use crypto::Address;

use crate::error::IntentError;

pub const DECIMALS: u8 = 3;
pub const MAX_SPEND_MESSAGE_BYTES: usize = 256;
/// Base race currency units per Gold before Chip War adjustments.
pub const RACE_CURRENCY_PER_GOLD: u128 = 100;
/// Integer count of the smallest representable unit of any currency.
pub type AtomicAmount = u128;

/// Convert at the active quote, rounding output down to the smallest unit.
pub fn exchange_output_amount(input: AtomicAmount, from_gold: bool, rate: u128) -> Option<AtomicAmount> {
    let (multiplier, divisor) = if from_gold { (rate, RACE_CURRENCY_PER_GOLD) } else { (RACE_CURRENCY_PER_GOLD, rate) };
    if divisor == 0 { return None; }
    let whole = (input / divisor).checked_mul(multiplier)?;
    let remainder = (input % divisor).checked_mul(multiplier)?;
    whole.checked_add(remainder / divisor)
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize,
)]
pub enum Currency {
    Cp,
    Dalant,
    Disena,
    Gold,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize,
)]
pub enum Race {
    #[default]
    Accretia,
    Bellato,
    Cora,
}

impl From<crypto::Race> for Race {
    fn from(race: crypto::Race) -> Self {
        match race { crypto::Race::Accretia => Self::Accretia, crypto::Race::Bellato => Self::Bellato, crypto::Race::Cora => Self::Cora }
    }
}

impl From<Race> for crypto::Race {
    fn from(race: Race) -> Self {
        match race { Race::Accretia => Self::Accretia, Race::Bellato => Self::Bellato, Race::Cora => Self::Cora }
    }
}

impl Race {
    pub const fn all() -> [Self; 3] { [Self::Accretia, Self::Bellato, Self::Cora] }
}

impl Currency {
    pub const fn for_race_reward(race: Race) -> Self {
        match race {
            Race::Accretia => Self::Cp,
            Race::Bellato => Self::Dalant,
            Race::Cora => Self::Disena,
        }
    }

    pub const fn all() -> [Self; 4] {
        [Self::Cp, Self::Dalant, Self::Disena, Self::Gold]
    }

    pub const fn required_race(self) -> Option<Race> {
        match self {
            Self::Cp => Some(Race::Accretia),
            Self::Dalant => Some(Race::Bellato),
            Self::Disena => Some(Race::Cora),
            Self::Gold => None,
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
        Self { recipient, currency, amount }
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
    /// Converts the selected currency at the signed Chip War quote.
    pub conversion: Option<CurrencyConversion>,
    /// Paid in the native currency selected by `race`.
    pub fee: Amount,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct CurrencyConversion {
    pub from: Currency,
    pub amount: Amount,
    /// Signed quote in race currency units per Gold; stale quotes are rejected.
    pub quote_rate: u128,
}

impl AccountIntent {
    pub fn validate_structure(&self) -> Result<(), IntentError> {
        if self.transfers.is_empty() == self.conversion.is_none() {
            return Err(IntentError::EmptyOutputs);
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
        if let Some(conversion) = self.conversion {
            if conversion.amount.is_zero() || !self.transfers.is_empty() {
                return Err(IntentError::ZeroAmount);
            }
            if conversion.quote_rate == 0 {
                return Err(IntentError::InvalidConversionAmount);
            }
        }
        Ok(())
    }
}
