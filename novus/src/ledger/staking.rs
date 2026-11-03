//! Consensus data model for race-native mining stake.
//!
//! Stake is delegated to a race-wide pool and boosts every miner in that race.
//! This state is not connected to finality voting.

use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};
use crypto::Address;

use crate::transaction::{Amount, Race};

/// A participating wallet earns one cumulative point per completed Chip War epoch.
pub const WAR_CONTRIBUTION_POINTS_PER_EPOCH: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct WarRankRecord {
    /// Most recent settled epoch in which this wallet participated.
    pub epoch: u64,
    /// Lifetime contribution total; points are cumulative and are never reset.
    pub contribution_points: u64,
    /// Zero-based rank index, from 0 through 7.
    pub rank_index: u8,
}

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
    /// Wallets that participated through active stake or node heartbeat this epoch.
    epoch_participants: BTreeSet<(Race, Address)>,
    /// Wallets that already submitted a node heartbeat this epoch.
    epoch_heartbeats: BTreeSet<(Race, Address)>,
    /// Cumulative contribution points for each wallet.
    contribution_points: BTreeMap<(Race, Address), u64>,
    /// Latest rank record for each wallet. Rank persists after participation stops.
    latest_war_ranks: BTreeMap<(Race, Address), WarRankRecord>,
    current_epoch: u64,
}

impl StakeState {
    /// Whether no delegation or pending stake state has ever been recorded.
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
            && self.pending.is_empty()
            && self.locked.is_empty()
            && self.withdrawable.is_empty()
            && self.epoch_participants.is_empty()
            && self.epoch_heartbeats.is_empty()
            && self.contribution_points.is_empty()
            && self.latest_war_ranks.is_empty()
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

    /// Number of unique active staking wallets participating in this race's war.
    pub fn active_participants_for_race(&self, race: Race) -> usize {
        self.active.get(&race).map_or(0, BTreeMap::len)
    }

    /// Unique wallets already marked as war participants in the current epoch.
    pub fn epoch_participants_for_race(&self, race: Race) -> usize {
        self.epoch_participants
            .iter()
            .filter(|&&(participant_race, _)| participant_race == race)
            .count()
    }

    /// Lifetime contribution total, including only fully settled war epochs.
    pub fn contribution_points(&self, race: Race, delegator: Address) -> u64 {
        self.contribution_points
            .get(&(race, delegator))
            .copied()
            .unwrap_or(0)
    }

    /// Whether this wallet has earned participation credit in the current epoch so far.
    pub fn participated_this_epoch(&self, race: Race, delegator: Address) -> bool {
        self.epoch_participants.contains(&(race, delegator))
    }

    /// Whether the wallet has already sent its one node heartbeat this epoch.
    pub fn has_war_heartbeat(&self, race: Race, delegator: Address) -> bool {
        self.epoch_heartbeats.contains(&(race, delegator))
    }

    /// Latest rank record. Its epoch is the most recent contribution epoch; rank persists.
    pub fn latest_war_rank(&self, race: Race, delegator: Address) -> Option<WarRankRecord> {
        self.latest_war_ranks.get(&(race, delegator)).copied()
    }

    /// Race-specific UI label for a zero-based rank index.
    pub fn rank_label(race: Race, rank_index: u8) -> Option<&'static str> {
        const BELLATO: [&str; 8] = [
            "Singles", "Doubles", "Minor", "Major", "Carters", "Royal", "Sink", "Maximus",
        ];
        const CORA: [&str; 8] = [
            "1 Knot", "2 Knot", "3 Knot", "4 Knot", "5 Knot", "6 Knot", "7 Knot", "8 Knot",
        ];
        const ACCRETIA: [&str; 8] = [
            "Hastati", "Principe", "Triarii", "Centurio", "Manipel", "Phalanx", "Cohort", "Legion",
        ];
        let labels = match race {
            Race::Bellato => &BELLATO,
            Race::Cora => &CORA,
            Race::Accretia => &ACCRETIA,
        };
        labels.get(usize::from(rank_index)).copied()
    }

    /// Records one epoch participation for every active staker.
    pub(crate) fn record_war_block(&mut self) -> Option<()> {
        for (&race, participants) in &self.active {
            for &delegator in participants.keys() {
                self.epoch_participants.insert((race, delegator));
            }
        }
        Some(())
    }

    /// Marks one signed node heartbeat as participation in this epoch.
    pub(crate) fn record_war_heartbeat(&mut self, race: Race, delegator: Address) -> Option<()> {
        if Race::from(delegator.race()) != race || self.has_war_heartbeat(race, delegator) {
            return None;
        }
        self.epoch_heartbeats.insert((race, delegator));
        self.epoch_participants.insert((race, delegator));
        Some(())
    }

    /// Settles one cumulative point for each unique participant in the completed epoch.
    pub(crate) fn settle_contribution_epoch(&mut self, epoch: u64) -> Option<()> {
        for &(race, delegator) in &self.epoch_participants {
            let key = (race, delegator);
            let total = self.contribution_points(race, delegator).checked_add(1)?;
            self.contribution_points.insert(key, total);
            // Rank 0: 1-8 points; each later rank spans 9 points; rank 7 starts at 63.
            let rank_index = if total <= 8 {
                0
            } else {
                1 + ((total - 9) / 9).min(6) as u8
            };
            self.latest_war_ranks.insert(
                key,
                WarRankRecord {
                    epoch,
                    contribution_points: total,
                    rank_index,
                },
            );
        }
        self.epoch_participants.clear();
        self.epoch_heartbeats.clear();
        Some(())
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
        if amount.as_atomic() < crate::consensus::MIN_WAR_STAKE_ATOMIC
            || Race::from(delegator.race()) != race
        {
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
        if !remaining.is_zero() && remaining.as_atomic() < crate::consensus::MIN_WAR_STAKE_ATOMIC {
            return None;
        }
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
