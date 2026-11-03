use super::*;
use super::{config::*, explorer::*, mempool::*, mining::*, state::*, util::*};

pub(super) fn serve_rpc(path: Option<&str>, listen: &str) -> Result<(), String> {
    let database = database_path(path);
    serve_rpc_database(database, listen)
}

pub(super) fn serve_rpc_database(database: PathBuf, listen: &str) -> Result<(), String> {
    load_or_initialize(&database)?;
    let listener = TcpListener::bind(listen).map_err(|error| format!("bind RPC: {error}"))?;
    println!("rpc: http://{listen}");

    let active = Arc::new(AtomicUsize::new(0));
    let active_by_ip = Arc::new(Mutex::new(std::collections::BTreeMap::new()));

    for connection in listener.incoming() {
        match connection {
            Ok(mut stream) => {
                if active.fetch_add(1, Ordering::AcqRel) >= MAX_RPC_CONNECTIONS {
                    active.fetch_sub(1, Ordering::AcqRel);
                    let _ = write_http_response(
                        &mut stream,
                        503,
                        &serde_json::json!({"error": "RPC connection limit reached"}),
                    );
                    continue;
                }

                let peer_ip = stream.peer_addr().ok().map(|address| address.ip());
                if let Some(ip) = peer_ip {
                    let mut by_ip = match active_by_ip.lock() {
                        Ok(by_ip) => by_ip,
                        Err(_) => {
                            active.fetch_sub(1, Ordering::AcqRel);
                            let _ = write_http_response(
                                &mut stream,
                                503,
                                &serde_json::json!({"error": "RPC connection limit state unavailable"}),
                            );
                            continue;
                        }
                    };
                    let count = by_ip.entry(ip).or_insert(0_usize);
                    if *count >= MAX_RPC_CONNECTIONS_PER_IP {
                        active.fetch_sub(1, Ordering::AcqRel);
                        let _ = write_http_response(
                            &mut stream,
                            429,
                            &serde_json::json!({"error": "RPC per-IP connection limit reached"}),
                        );
                        continue;
                    }
                    *count += 1;
                }

                let database = database.clone();
                let active = Arc::clone(&active);
                let active_by_ip = Arc::clone(&active_by_ip);
                thread::spawn(move || {
                    if let Err(error) = handle_rpc_connection(&database, &mut stream) {
                        let _ = write_http_response(
                            &mut stream,
                            400,
                            &serde_json::json!({"error": error}),
                        );
                    }

                    active.fetch_sub(1, Ordering::AcqRel);
                    if let Some(ip) = peer_ip
                        && let Ok(mut by_ip) = active_by_ip.lock()
                        && let Some(count) = by_ip.get_mut(&ip)
                    {
                        *count = count.saturating_sub(1);
                        if *count == 0 {
                            by_ip.remove(&ip);
                        }
                    }
                });
            }
            Err(error) => eprintln!("node: accept RPC connection: {error}"),
        }
    }
    Ok(())
}

pub(super) fn handle_rpc_connection(database: &Path, stream: &mut TcpStream) -> Result<(), String> {
    stream
        .set_read_timeout(Some(RPC_TIMEOUT))
        .and_then(|_| stream.set_write_timeout(Some(RPC_TIMEOUT)))
        .map_err(|error| format!("configure RPC timeout: {error}"))?;
    let request = read_http_request(stream)?;
    let request_line = request.headers.lines().next().ok_or("empty RPC request")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or("missing RPC method")?;
    let route = parts.next().ok_or("missing RPC route")?;
    if method == "POST" && route == "/transaction" {
        let transaction: Transaction = canonical_decode(&request.body)
            .map_err(|error| format!("invalid submitted transaction: {error}"))?;
        let hash = insert_mempool_transaction(database, transaction, false)?;
        return write_http_response(stream, 200, &serde_json::json!({"hash": hex::encode(hash)}));
    }
    if method != "GET" {
        return Err("unsupported RPC method".into());
    }
    if !request.body.is_empty() {
        return Err("GET request body is not allowed".into());
    }
    match route {
        "/openapi.json" => {
            return write_http_bytes(stream, 200, "application/json", OPENAPI_JSON);
        }
        "/docs" | "/docs/" => {
            return write_http_bytes(stream, 200, "text/html; charset=utf-8", API_DOCS_HTML);
        }
        _ => {}
    }

    let (ledger, _header_checkpoints, cumulative_work) =
        load_or_initialize_header_snapshot(database)?;

    let response = match route {
        "/status" => status_response(&ledger, cumulative_work)?,
        "/fee-policy" => {
            let emission = expected_next_emission(&ledger)?;
            serde_json::json!({
                "tax_currency_policy": {
                    "race_currency": "same_as_transferred_or_converted_currency",
                    "gold": "gold",
                    "gold_to_novus": "gold",
                    "novus": "none"
                },
                "novus_transfer_tax_percent": 0,
                "transaction_tax_rates_percent": {
                    "base_race_currency_transfer": novus::consensus::TRANSFER_TAX_PERCENT,
                    "base_race_currency_to_gold_conversion": novus::consensus::RACE_TO_GOLD_CONVERSION_TAX_PERCENT,
                    "base_gold_transfer": novus::consensus::GOLD_TRANSFER_TAX_PERCENT,
                    "base_gold_to_race_currency_conversion": novus::consensus::GOLD_TO_RACE_CONVERSION_TAX_PERCENT,
                    "base_gold_to_novus_conversion": novus::consensus::GOLD_TO_NOVUS_CONVERSION_TAX_PERCENT,
                    "novus_transfer": 0
                },
                "race_tax_modifier_percent_points": ledger.state().chip_war.tax_rates.iter().map(|(race, rate)| (format!("{race:?}").to_lowercase(), rate)).collect::<std::collections::BTreeMap<_, _>>(),
                "race_tax_modifier_applies_to": ["race_currency_transfer", "race_currency_to_gold_conversion", "gold_transfer", "gold_to_race_currency_conversion", "gold_to_novus_conversion"],
                "race_tax_adjustment_percent_points": {
                    "loss_increase": novus::ledger::RACE_TAX_LOSS_INCREASE_PERCENT,
                    "failure_increase": novus::ledger::RACE_TAX_FAILURE_INCREASE_PERCENT,
                    "win_decrease": novus::ledger::RACE_TAX_WIN_DECREASE_PERCENT,
                    "minimum": novus::ledger::MIN_RACE_TAX_MODIFIER_PERCENT,
                    "maximum": novus::ledger::MAX_RACE_TAX_MODIFIER_PERCENT,
                    "effective_tax_cap": novus::ledger::MAX_EFFECTIVE_TAX_PERCENT
                },
                "relay_fee_policy": {
                    "novus_transfer": "size_based_fee_paid_in_novus_to_including_block_miner",
                    "other_transactions": "none"
                },
                "gold_transfer_policy": "sender_race_only",
                "novus_transfer_policy": "global_any_race",
                "mixed_currency_transfers_allowed": false,
                "next_block_emission": emission.as_atomic(),
                "difficulty": {
                    "target_block_time_seconds": novus::consensus::TARGET_BLOCK_TIME_SECONDS,
                    "algorithm": novus::consensus::DIFFICULTY_ALGORITHM,
                    "window_blocks": novus::consensus::DIFFICULTY_ADJUSTMENT_WINDOW,
                    "adjusts_every_block": true,
                    "target_timespan_seconds": novus::consensus::DIFFICULTY_TARGET_TIMESPAN_SECONDS,
                    "maximum_difficulty_multiplier_from_start": novus::consensus::MAX_DIFFICULTY_MULTIPLIER,
                    "minimum_target_bits": novus::consensus::minimum_difficulty_target_bits(),
                    "maximum_target_bits": novus::consensus::TARGET_BITS_START,
                    "maximum_solve_time_per_sample_multiplier": novus::consensus::DIFFICULTY_TIMESPAN_CLAMP_FACTOR,
                },
                "stake_pow": {
                    "minimum_stake_atomic": novus::consensus::MIN_WAR_STAKE_ATOMIC,
                    "maximum_stake_atomic": serde_json::Value::Null,
                    "reference_stake_atomic": novus::consensus::STAKE_REFERENCE_ATOMIC,
                    "max_boost_ppm": novus::consensus::STAKE_MAX_BOOST_PPM,
                    "max_target_multiplier": 2,
                    "activation_epochs": 1,
                    "unbonding_epochs": 1,
                    "epoch_seconds": novus::ledger::CHIP_WAR_EPOCH_SECONDS,
                    "target_snapshot": "parent_state_total_active_race_pool_stake",
                    "cumulative_work": "effective_target",
                    "stake_reward": "none",
                },
                "war_contribution": {
                    "points_per_participating_epoch": novus::ledger::WAR_CONTRIBUTION_POINTS_PER_EPOCH,
                    "points_are_cumulative": true,
                    "maximum_points": serde_json::Value::Null,
                    "points_are_transferable": false,
                    "rank_tiers": 8,
                    "rank_settlement": "at_chip_war_epoch_boundary",
                    "node_participation": "one_signed_wallet_heartbeat_per_epoch_no_block_found_required",
                },
                "race_exchange_rate_units_per_gold": novus::transaction::Race::all().into_iter().map(|race| (format!("{race:?}").to_lowercase(), ledger.state().chip_war.exchange_rate(race))).collect::<std::collections::BTreeMap<_, _>>(),
                "race_currency_to_gold": {
                    "base_rate": novus::transaction::RACE_CURRENCY_PER_GOLD,
                    "race_rates_units_per_gold": novus::transaction::Race::all().into_iter().map(|race| (format!("{race:?}").to_lowercase(), ledger.state().chip_war.exchange_rate(race))).collect::<std::collections::BTreeMap<_, _>>(),
                    "currency_units_per_gold": {
                        "accretia": "cp",
                        "bellato": "dalant",
                        "cora": "disena"
                    },
                    "directions": ["race_currency_to_gold", "gold_to_wallet_race_currency"],
                    "uses_exchange_vault": true
                },
                "global_gold_to_novus": {
                    "gold_per_quote": 1,
                    "gold_per_novus": novus::transaction::GOLD_PER_NOVUS,
                    "gold_supply_cap_atomic": novus::ledger::GOLD_SUPPLY_CAP.as_atomic(),
                    "gold_converted_this_cycle_atomic": ledger.state().global_currency.gold_converted_this_cycle.as_atomic(),
                    "total_gold_burned_atomic": ledger.state().global_currency.total_gold_burned.as_atomic(),
                    "total_novus_minted_atomic": ledger.state().global_currency.total_novus_minted.as_atomic(),
                    "refill_after_cumulative_gold_conversion_atomic": novus::ledger::GOLD_SUPPLY_CAP.as_atomic(),
                    "direction": "gold_to_novus",
                    "reverse_supported": false,
                    "burns_input_gold": true,
                    "refills_global_gold_exchange_vault": true,
                    "uses_exchange_vault": true
                },
                "block_reward_currency_by_race": {
                    "accretia": "cp",
                    "bellato": "dalant",
                    "cora": "disena"
                },
            })
        }
        "/blocks/latest" => latest_blocks_response(&ledger)?,
        "/tax-vault" => tax_vault_response(&ledger, None)?,
        route if route.starts_with("/tax-vault/") => {
            let race = parse_race_slug(route.trim_start_matches("/tax-vault/"))?;
            tax_vault_response(&ledger, Some(race))?
        }
        "/war" => war_response(&ledger, None)?,
        route if route.starts_with("/war/") => {
            let race = parse_race_slug(route.trim_start_matches("/war/"))?;
            war_response(&ledger, Some(race))?
        }
        route if route.starts_with("/balance/") => {
            let address = route.trim_start_matches("/balance/");
            if address.is_empty() || address.contains(['/', '?', '#']) {
                return Err("invalid balance route".into());
            }
            balance_response(&ledger, &read_mempool(database)?, parse_address(address)?)?
        }
        route if route.starts_with("/race/") => {
            let address = parse_address(route.trim_start_matches("/race/"))?;
            let race = ledger
                .state()
                .accounts
                .get(address)
                .race
                .map(|race| match race {
                    novus::transaction::Race::Accretia => "accretia",
                    novus::transaction::Race::Bellato => "bellato",
                    novus::transaction::Race::Cora => "cora",
                });
            serde_json::json!({"race": race})
        }
        route if route.starts_with("/stake/") => {
            let address = parse_address(route.trim_start_matches("/stake/"))?;
            let race = novus::transaction::Race::from(address.race());
            let stake = &ledger.state().staking;
            let contribution_points = stake.contribution_points(race, address);
            let last_rank = stake.latest_war_rank(race, address);
            serde_json::json!({
                "address": novus::crypto::address_to_string(&address, address.race().prefix()),
                "race": format!("{race:?}").to_lowercase(),
                "epoch": stake.current_epoch(),
                "locked_atomic": stake.locked_by(race, address).as_atomic(),
                "withdrawable_atomic": stake.withdrawable_by(race, address).as_atomic(),
                "active_stake_atomic": stake.active_by_delegator(race, address).as_atomic(),
                "active_participant": !stake.active_by_delegator(race, address).is_zero(),
                "minimum_stake_atomic": novus::consensus::MIN_WAR_STAKE_ATOMIC,
                "race_active_participant_count": stake.active_participants_for_race(race),
                "race_pool_active_stake_atomic": stake.active_for_race(race).unwrap_or_default().as_atomic(),
                "contribution": {
                    "epoch": stake.current_epoch(),
                    "participated_this_epoch": stake.participated_this_epoch(race, address),
                    "heartbeat_submitted_this_epoch": stake.has_war_heartbeat(race, address),
                    "total_points": contribution_points,
                    "transferable": false,
                },
                "latest_rank": last_rank.map(|record| serde_json::json!({
                    "last_contribution_epoch": record.epoch,
                    "total_contribution_points": record.contribution_points,
                    "rank_index": record.rank_index,
                    "label": novus::ledger::StakeState::rank_label(race, record.rank_index),
                })),
            })
        }
        route if route.starts_with("/block/") => {
            let height = route
                .trim_start_matches("/block/")
                .parse::<u64>()
                .map_err(|_| "invalid block height")?;
            block_response(
                &ledger,
                ledger
                    .chain
                    .block(&Height(height))
                    .ok_or("block was not found")?,
            )?
        }
        route if route.starts_with("/account/") => {
            let account = route.trim_start_matches("/account/");
            if account.is_empty() || account.contains(['/', '?', '#']) {
                return Err("invalid account route".into());
            }
            account_response(&ledger, parse_address(account)?)?
        }
        route if route.starts_with("/explorer/address/") => {
            let value = route.trim_start_matches("/explorer/address/");
            let (address, query) = value.split_once('?').unwrap_or((value, ""));

            if address.is_empty() || address.contains(['/', '#']) {
                return Err("invalid explorer address route".into());
            }

            let mut include_emissions = true;
            let mut limit = DEFAULT_ADDRESS_ACTIVITY_LIMIT;
            let mut before = None;

            let mut seen_include_emissions = false;
            let mut seen_limit = false;
            let mut seen_before = false;

            if !query.is_empty() {
                for parameter in query.split('&') {
                    let (name, value) = parameter
                        .split_once('=')
                        .ok_or("invalid explorer address query")?;

                    match name {
                        "include_emissions" => {
                            if seen_include_emissions {
                                return Err("duplicate include_emissions query".into());
                            }

                            seen_include_emissions = true;

                            include_emissions = match value {
                                "true" => true,
                                "false" => false,
                                _ => return Err("invalid include_emissions query".into()),
                            };
                        }

                        "limit" => {
                            if seen_limit {
                                return Err("duplicate limit query".into());
                            }

                            seen_limit = true;

                            limit = value
                                .parse::<usize>()
                                .map_err(|_| "invalid explorer address limit")?;

                            if limit == 0 || limit > MAX_ADDRESS_ACTIVITY_LIMIT {
                                return Err("explorer address limit is out of range".into());
                            }
                        }

                        "before" => {
                            if seen_before {
                                return Err("duplicate before query".into());
                            }

                            seen_before = true;

                            if value.len() != crate::storage::ADDRESS_ACTIVITY_CURSOR_SIZE * 2 {
                                return Err("invalid explorer address cursor".into());
                            }

                            let bytes = hex::decode(value)
                                .map_err(|_| "invalid explorer address cursor")?;

                            let cursor: [u8; crate::storage::ADDRESS_ACTIVITY_CURSOR_SIZE] = bytes
                                .try_into()
                                .map_err(|_| "invalid explorer address cursor")?;

                            before = Some(cursor);
                        }

                        _ => return Err("invalid explorer address query".into()),
                    }
                }
            }

            explorer_address_response(
                database,
                &ledger,
                &read_mempool(database)?,
                parse_address(address)?,
                include_emissions,
                limit,
                before,
            )?
        }
        route if route.starts_with("/explorer/transaction/") => {
            let hash = route.trim_start_matches("/explorer/transaction/");
            explorer_transaction_response(database, &ledger, parse_hash(hash)?)?
        }
        _ => return Err("unknown RPC route".into()),
    };
    write_http_response(stream, 200, &response)
}

fn parse_race_slug(value: &str) -> Result<novus::transaction::Race, String> {
    match value {
        "accretia" => Ok(novus::transaction::Race::Accretia),
        "bellato" => Ok(novus::transaction::Race::Bellato),
        "cora" => Ok(novus::transaction::Race::Cora),
        _ => Err("race must be accretia, bellato, or cora".into()),
    }
}

fn race_slug(race: novus::transaction::Race) -> &'static str {
    match race {
        novus::transaction::Race::Accretia => "accretia",
        novus::transaction::Race::Bellato => "bellato",
        novus::transaction::Race::Cora => "cora",
    }
}

fn tax_vault_response(
    ledger: &Ledger,
    requested_race: Option<novus::transaction::Race>,
) -> Result<serde_json::Value, String> {
    use novus::transaction::{Amount, Currency, Race};
    let races = requested_race.map_or_else(|| Race::all().to_vec(), |race| vec![race]);
    let mut balances = std::collections::BTreeMap::new();
    let mut totals = Currency::all()
        .into_iter()
        .map(|currency| (currency, Amount::ZERO))
        .collect::<std::collections::BTreeMap<_, _>>();

    for race in races {
        let mut by_currency = std::collections::BTreeMap::new();
        for currency in Currency::all() {
            let amount = ledger.state().tax_vault.balance(race, currency);
            if !amount.is_zero() {
                by_currency.insert(format!("{currency:?}").to_lowercase(), amount.as_atomic());
                let total = totals
                    .get(&currency)
                    .copied()
                    .unwrap_or(Amount::ZERO)
                    .checked_add(amount)
                    .ok_or("tax vault total overflow")?;
                totals.insert(currency, total);
            }
        }
        balances.insert(race_slug(race), serde_json::json!(by_currency));
    }

    let total_by_currency = totals
        .into_iter()
        .filter(|(_, amount)| !amount.is_zero())
        .map(|(currency, amount)| (format!("{currency:?}").to_lowercase(), amount.as_atomic()))
        .collect::<std::collections::BTreeMap<_, _>>();
    Ok(serde_json::json!({
        "tip_height": ledger.tip_height().map_or(0, |height| height.0),
        "scope": requested_race.map(race_slug).unwrap_or("all"),
        "balances_by_race": balances,
        "total_by_currency_atomic": total_by_currency,
        "claim_policy": "only_miners_from_the_origin_race_can_claim_that_race_vault",
    }))
}

fn war_response(
    ledger: &Ledger,
    requested_race: Option<novus::transaction::Race>,
) -> Result<serde_json::Value, String> {
    use novus::transaction::Race;
    let races = requested_race.map_or_else(|| Race::all().to_vec(), |race| vec![race]);
    let war = &ledger.state().chip_war;
    let staking = &ledger.state().staking;
    let races = races
        .into_iter()
        .map(|race| {
            let rank_labels = (0..8)
                .map(|index| novus::ledger::StakeState::rank_label(race, index).unwrap_or(""))
                .collect::<Vec<_>>();
            serde_json::json!({
                "race": race_slug(race),
                "active_staker_count": staking.active_participants_for_race(race),
                "current_epoch_participant_count": staking.epoch_participants_for_race(race),
                "active_stake_atomic": staking.active_for_race(race).unwrap_or_default().as_atomic(),
                "blocks_in_epoch": war.block_counts.get(&race).copied().unwrap_or(0),
                "tax_modifier_percent_points": war.tax_rate(race),
                "exchange_rate_units_per_gold": war.exchange_rate(race),
                "last_outcome": war.last_outcomes.get(&race).map(|outcome| format!("{outcome:?}").to_lowercase()),
                "rank_labels": rank_labels,
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::json!({
        "tip_height": ledger.tip_height().map_or(0, |height| height.0),
        "epoch": war.completed_epochs,
        "epoch_seconds": novus::ledger::CHIP_WAR_EPOCH_SECONDS,
        "epoch_blocks": novus::ledger::CHIP_WAR_EPOCH_BLOCKS,
        "points_per_participating_epoch": novus::ledger::WAR_CONTRIBUTION_POINTS_PER_EPOCH,
        "contribution_points_are_cumulative": true,
        "races": races,
    }))
}

pub(super) fn read_http_request(stream: &mut impl Read) -> Result<HttpRequest, String> {
    let mut bytes = Vec::new();
    let header_end = loop {
        if bytes.len() >= MAX_RPC_HEADER_SIZE {
            return Err("RPC headers exceed size limit".into());
        }
        let mut chunk = [0_u8; 1024];
        let length = stream
            .read(&mut chunk)
            .map_err(|error| format!("read RPC: {error}"))?;
        if length == 0 {
            return Err("RPC connection closed before headers completed".into());
        }
        bytes.extend_from_slice(&chunk[..length]);
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let headers = std::str::from_utf8(&bytes[..header_end])
        .map_err(|_| "RPC headers are not UTF-8")?
        .to_string();
    let mut content_length = None;
    for line in headers.lines().skip(1) {
        if line.trim().is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err("malformed RPC header".into());
        };
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err("RPC Transfer-Encoding is not supported".into());
        }
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err("duplicate RPC Content-Length".into());
            }
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| "invalid RPC Content-Length")?,
            );
        }
    }
    let content_length = content_length.unwrap_or(0);
    if content_length > MAX_STORED_TRANSACTION_SIZE {
        return Err("RPC request body exceeds transaction size limit".into());
    }
    let total = header_end
        .checked_add(content_length)
        .ok_or("RPC request size overflow")?;
    if bytes.len() > total {
        bytes.truncate(total);
    }
    while bytes.len() < total {
        let remaining = total - bytes.len();
        let mut chunk = vec![0_u8; remaining.min(8 * 1024)];
        let length = stream
            .read(&mut chunk)
            .map_err(|error| format!("read RPC body: {error}"))?;
        if length == 0 {
            return Err("RPC body is truncated".into());
        }
        bytes.extend_from_slice(&chunk[..length]);
    }
    Ok(HttpRequest {
        headers,
        body: bytes[header_end..].to_vec(),
    })
}

pub(super) fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    value: &serde_json::Value,
) -> Result<(), String> {
    let body = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    write_http_bytes(stream, status, "application/json", &body)
}

pub(super) fn write_http_bytes(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> Result<(), String> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        429 => "Too Many Requests",
        503 => "Service Unavailable",
        _ => "Error",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n\r\n",
        body.len()
    )
    .and_then(|_| stream.write_all(body))
    .map_err(|error| format!("write RPC: {error}"))
}
