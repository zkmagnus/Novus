//! Canonical block-count Chip War epochs and race economy modifiers.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};

use crate::{consensus::TARGET_BLOCK_TIME_SECONDS, transaction::Race};

pub const CHIP_WAR_EPOCH_SECONDS: u64 = 3 * 24 * 60 * 60;
pub const CHIP_WAR_EPOCH_BLOCKS: u64 = CHIP_WAR_EPOCH_SECONDS / TARGET_BLOCK_TIME_SECONDS;
/// Chip War tax modifier starts at the base rates, so its initial offset is zero.
pub const BASE_RACE_TAX_MODIFIER_PERCENT: u8 = 0;
pub const MIN_RACE_TAX_MODIFIER_PERCENT: u8 = 0;
/// A +19 point modifier brings the 1% base transfer tax to the 20% effective cap.
pub const MAX_RACE_TAX_MODIFIER_PERCENT: u8 = 19;
pub const MAX_EFFECTIVE_TAX_PERCENT: u8 = 20;
pub const RACE_TAX_LOSS_INCREASE_PERCENT: u8 = 2;
pub const RACE_TAX_FAILURE_INCREASE_PERCENT: u8 = 1;
pub const RACE_TAX_WIN_DECREASE_PERCENT: u8 = 1;
pub const EXCHANGE_RATE_STEP_PERCENT: i8 = 5;
pub const MAX_EXCHANGE_RATE_DEVIATION_PERCENT: i8 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum RaceWarOutcome {
    Winner,
    Loser,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ChipWarState {
    /// Number of fully settled epochs.
    pub completed_epochs: u64,
    /// Canonical blocks counted in the current epoch so far.
    pub block_counts: BTreeMap<Race, u64>,
    /// Chip War tax modifier in percentage points above each transaction's base rate.
    pub tax_rates: BTreeMap<Race, u8>,
    /// Signed percentage offset from the 10,000:1 base quote.
    pub exchange_rate_adjustments: BTreeMap<Race, i8>,
    /// Outcome of the last settled epoch.
    pub last_outcomes: BTreeMap<Race, RaceWarOutcome>,
}

impl Default for ChipWarState {
    fn default() -> Self {
        let mut block_counts = BTreeMap::new();
        let mut tax_rates = BTreeMap::new();
        let mut exchange_rate_adjustments = BTreeMap::new();
        for race in Race::all() {
            block_counts.insert(race, 0);
            tax_rates.insert(race, BASE_RACE_TAX_MODIFIER_PERCENT);
            exchange_rate_adjustments.insert(race, 0);
        }
        Self {
            completed_epochs: 0,
            block_counts,
            tax_rates,
            exchange_rate_adjustments,
            last_outcomes: BTreeMap::new(),
        }
    }
}

impl ChipWarState {
    pub fn tax_rate(&self, race: Race) -> u8 {
        self.tax_rates
            .get(&race)
            .copied()
            .unwrap_or(BASE_RACE_TAX_MODIFIER_PERCENT)
    }

    pub fn exchange_rate(&self, race: Race) -> u128 {
        let adjustment = self
            .exchange_rate_adjustments
            .get(&race)
            .copied()
            .unwrap_or(0);
        crate::transaction::RACE_CURRENCY_PER_GOLD * (100 + i128::from(adjustment)) as u128 / 100
    }

    pub fn record_block(&mut self, height: u64, race: Race) -> Option<ChipWarRollback> {
        let previous = self.clone();
        let count = self
            .block_counts
            .get(&race)
            .copied()
            .unwrap_or(0)
            .checked_add(1)?;
        self.block_counts.insert(race, count);
        if height.is_multiple_of(CHIP_WAR_EPOCH_BLOCKS) && self.settle_epoch().is_none() {
            *self = previous;
            return None;
        }
        Some(ChipWarRollback { previous })
    }

    fn settle_epoch(&mut self) -> Option<()> {
        let high = self.block_counts.values().copied().max()?;
        let top = self
            .block_counts
            .iter()
            .filter_map(|(&race, &count)| (count == high).then_some(race))
            .collect::<Vec<_>>();
        let unique_scores = self
            .block_counts
            .values()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let mut outcomes = BTreeMap::new();
        if unique_scores != Race::all().len() || top.len() != 1 {
            for race in Race::all() {
                outcomes.insert(race, RaceWarOutcome::Failure);
            }
        } else {
            let winner = top[0];
            let low = self.block_counts.values().copied().min()?;
            let loser = self
                .block_counts
                .iter()
                .find_map(|(&race, &count)| (count == low).then_some(race))?;
            for race in Race::all() {
                let outcome = if race == winner {
                    RaceWarOutcome::Winner
                } else if race == loser {
                    RaceWarOutcome::Loser
                } else {
                    RaceWarOutcome::Failure
                };
                outcomes.insert(race, outcome);
            }
        }
        for race in Race::all() {
            let outcome = outcomes[&race];
            let old = self.tax_rate(race);
            let next = match outcome {
                RaceWarOutcome::Winner => old
                    .saturating_sub(RACE_TAX_WIN_DECREASE_PERCENT)
                    .max(MIN_RACE_TAX_MODIFIER_PERCENT),
                RaceWarOutcome::Loser => old
                    .saturating_add(RACE_TAX_LOSS_INCREASE_PERCENT)
                    .min(MAX_RACE_TAX_MODIFIER_PERCENT),
                RaceWarOutcome::Failure => old
                    .saturating_add(RACE_TAX_FAILURE_INCREASE_PERCENT)
                    .min(MAX_RACE_TAX_MODIFIER_PERCENT),
            };
            self.tax_rates.insert(race, next);
            let old_adjustment = self
                .exchange_rate_adjustments
                .get(&race)
                .copied()
                .unwrap_or(0);
            let next_adjustment = match outcome {
                RaceWarOutcome::Winner => old_adjustment
                    .saturating_sub(EXCHANGE_RATE_STEP_PERCENT)
                    .max(-MAX_EXCHANGE_RATE_DEVIATION_PERCENT),
                RaceWarOutcome::Loser => old_adjustment
                    .saturating_add(EXCHANGE_RATE_STEP_PERCENT)
                    .min(MAX_EXCHANGE_RATE_DEVIATION_PERCENT),
                RaceWarOutcome::Failure => old_adjustment,
            };
            self.exchange_rate_adjustments.insert(race, next_adjustment);
        }
        self.last_outcomes = outcomes;
        self.completed_epochs = self.completed_epochs.checked_add(1)?;
        for race in Race::all() {
            self.block_counts.insert(race, 0);
        }
        Some(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ChipWarRollback {
    previous: ChipWarState,
}

impl ChipWarState {
    pub(crate) fn rollback(&mut self, journal: ChipWarRollback) {
        *self = journal.previous;
    }
}
