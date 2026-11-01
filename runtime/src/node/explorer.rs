use super::*;
use super::{config::*, state::*, util::*};

pub(super) const DEFAULT_ADDRESS_ACTIVITY_LIMIT: usize = 50;
pub(super) const MAX_ADDRESS_ACTIVITY_LIMIT: usize = 250;

pub(super) fn print_account(path: Option<&str>, address: &str) -> Result<(), String> {
    let database = database_path(path);
    let ledger = load_or_initialize(&database)?;
    let address = parse_address(address)?;
    let response = account_response(&ledger, address)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&response).map_err(|error| error.to_string())?
    );
    Ok(())
}

pub(super) fn account_response(
    ledger: &Ledger,
    address: Address,
) -> Result<serde_json::Value, String> {
    let account = ledger.state().accounts.get(address);
    Ok(serde_json::json!({
        "address": novus::crypto::address_to_string(&address, address.race().prefix()),
        "tip_height": ledger.tip_height().map_or(0, |height| height.0),
        "balance": account.balance().as_atomic(),
        "balances": account.balances.iter().map(|(currency, amount)| (format!("{currency:?}").to_lowercase(), amount.as_atomic())).collect::<std::collections::BTreeMap<_, _>>(),
        "nonce": account.nonce,
    }))
}

pub(super) fn balance_response(
    ledger: &Ledger,
    mempool: &[Transaction],
    address: Address,
) -> Result<serde_json::Value, String> {
    let account = ledger.state().accounts.get(address);
    let total = account.balance();
    let mut next_nonce = account.nonce;
    loop {
        if !mempool
            .iter()
            .any(|tx| tx.intent.sender == address && tx.intent.nonce == next_nonce)
        {
            break;
        }
        let Some(next) = next_nonce.checked_add(1) else {
            break;
        };
        next_nonce = next;
    }
    Ok(serde_json::json!({
        "address": novus::crypto::address_to_string(&address, address.race().prefix()),
        "tip_height": ledger.tip_height().map_or(0, |height| height.0),
        "total": total.as_atomic(),
        "available": total.as_atomic(),
        "balances": account.balances.iter().map(|(currency, amount)| (format!("{currency:?}").to_lowercase(), amount.as_atomic())).collect::<std::collections::BTreeMap<_, _>>(),
        "nonce": next_nonce,
        "reserved": 0,
        "immature": 0,
    }))
}

pub(super) fn explorer_address_response(
    database: &Path,
    ledger: &Ledger,
    _mempool: &[Transaction],
    address: Address,
    include_emissions: bool,
    limit: usize,
    before: Option<[u8; crate::storage::ADDRESS_ACTIVITY_CURSOR_SIZE]>,
) -> Result<serde_json::Value, String> {
    let account = ledger.state().accounts.get(address);
    let total = account.balance();
    let reserved = Amount::ZERO;
    let immature = Amount::ZERO;
    let page = index::address_activity_page(database, ledger, address, before, limit)?;
    let mut activities = Vec::new();
    let mut emission_count = 0usize;
    for location in &page.locations {
        match *location {
            index::ActivityLocation::Emission { height } => {
                emission_count = emission_count.saturating_add(1);
                if !include_emissions {
                    continue;
                }

                let block = ledger
                    .chain
                    .block(&height)
                    .ok_or("indexed emission block is missing from the canonical chain")?;

                let emission = block
                    .emission()
                    .filter(|emission| emission.to == address)
                    .ok_or("indexed emission does not match the canonical chain")?;

                activities.push(serde_json::json!({
                    "height": block.height().0,
                    "block_hash": hex::encode(
                        block.hash().map_err(|error| error.to_string())?.0
                    ),
                    "hash": serde_json::Value::Null,
                    "type": "emission",
                    "direction": "in",
                    "amount": emission.subsidy.as_atomic(),
                    "gross_subsidy": emission.subsidy.as_atomic(),
                    "size_bytes": serde_json::Value::Null,
                }));
            }

            index::ActivityLocation::Transaction {
                height,
                transaction_index,
            } => {
                let block = ledger
                    .chain
                    .block(&height)
                    .ok_or("indexed activity block is missing from the canonical chain")?;

                let transaction = block
                    .transactions()
                    .get(transaction_index)
                    .ok_or("indexed activity transaction is missing from its block")?;

                let tax_rate = historical_race_tax_rate(ledger, block.height().0, transaction.intent.race);
                let exchange_rate = historical_exchange_rate(ledger, block.height().0, transaction.intent.race);
                if let Some(activity) = address_transaction_activity(transaction, address, block, tax_rate, exchange_rate)? {
                    activities.push(activity);
                }
            }
        }
    }

    let next_cursor = page.next_cursor.map(hex::encode);

    Ok(serde_json::json!({
        "address": novus::crypto::address_to_string(&address, address.race().prefix()),
        "tip_height": ledger.tip_height().map_or(0, |height| height.0),
        "balance": {
            "total": total.as_atomic(),
            "reserved": reserved.as_atomic(),
            "immature": immature.as_atomic(),
        },
        "activity_count": activities.len(),
        "emission_count": emission_count,
        "activities": activities,
        "next_cursor": next_cursor,
    }))
}

pub(super) fn address_transaction_activity(
    transaction: &Transaction,
    address: Address,
    block: &Block,
    tax_rate: u8,
    exchange_rate: u128,
) -> Result<Option<serde_json::Value>, String> {
    if let Some(conversion) = transaction.intent.conversion {
        if transaction.intent.sender != address { return Ok(None); }
        let (output_currency, output_amount) = if conversion.from == novus::transaction::Currency::Gold {
            (novus::transaction::Currency::for_race_reward(transaction.intent.race), novus::transaction::exchange_output_amount(conversion.amount.as_atomic(), true, exchange_rate).unwrap_or_default())
        } else {
            (novus::transaction::Currency::Gold, novus::transaction::exchange_output_amount(conversion.amount.as_atomic(), false, exchange_rate).unwrap_or_default())
        };
        return Ok(Some(serde_json::json!({
            "height": block.height().0,
            "block_hash": hex::encode(block.hash().map_err(|error| error.to_string())?.0),
            "hash": hex::encode(transaction.id().map_err(|error| error.to_string())?),
            "type": "conversion",
            "direction": "convert",
            "amount": conversion.amount.as_atomic(),
            "currency": format!("{:?}", conversion.from).to_lowercase(),
            "output_currency": format!("{:?}", output_currency).to_lowercase(),
            "output_amount": output_amount,
            "exchange_rate_units_per_gold": exchange_rate,
            "protocol_tax": transaction_tax_json(transaction, tax_rate),
            "tax_vault_credit": tax_vault_credit_json(transaction, tax_rate),
            "fee": transaction.intent.fee.as_atomic(),
            "fee_currency": format!("{:?}", novus::transaction::Currency::for_race_reward(transaction.intent.race)).to_lowercase(),
            "size_bytes": canonical_bytes(transaction).map_err(|error| error.to_string())?.len(),
        })));
    }
    let sender = Some(transaction.intent.sender);
    let received = checked_output_sum(
        transaction
            .intent
            .transfers
            .iter()
            .filter(|transfer| transfer.recipient == address)
            .map(|transfer| transfer.amount),
    )?;
    let (direction, amount) = if sender == Some(address) {
        let external = checked_output_sum(
            transaction
                .intent
                .transfers
                .iter()
                .filter(|transfer| transfer.recipient != address)
                .map(|transfer| transfer.amount),
        )?;
        (
            if external.as_atomic() == 0 {
                "self"
            } else {
                "out"
            },
            external,
        )
    } else if received.as_atomic() > 0 {
        ("in", received)
    } else {
        return Ok(None);
    };
    Ok(Some(serde_json::json!({
        "height": block.height().0,
        "block_hash": hex::encode(block.hash().map_err(|error| error.to_string())?.0),
        "hash": hex::encode(transaction.id().map_err(|error| error.to_string())?),
        "type": transaction_kind(transaction),
        "direction": direction,
        "amount": amount.as_atomic(),
        "protocol_tax": transaction_tax_json(transaction, tax_rate),
        "tax_vault_credit": tax_vault_credit_json(transaction, tax_rate),
        "fee": transaction.intent.fee.as_atomic(),
        "fee_currency": format!("{:?}", novus::transaction::Currency::for_race_reward(transaction.intent.race)).to_lowercase(),
        "message": transaction.intent.message.as_deref(),
        "size_bytes": canonical_bytes(transaction).map_err(|error| error.to_string())?.len(),
    })))
}

pub(super) fn explorer_transaction_response(
    database: &Path,
    ledger: &Ledger,
    hash: [u8; 32],
) -> Result<serde_json::Value, String> {
    let tip_height = ledger.tip_height().map_or(0, |height| height.0);

    let location = index::transaction_location(database, ledger, hash)?
        .ok_or("transaction was not found in the canonical chain")?;

    let block = ledger
        .chain
        .block(&location.height)
        .ok_or("indexed transaction block is missing from the canonical chain")?;

    let transaction = block
        .transactions()
        .get(location.transaction_index)
        .ok_or("indexed transaction position is missing from its block")?;

    if transaction.id().map_err(|error| error.to_string())? != hash {
        return Err("transaction index does not match the canonical chain".into());
    }
    let tax_rate = historical_race_tax_rate(ledger, block.height().0, transaction.intent.race);
    let exchange_rate = historical_exchange_rate(ledger, block.height().0, transaction.intent.race);

    Ok(serde_json::json!({
        "hash": hex::encode(hash),
        "type": transaction_kind(transaction),
        "status": "confirmed",
        "height": block.height().0,
        "block_hash": hex::encode(
            block.hash().map_err(|error| error.to_string())?.0
        ),
        "confirmations": tip_height
            .saturating_sub(block.height().0)
            .saturating_add(1),
        "size_bytes": canonical_bytes(transaction)
            .map_err(|error| error.to_string())?
            .len(),
        "transaction": transaction_response(transaction, tax_rate, exchange_rate),
    }))
}

pub(super) fn transaction_response(transaction: &Transaction, tax_rate: u8, exchange_rate: u128) -> serde_json::Value {
    spend_transaction_response(transaction, tax_rate, exchange_rate)
}

pub(super) fn spend_transaction_response(transaction: &Transaction, tax_rate: u8, exchange_rate: u128) -> serde_json::Value {
    let intent = &transaction.intent;
    let conversion = intent.conversion.map(|conversion| {
        let (to, output_amount) = if conversion.from == novus::transaction::Currency::Gold {
            (novus::transaction::Currency::for_race_reward(intent.race), novus::transaction::exchange_output_amount(conversion.amount.as_atomic(), true, exchange_rate).unwrap_or_default())
        } else {
            (novus::transaction::Currency::Gold, novus::transaction::exchange_output_amount(conversion.amount.as_atomic(), false, exchange_rate).unwrap_or_default())
        };
        serde_json::json!({
            "from": format!("{:?}", conversion.from).to_lowercase(),
            "input_amount": conversion.amount.as_atomic(),
            "to": format!("{:?}", to).to_lowercase(),
            "output_amount": output_amount,
            "rate": exchange_rate,
        })
    });
    serde_json::json!({
        "type": transaction_kind(transaction),
        "protocol_tax": transaction_tax_json(transaction, tax_rate),
        "tax_vault_credit": tax_vault_credit_json(transaction, tax_rate),
        "message": intent.message.as_deref(),
        "signer": novus::crypto::address_to_string(&intent.sender, intent.sender.race().prefix()),
        "nonce": intent.nonce,
        "transfers": intent.transfers.iter().map(|transfer| serde_json::json!({
            "address": novus::crypto::address_to_string(&transfer.recipient, transfer.recipient.race().prefix()),
            "currency": format!("{:?}", transfer.currency).to_lowercase(),
            "amount": transfer.amount.as_atomic(),
        })).collect::<Vec<_>>(),
        "conversion": conversion,
        "fee": intent.fee.as_atomic(),
        "fee_currency": format!("{:?}", novus::transaction::Currency::for_race_reward(intent.race)).to_lowercase(),
    })
}

pub(super) fn checked_output_sum(
    amounts: impl IntoIterator<Item = Amount>,
) -> Result<Amount, String> {
    amounts
        .into_iter()
        .try_fold(Amount::from_atomic(0), |total, amount| {
            total
                .checked_add(amount)
                .ok_or_else(|| "explorer amount overflow".to_string())
        })
}

pub(super) fn transaction_kind(transaction: &Transaction) -> &'static str {
    if transaction.intent.conversion.is_some() { "conversion" } else { "transfer" }
}

fn transaction_tax_json(transaction: &Transaction, tax_rate: u8) -> std::collections::BTreeMap<String, u128> {
    novus::consensus::calculate_transfer_taxes_with_rates(&transaction.intent, |_| tax_rate)
        .unwrap_or_default()
        .into_iter()
        .map(|(currency, amount)| (format!("{currency:?}").to_lowercase(), amount.as_atomic()))
        .collect()
}

fn tax_vault_credit_json(transaction: &Transaction, tax_rate: u8) -> std::collections::BTreeMap<String, u128> {
    let mut credits = transaction_tax_json(transaction, tax_rate);
    let fee_currency = novus::transaction::Currency::for_race_reward(transaction.intent.race);
    let key = format!("{fee_currency:?}").to_lowercase();
    let amount = credits.get(&key).copied().unwrap_or_default()
        .saturating_add(transaction.intent.fee.as_atomic());
    if amount > 0 { credits.insert(key, amount); }
    credits
}

fn historical_race_tax_rate(ledger: &Ledger, block_height: u64, race: novus::transaction::Race) -> u8 {
    let mut state = novus::ledger::ChipWarState::default();
    for block in ledger.chain.blocks().filter(|block| block.height().0 < block_height && !block.is_genesis()) {
        let miner_race = novus::transaction::Race::from(block.miner_address().race());
        if state.record_block(block.height().0, miner_race).is_none() { break; }
    }
    state.tax_rate(race)
}

fn historical_exchange_rate(ledger: &Ledger, block_height: u64, race: novus::transaction::Race) -> u128 {
    let mut state = novus::ledger::ChipWarState::default();
    for block in ledger.chain.blocks().filter(|block| block.height().0 < block_height && !block.is_genesis()) {
        let miner_race = novus::transaction::Race::from(block.miner_address().race());
        if state.record_block(block.height().0, miner_race).is_none() { break; }
    }
    state.exchange_rate(race)
}

pub(super) fn status_response(
    ledger: &Ledger,
    cumulative_work: Work,
) -> Result<serde_json::Value, String> {
    let tip_height = ledger.tip_height().ok_or("canonical genesis is missing")?;
    let tip_hash = ledger.tip_hash().ok_or("canonical genesis is missing")?;
    let next_difficulty =
        expected_next_difficulty(&ledger.chain).map_err(|error| error.to_string())?;
    let novus = &ledger.state().novus;
    let total_supply = novus
        .total_mined
        .checked_sub(novus.total_burned)
        .ok_or("total burned exceeds total mined")?;

    Ok(serde_json::json!({
        "tip_height": tip_height.0,
        "next_height": tip_height.0.saturating_add(1),
        "finalized_height": ledger.finalized_height().0,
        "tip_hash": hex::encode(tip_hash.0),
        "next_difficulty": next_difficulty,
        "cumulative_work": format_work(cumulative_work.to_be_limbs()),
        "total_mined": novus.total_mined.as_atomic(),
        "total_burned": novus.total_burned.as_atomic(),
        "total_supply": total_supply.as_atomic(),
        "race_block_rewards": novus.race_rewards.iter().map(|(currency, amount)| (format!("{currency:?}").to_lowercase(), amount.as_atomic())).collect::<std::collections::BTreeMap<_, _>>(),
        "tax_vault": ledger.state().tax_vault.balances().iter().map(|(currency, amount)| (format!("{currency:?}").to_lowercase(), amount.as_atomic())).collect::<std::collections::BTreeMap<_, _>>(),
        "exchange_vault": ledger.state().exchange_vault.balances().iter().map(|(currency, amount)| (format!("{currency:?}").to_lowercase(), amount.as_atomic())).collect::<std::collections::BTreeMap<_, _>>(),
        "chip_war": {
            "epoch_blocks": novus::ledger::CHIP_WAR_EPOCH_BLOCKS,
            "completed_epochs": ledger.state().chip_war.completed_epochs,
            "blocks_in_current_epoch": ledger.state().chip_war.block_counts.iter().map(|(race, count)| (format!("{race:?}").to_lowercase(), count)).collect::<std::collections::BTreeMap<_, _>>(),
            "race_tax_rates_percent": ledger.state().chip_war.tax_rates.iter().map(|(race, rate)| (format!("{race:?}").to_lowercase(), rate)).collect::<std::collections::BTreeMap<_, _>>(),
            "race_exchange_rates_units_per_gold": novus::transaction::Race::all().into_iter().map(|race| (format!("{race:?}").to_lowercase(), ledger.state().chip_war.exchange_rate(race))).collect::<std::collections::BTreeMap<_, _>>(),
            "race_exchange_rate_adjustments_percent": ledger.state().chip_war.exchange_rate_adjustments.iter().map(|(race, adjustment)| (format!("{race:?}").to_lowercase(), adjustment)).collect::<std::collections::BTreeMap<_, _>>(),
            "last_outcomes": ledger.state().chip_war.last_outcomes.iter().map(|(race, outcome)| (format!("{race:?}").to_lowercase(), format!("{outcome:?}").to_lowercase())).collect::<std::collections::BTreeMap<_, _>>(),
        },
    }))
}

pub(super) fn latest_blocks_response(ledger: &Ledger) -> Result<serde_json::Value, String> {
    let blocks = ledger
        .chain
        .blocks()
        .rev()
        .take(20)
        .map(|block| block_response(ledger, block))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(serde_json::json!({ "blocks": blocks }))
}

pub(super) fn block_response(ledger: &Ledger, block: &Block) -> Result<serde_json::Value, String> {
    let block_reward = block.emission().map(|emission| {
        let currency = novus::transaction::Currency::for_race_reward(
            novus::transaction::Race::from(emission.to.race()),
        );
        serde_json::json!({ "currency": format!("{currency:?}").to_lowercase(), "amount": emission.subsidy.as_atomic() })
    });
    let transaction_details = block
        .transactions()
        .iter()
        .map(|transaction| {
            Ok(serde_json::json!({
                "hash": hex::encode(
                    transaction.id().map_err(|error| error.to_string())?
                ),
                "type": transaction_kind(transaction),
                "size_bytes": canonical_bytes(transaction)
                    .map_err(|error| error.to_string())?
                    .len(),
                "transaction": transaction_response(transaction, historical_race_tax_rate(ledger, block.height().0, transaction.intent.race), historical_exchange_rate(ledger, block.height().0, transaction.intent.race)),
            }))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let hash = transaction_details
        .iter()
        .filter_map(|transaction| transaction.get("hash").cloned())
        .collect::<Vec<_>>();
    Ok(serde_json::json!({
        "height": block.height().0,
        "hash": hex::encode(block.hash().map_err(|error| error.to_string())?.0),
        "previous_hash": hex::encode(block.previous_hash().0),
        "difficulty": block.target_bits(),
        "block_size": block.block_size(),
        "nonce": block.header.nonce.0,
        "transactions": block.transaction_count(),
        "hash": hash,
        "transaction_details": transaction_details,
        "miner": novus::crypto::address_to_string(&block.miner_address(), block.miner_address().race().prefix()),
        "block_reward": block_reward,
    }))
}
