//! Accretia native currency.

pub use crate::transaction::Amount as Cp;
pub const CURRENCY: crate::transaction::Currency = crate::transaction::Currency::Cp;
pub const RACE: crate::transaction::Race = crate::transaction::Race::Accretia;
pub const SYMBOL: &str = "CP";
