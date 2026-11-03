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
        "stake": stake_summary(ledger, address),
    }))
}

fn stake_summary(ledger: &Ledger, address: Address) -> serde_json::Value {
    let race = novus::transaction::Race::from(address.race());
    let active_stake = ledger.state().staking.active_by_delegator(race, address);
    let race_pool_stake = ledger
        .state()
        .staking
        .active_for_race(race)
        .unwrap_or_default();
    serde_json::json!({
        "epoch": ledger.state().staking.current_epoch(),
        "locked_atomic": ledger.state().staking.locked_by(race, address).as_atomic(),
        "withdrawable_atomic": ledger.state().staking.withdrawable_by(race, address).as_atomic(),
        "active_stake_atomic": active_stake.as_atomic(),
        "race_pool_active_stake_atomic": race_pool_stake.as_atomic(),
    })
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

                let tax_rate =
                    historical_race_tax_rate(ledger, block.height().0, transaction.intent.race);
                let exchange_rate =
                    historical_exchange_rate(ledger, block.height().0, transaction.intent.race);
                if let Some(activity) = address_transaction_activity(
                    transaction,
                    address,
                    block,
                    tax_rate,
                    exchange_rate,
                )? {
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
    _tax_rate: u8,
    exchange_rate: u128,
) -> Result<Option<serde_json::Value>, String> {
    if let Some(action) = transaction.intent.stake_action {
        if transaction.intent.sender != address {
            return Ok(None);
        }
        let (kind, direction, amount) = match action {
            novus::transaction::StakeAction::Delegate { amount } => {
                ("stake_delegate", "delegate", amount)
            }
            novus::transaction::StakeAction::Undelegate { amount } => {
                ("stake_undelegate", "undelegate", amount)
            }
            novus::transaction::StakeAction::Withdraw { amount } => {
                ("stake_withdraw", "withdraw", amount)
            }
        };
        return Ok(Some(serde_json::json!({
            "height": block.height().0,
            "block_hash": hex::encode(block.hash().map_err(|error| error.to_string())?.0),
            "hash": hex::encode(transaction.id().map_err(|error| error.to_string())?),
            "type": kind,
            "direction": direction,
            "amount": amount.as_atomic(),
            "currency": format!("{:?}", novus::transaction::Currency::for_race_reward(transaction.intent.race)).to_lowercase(),
            "size_bytes": canonical_bytes(transaction).map_err(|error| error.to_string())?.len(),
        })));
    }
    if let Some(conversion) = transaction.intent.conversion {
        if transaction.intent.sender != address {
            return Ok(None);
        }
        let conversion_rate = if (conversion.from, conversion.to)
            == (
                novus::transaction::Currency::Gold,
                novus::transaction::Currency::Novus,
            ) {
            novus::transaction::GOLD_PER_NOVUS
        } else {
            exchange_rate
        };
        let output_amount = novus::transaction::conversion_output_amount(
            conversion.amount.as_atomic(),
            conversion.from,
            conversion.to,
            conversion_rate,
        )
        .unwrap_or_default();
        return Ok(Some(serde_json::json!({
            "height": block.height().0,
            "block_hash": hex::encode(block.hash().map_err(|error| error.to_string())?.0),
            "hash": hex::encode(transaction.id().map_err(|error| error.to_string())?),
            "type": "conversion",
            "direction": "convert",
            "amount": conversion.amount.as_atomic(),
            "currency": format!("{:?}", conversion.from).to_lowercase(),
            "output_currency": format!("{:?}", conversion.to).to_lowercase(),
            "output_amount": output_amount,
            "exchange_rate_units_per_gold": conversion_rate,
            "protocol_tax": transaction_tax_json(transaction),
            "tax_vault_credit": tax_vault_credit_json(transaction),
            "tax": transaction.intent.tax.as_atomic(),
            "tax_currency": format!("{:?}", conversion.from).to_lowercase(),
            "fee": transaction.intent.fee.as_atomic(),
            "fee_currency": (!transaction.intent.fee.is_zero()).then_some("novus"),
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
        "protocol_tax": transaction_tax_json(transaction),
        "tax_vault_credit": tax_vault_credit_json(transaction),
        "tax": transaction.intent.tax.as_atomic(),
        "tax_currency": transaction.intent.transfers.first().map(|transfer| format!("{:?}", transfer.currency).to_lowercase()),
        "fee": transaction.intent.fee.as_atomic(),
        "fee_currency": (!transaction.intent.fee.is_zero()).then_some("novus"),
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

pub(super) fn transaction_response(
    transaction: &Transaction,
    tax_rate: u8,
    exchange_rate: u128,
) -> serde_json::Value {
    spend_transaction_response(transaction, tax_rate, exchange_rate)
}

pub(super) fn spend_transaction_response(
    transaction: &Transaction,
    _tax_rate: u8,
    exchange_rate: u128,
) -> serde_json::Value {
    let intent = &transaction.intent;
    let conversion = intent.conversion.map(|conversion| {
        let rate = if (conversion.from, conversion.to)
            == (
                novus::transaction::Currency::Gold,
                novus::transaction::Currency::Novus,
            ) {
            novus::transaction::GOLD_PER_NOVUS
        } else {
            exchange_rate
        };
        let output_amount = novus::transaction::conversion_output_amount(
            conversion.amount.as_atomic(),
            conversion.from,
            conversion.to,
            rate,
        )
        .unwrap_or_default();
        serde_json::json!({
            "from": format!("{:?}", conversion.from).to_lowercase(),
            "input_amount": conversion.amount.as_atomic(),
            "to": format!("{:?}", conversion.to).to_lowercase(),
            "output_amount": output_amount,
            "rate": rate,
        })
    });
    let stake_action = intent.stake_action.map(|action| match action {
        novus::transaction::StakeAction::Delegate { amount } => serde_json::json!({
            "action": "delegate",
            "amount": amount.as_atomic(),
        }),
        novus::transaction::StakeAction::Undelegate { amount } => serde_json::json!({
            "action": "undelegate",
            "amount": amount.as_atomic(),
        }),
        novus::transaction::StakeAction::Withdraw { amount } => serde_json::json!({
            "action": "withdraw",
            "amount": amount.as_atomic(),
        }),
    });
    serde_json::json!({
        "type": transaction_kind(transaction),
        "protocol_tax": transaction_tax_json(transaction),
        "tax_vault_credit": tax_vault_credit_json(transaction),
        "tax": intent.tax.as_atomic(),
        "tax_currency": intent.conversion.map(|conversion| conversion.from).or_else(|| intent.transfers.first().map(|transfer| transfer.currency)).map(|currency| format!("{currency:?}").to_lowercase()),
        "message": intent.message.as_deref(),
        "signer": novus::crypto::address_to_string(&intent.sender, intent.sender.race().prefix()),
        "nonce": intent.nonce,
        "transfers": intent.transfers.iter().map(|transfer| serde_json::json!({
            "address": novus::crypto::address_to_string(&transfer.recipient, transfer.recipient.race().prefix()),
            "currency": format!("{:?}", transfer.currency).to_lowercase(),
            "amount": transfer.amount.as_atomic(),
        })).collect::<Vec<_>>(),
        "conversion": conversion,
        "stake_action": stake_action,
        "fee": intent.fee.as_atomic(),
        "fee_currency": (!intent.fee.is_zero()).then_some("novus"),
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
    if transaction.intent.stake_action.is_some() {
        match transaction.intent.stake_action.expect("checked as some") {
            novus::transaction::StakeAction::Delegate { .. } => "stake_delegate",
            novus::transaction::StakeAction::Undelegate { .. } => "stake_undelegate",
            novus::transaction::StakeAction::Withdraw { .. } => "stake_withdraw",
        }
    } else if transaction.intent.conversion.is_some() {
        "conversion"
    } else {
        "transfer"
    }
}

fn transaction_tax_json(transaction: &Transaction) -> std::collections::BTreeMap<String, u128> {
    if transaction.intent.tax.is_zero() {
        return std::collections::BTreeMap::new();
    }
    let currency = transaction
        .intent
        .conversion
        .map(|conversion| conversion.from)
        .or_else(|| {
            transaction
                .intent
                .transfers
                .first()
                .map(|transfer| transfer.currency)
        });
    currency
        .map(|currency| {
            std::collections::BTreeMap::from([(
                format!("{currency:?}").to_lowercase(),
                transaction.intent.tax.as_atomic(),
            )])
        })
        .unwrap_or_default()
}

fn tax_vault_credit_json(transaction: &Transaction) -> std::collections::BTreeMap<String, u128> {
    let credits = transaction_tax_json(transaction);
    credits
}

fn historical_race_tax_rate(
    ledger: &Ledger,
    block_height: u64,
    race: novus::transaction::Race,
) -> u8 {
    let mut state = novus::ledger::ChipWarState::default();
    for block in ledger
        .chain
        .blocks()
        .filter(|block| block.height().0 < block_height && !block.is_genesis())
    {
        let miner_race = novus::transaction::Race::from(block.miner_address().race());
        if state.record_block(block.height().0, miner_race).is_none() {
            break;
        }
    }
    state.tax_rate(race)
}

fn historical_exchange_rate(
    ledger: &Ledger,
    block_height: u64,
    race: novus::transaction::Race,
) -> u128 {
    let mut state = novus::ledger::ChipWarState::default();
    for block in ledger
        .chain
        .blocks()
        .filter(|block| block.height().0 < block_height && !block.is_genesis())
    {
        let miner_race = novus::transaction::Race::from(block.miner_address().race());
        if state.record_block(block.height().0, miner_race).is_none() {
            break;
        }
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
    let currency_supply = currency_supply(ledger)?;

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
        "currency_supply": currency_supply.iter().map(|(currency, amount)| (format!("{currency:?}").to_lowercase(), amount.as_atomic())).collect::<std::collections::BTreeMap<_, _>>(),
        "gold_conversion_cycle": {
            "cap_atomic": novus::ledger::GOLD_SUPPLY_CAP.as_atomic(),
            "converted_atomic": ledger.state().global_currency.gold_converted_this_cycle.as_atomic(),
            "total_gold_burned_atomic": ledger.state().global_currency.total_gold_burned.as_atomic(),
            "total_novus_minted_atomic": ledger.state().global_currency.total_novus_minted.as_atomic()
        },
        "race_block_rewards": novus.race_rewards.iter().map(|(currency, amount)| (format!("{currency:?}").to_lowercase(), amount.as_atomic())).collect::<std::collections::BTreeMap<_, _>>(),
        "race_tax_modifier_percent_points": ledger.state().chip_war.tax_rates.iter().map(|(race, rate)| (format!("{race:?}").to_lowercase(), rate)).collect::<std::collections::BTreeMap<_, _>>(),
        "tax_vault": ledger.state().tax_vault.balances().iter().map(|((race, currency), amount)| (format!("{race:?}_{currency:?}").to_lowercase(), amount.as_atomic())).collect::<std::collections::BTreeMap<_, _>>(),
        "exchange_vault": ledger.state().exchange_vault.balances().iter().map(|(currency, amount)| (format!("{currency:?}").to_lowercase(), amount.as_atomic())).collect::<std::collections::BTreeMap<_, _>>(),
        "chip_war": {
            "epoch_seconds": novus::ledger::CHIP_WAR_EPOCH_SECONDS,
            "epoch_blocks": novus::ledger::CHIP_WAR_EPOCH_BLOCKS,
            "completed_epochs": ledger.state().chip_war.completed_epochs,
            "blocks_in_current_epoch": ledger.state().chip_war.block_counts.iter().map(|(race, count)| (format!("{race:?}").to_lowercase(), count)).collect::<std::collections::BTreeMap<_, _>>(),
            "race_exchange_rates_units_per_gold": novus::transaction::Race::all().into_iter().map(|race| (format!("{race:?}").to_lowercase(), ledger.state().chip_war.exchange_rate(race))).collect::<std::collections::BTreeMap<_, _>>(),
            "race_exchange_rate_adjustments_percent": ledger.state().chip_war.exchange_rate_adjustments.iter().map(|(race, adjustment)| (format!("{race:?}").to_lowercase(), adjustment)).collect::<std::collections::BTreeMap<_, _>>(),
            "last_outcomes": ledger.state().chip_war.last_outcomes.iter().map(|(race, outcome)| (format!("{race:?}").to_lowercase(), format!("{outcome:?}").to_lowercase())).collect::<std::collections::BTreeMap<_, _>>(),
        },
    }))
}

fn currency_supply(
    ledger: &Ledger,
) -> Result<std::collections::BTreeMap<novus::transaction::Currency, Amount>, String> {
    let mut supply = novus::transaction::Currency::all()
        .into_iter()
        .map(|currency| (currency, Amount::ZERO))
        .collect::<std::collections::BTreeMap<_, _>>();
    for (_, account) in ledger.state().accounts.iter() {
        for (&currency, &amount) in &account.balances {
            let total = supply
                .get(&currency)
                .copied()
                .unwrap_or(Amount::ZERO)
                .checked_add(amount)
                .ok_or("currency supply overflow")?;
            supply.insert(currency, total);
        }
    }
    for (&(_, currency), &amount) in ledger.state().tax_vault.balances() {
        let total = supply
            .get(&currency)
            .copied()
            .unwrap_or(Amount::ZERO)
            .checked_add(amount)
            .ok_or("currency supply overflow")?;
        supply.insert(currency, total);
    }
    for (&currency, &amount) in ledger.state().exchange_vault.balances() {
        let total = supply
            .get(&currency)
            .copied()
            .unwrap_or(Amount::ZERO)
            .checked_add(amount)
            .ok_or("currency supply overflow")?;
        supply.insert(currency, total);
    }
    Ok(supply)
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
