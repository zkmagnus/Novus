use super::*;
use super::{config::*, gossip::*, mempool::*, state::*, util::*};
use novus::consensus::{MEDIAN_TIME_PAST_WINDOW, reward_for_previous_hash, validate_timestamp};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn mining_loop(database: PathBuf, miner: Address) {
    let mut next_nonce = 0_u64;
    println!("mining_state: ready");
    loop {
        match mine_block_database(&database, miner, next_nonce, 1_000_000) {
            Ok(MiningAttempt::Mined) => next_nonce = 0,
            Ok(MiningAttempt::Exhausted { next }) => next_nonce = next,
            Err(error) => {
                next_nonce = 0;
                eprintln!("node: mining attempt failed: {error}");
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    }
}

pub(super) enum MiningAttempt {
    Mined,
    Exhausted { next: u64 },
}

pub(super) fn mine_block_database(
    database: &Path,
    miner: Address,
    start_nonce: u64,
    attempts: u64,
) -> Result<MiningAttempt, String> {
    let ledger = load_or_initialize(database)?;
    let mempool = read_mempool(database)?;
    let transactions = select_block_transactions(&ledger, miner, &mempool)?;
    validate_mempool(&ledger, &transactions)?;
    let mut block = candidate_block(&ledger, miner, transactions.clone())?;
    block
        .validate_structure()
        .map_err(|error| format!("mining candidate is invalid: {error}"))?;
    let height = block.height();
    let race = novus::transaction::Race::from(miner.race());
    let active_stake = ledger
        .state()
        .staking
        .active_for_race(race)
        .ok_or("active stake total overflow")?;
    let network_target = novus::consensus::PoWTarget::from_compact(block.target_bits())
        .ok_or("network target is invalid")?;
    let pow_limit = novus::consensus::stake_target_limit();
    let target = novus::consensus::effective_target(network_target, active_stake, pow_limit)
        .ok_or("stake-adjusted mining target is invalid")?;
    let found = crate::miner::mine_range_at_target(
        &mut block,
        crate::miner::MiningRange {
            start_nonce,
            attempts,
        },
        target,
    )
    .map_err(|error| error.to_string())?;
    if found.is_none() {
        return Ok(MiningAttempt::Exhausted {
            next: start_nonce.wrapping_add(attempts),
        });
    }
    let _mutation = state_mutation_lock()?
        .lock()
        .map_err(|_| "state mutation lock is poisoned")?;
    let mut ledger = load_or_initialize_owned(database)?;
    if ledger.tip_hash().map(|hash| hash.0) != Some(block.previous_hash().0) {
        return Err("mined candidate became stale while mining".into());
    }
    apply_block(&mut ledger, block.clone()).map_err(|error| error.to_string())?;
    let included = transactions
        .iter()
        .map(|transaction| transaction.id().map_err(|error| error.to_string()))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let remaining = reconcile_mempool(&ledger, read_mempool(database)?, &included);
    persist_block_and_mempool(database, &block, &remaining)?;
    let _ = update_ledger_cache(database, ledger)?;
    notify_gossip();
    println!(
        "mined height={} nonce={} hash={}",
        height.0,
        block.header.nonce.0,
        hex::encode(block.hash().map_err(|error| error.to_string())?.0)
    );
    Ok(MiningAttempt::Mined)
}

pub(super) fn mine_one_block(path: Option<&str>, miner: &str) -> Result<(), String> {
    let database = database_path(path);
    let ledger = load_or_initialize(&database)?;
    let miner = resolve_miner(&ledger, miner)?;
    let mut next_nonce = 0_u64;
    loop {
        match mine_block_database(&database, miner, next_nonce, 1_000_000)? {
            MiningAttempt::Mined => return Ok(()),
            MiningAttempt::Exhausted { next } => next_nonce = next,
        }
    }
}

pub(super) fn select_block_transactions(
    ledger: &Ledger,
    miner: Address,
    mempool: &[Transaction],
) -> Result<Vec<Transaction>, String> {
    let mut selected = Vec::new();
    for transaction in mempool {
        let mut candidate = selected.clone();
        candidate.push(transaction.clone());
        let block = candidate_block_uncommitted(ledger, miner, candidate.clone())?;
        if block.block_size() as usize > novus::blockchain::MAX_BLOCK_SIZE {
            break;
        }
        selected = candidate;
    }
    Ok(selected)
}

pub(super) fn candidate_block(
    ledger: &Ledger,
    miner: Address,
    transactions: Vec<Transaction>,
) -> Result<Block, String> {
    let mut block = candidate_block_uncommitted(ledger, miner, transactions)?;
    let (state_root, block_size) = ledger
        .preview_block_commitments(&block)
        .map_err(|error| error.to_string())?;

    block.set_state_root(state_root);
    block.set_block_size(block_size);

    Ok(block)
}

fn candidate_block_uncommitted(
    ledger: &Ledger,
    miner: Address,
    transactions: Vec<Transaction>,
) -> Result<Block, String> {
    let height = Height(
        ledger
            .tip_height()
            .map_or(0, |height| height.0.saturating_add(1)),
    );

    let previous = ledger.tip_hash().ok_or("canonical genesis is missing")?;

    let difficulty = expected_next_difficulty(&ledger.chain).map_err(|error| error.to_string())?;

    let subsidy = expected_next_emission(ledger)?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is before Unix epoch")?
        .as_secs();
    let previous_timestamps = ledger
        .chain
        .blocks()
        .rev()
        .take(MEDIAN_TIME_PAST_WINDOW)
        .map(Block::timestamp)
        .collect::<Vec<_>>();
    let mut ordered_timestamps = previous_timestamps.clone();
    ordered_timestamps.sort_unstable();
    let median_time_past = ordered_timestamps
        .get(ordered_timestamps.len() / 2)
        .copied()
        .unwrap_or_default();
    let timestamp = now.max(median_time_past.saturating_add(1));
    validate_timestamp(timestamp, &previous_timestamps, now)
        .map_err(|error| format!("cannot create mining candidate: {error}"))?;

    Block::from_protocol_transactions(
        height,
        previous,
        difficulty,
        timestamp,
        Nonce(0),
        Some(Emission::new(miner, subsidy)),
        transactions,
    )
    .map_err(|error| error.to_string())
}
pub(super) fn expected_next_emission(ledger: &Ledger) -> Result<Amount, String> {
    let previous = ledger.tip_hash().ok_or("canonical genesis is missing")?;
    Ok(reward_for_previous_hash(previous.0))
}
