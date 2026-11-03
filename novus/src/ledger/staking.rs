//! Consensus data model for race-native mining stake.
//!
//! Stake is delegated to a race-wide pool and boosts every miner in that race.
//! This state is not connected to finality voting.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use crypto::Address;

use crate::transaction::{Amount, Race};

/// A pending change to a race delegation pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum StakeChangeKind {
    /// Add stake to the active delegation after the activation delay.
    Delegate,
    /// Remove stake from the active delegation and begin unbonding.
    Undelegate,
}

/// A queued delegation change, ordered by the epoch at which it takes effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PendingStakeChange {
    pub effective_epoch: u64,
    pub delegator: Address,
    pub race: Race,
    pub amount: Amount,
    pub kind: StakeChangeKind,
}

/// Active race pools and changes awaiting activation.
#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct StakeState {
    /// `race -> (delegator -> active amount)`.
    active: BTreeMap<Race, BTreeMap<Address, Amount>>,
    /// Changes are applied in `(epoch, race, delegator, kind)` order.
    pending: Vec<PendingStakeChange>,
    /// Amount currently locked, including stake in activation or unbonding.
    locked: BTreeMap<(Race, Address), Amount>,
    /// Unbonded stake ready to be returned by a signed withdraw action.
    withdrawable: BTreeMap<(Race, Address), Amount>,
    current_epoch: u64,
}

impl StakeState {
    /// Whether no delegation or pending stake state has ever been recorded.
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
            && self.pending.is_empty()
            && self.locked.is_empty()
            && self.withdrawable.is_empty()
            && self.current_epoch == 0
    }

    /// The current Chip War stake epoch.
    pub const fn current_epoch(&self) -> u64 {
        self.current_epoch
    }

    /// Total active stake supporting every miner of this race.
    pub fn active_for_race(&self, race: Race) -> Option<Amount> {
        self.active
            .get(&race)
            .into_iter()
            .flat_map(|delegators| delegators.values())
            .try_fold(Amount::ZERO, |sum, value| sum.checked_add(*value))
    }

    /// Active delegators supporting miners in this race.
    pub fn active_positions_for_race(&self, race: Race) -> Vec<(Address, Amount)> {
        self.active
            .get(&race)
            .map(|delegators| {
                delegators
                    .iter()
                    .map(|(&who, &amount)| (who, amount))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Active amount a delegator assigned to a race pool.
    pub fn active_delegation(&self, race: Race, delegator: Address) -> Amount {
        self.active
            .get(&race)
            .and_then(|delegators| delegators.get(&delegator))
            .copied()
            .unwrap_or(Amount::ZERO)
    }

    /// Active stake owned by a delegator in its race pool.
    pub fn active_by_delegator(&self, race: Race, delegator: Address) -> Amount {
        self.active_delegation(race, delegator)
    }

    /// Total amount locked for a delegator in a race pool.
    pub fn locked_by(&self, race: Race, delegator: Address) -> Amount {
        self.locked
            .get(&(race, delegator))
            .copied()
            .unwrap_or(Amount::ZERO)
    }

    /// Stake that has completed its unbonding delay.
    pub fn withdrawable_by(&self, race: Race, delegator: Address) -> Amount {
        self.withdrawable
            .get(&(race, delegator))
            .copied()
            .unwrap_or(Amount::ZERO)
    }

    /// Removes matured stake from escrow after a signed withdraw action.
    pub fn withdraw(&mut self, race: Race, delegator: Address, amount: Amount) -> Option<()> {
        let matured = self.withdrawable_by(race, delegator).checked_sub(amount)?;
        let locked = self.locked_by(race, delegator).checked_sub(amount)?;
        set_amount(&mut self.withdrawable, (race, delegator), matured);
        set_amount(&mut self.locked, (race, delegator), locked);
        Some(())
    }

    /// Schedules delegation activation one complete epoch from now.
    ///
    /// Caller must already have debited and locked `amount` atomically.
    pub fn schedule_delegate(
        &mut self,
        race: Race,
        delegator: Address,
        amount: Amount,
    ) -> Option<()> {
        if amount.is_zero() || Race::from(delegator.race()) != race {
            return None;
        }
        let pending_for_race = self
            .pending
            .iter()
            .filter(|change| change.race == race && change.kind == StakeChangeKind::Delegate)
            .try_fold(Amount::ZERO, |sum, change| sum.checked_add(change.amount))?;
        self.active_for_race(race)?
            .checked_add(pending_for_race)?
            .checked_add(amount)?;
        let locked = self.locked_by(race, delegator).checked_add(amount)?;
        let effective_epoch = self.current_epoch.checked_add(1)?;
        self.locked.insert((race, delegator), locked);
        self.pending.push(PendingStakeChange {
            effective_epoch,
            delegator,
            race,
            amount,
            kind: StakeChangeKind::Delegate,
        });
        Some(())
    }

    /// Schedules stake removal immediately from mining weight and returns it
    /// to the delegator after one complete epoch.
    ///
    /// Caller must enforce transaction balance/nonce rules and keep the stake
    /// locked until the recorded unbonding epoch.
    pub fn schedule_undelegate(
        &mut self,
        race: Race,
        delegator: Address,
        amount: Amount,
    ) -> Option<()> {
        if amount.is_zero() || Race::from(delegator.race()) != race {
            return None;
        }
        let active = self.active_delegation(race, delegator);
        let remaining = active.checked_sub(amount)?;
        let effective_epoch = self.current_epoch.checked_add(1)?;
        let pool = self.active.get_mut(&race)?;
        if remaining.is_zero() {
            pool.remove(&delegator);
        } else {
            pool.insert(delegator, remaining);
        }
        if pool.is_empty() {
            self.active.remove(&race);
        }
        self.pending.push(PendingStakeChange {
            effective_epoch,
            delegator,
            race,
            amount,
            kind: StakeChangeKind::Undelegate,
        });
        Some(())
    }

    /// Advances to a later epoch and applies all due changes.
    /// Returns undelegated amounts that have completed their lock period.
    pub fn advance_epoch(&mut self, epoch: u64) -> Option<Vec<(Race, Address, Amount)>> {
        if epoch < self.current_epoch {
            return None;
        }
        let mut staged = self.clone();
        let withdrawable = staged.advance_epoch_inner(epoch)?;
        *self = staged;
        Some(withdrawable)
    }

    fn advance_epoch_inner(&mut self, epoch: u64) -> Option<Vec<(Race, Address, Amount)>> {
        self.pending.sort_by_key(|change| {
            (
                change.effective_epoch,
                change.race,
                change.delegator,
                change.kind as u8,
            )
        });
        let pending = core::mem::take(&mut self.pending);
        let mut remaining = Vec::with_capacity(pending.len());
        let mut withdrawable = Vec::new();
        for change in pending {
            if change.effective_epoch > epoch {
                remaining.push(change);
                continue;
            }
            match change.kind {
                StakeChangeKind::Delegate => {
                    {
                        let pool = self.active.entry(change.race).or_default();
                        let prior = pool.get(&change.delegator).copied().unwrap_or(Amount::ZERO);
                        pool.insert(change.delegator, prior.checked_add(change.amount)?);
                    }
                    self.active_for_race(change.race)?;
                }
                StakeChangeKind::Undelegate => {
                    let key = (change.race, change.delegator);
                    let matured = self
                        .withdrawable_by(change.race, change.delegator)
                        .checked_add(change.amount)?;
                    self.withdrawable.insert(key, matured);
                    withdrawable.push((change.race, change.delegator, change.amount));
                }
            }
        }
        self.pending = remaining;
        self.current_epoch = epoch;
        Some(withdrawable)
    }
}

fn set_amount<K: Ord + Copy>(map: &mut BTreeMap<K, Amount>, key: K, amount: Amount) {
    if amount.is_zero() {
        map.remove(&key);
    } else {
        map.insert(key, amount);
    }
}
