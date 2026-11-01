use super::*;

pub(super) fn print_history(args: &[String]) -> Result<(), String> {
    let path = option(args, "--wallet").unwrap_or(DEFAULT_WALLET_PATH);
    let rpc = option(args, "--rpc").unwrap_or(DEFAULT_RPC_ADDR);
    let limit = option(args, "--limit")
        .map(parse_history_limit)
        .transpose()?
        .unwrap_or(DEFAULT_HISTORY_LIMIT);
    let before = option(args, "--before")
        .map(validate_history_cursor)
        .transpose()?;
    let bytes =
        Zeroizing::new(fs::read(path).map_err(|error| format!("failed to read {path}: {error}"))?);
    let raw_address = wallet_address_from_file_bytes(&bytes)?;
    let address = novus::crypto::address_to_string(&raw_address, raw_address.race().prefix());
    let history = fetch_address_history(rpc, &address, limit, before)?;
    let next_cursor = print_history_page(history);

    if let Some(cursor) = next_cursor {
        println!("Next Cursor: {cursor}");
    }
    Ok(())
}

pub(super) fn parse_history_limit(value: &str) -> Result<usize, String> {
    let limit = value
        .parse::<usize>()
        .map_err(|_| format!("invalid --limit `{value}`; use 1..={MAX_HISTORY_LIMIT}"))?;
    if !(1..=MAX_HISTORY_LIMIT).contains(&limit) {
        return Err(format!(
            "invalid --limit `{value}`; use 1..={MAX_HISTORY_LIMIT}"
        ));
    }
    Ok(limit)
}

pub(super) fn validate_history_cursor(value: &str) -> Result<&str, String> {
    if value.len() != HISTORY_CURSOR_HEX_LEN || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(format!(
            "invalid --before cursor; expected {HISTORY_CURSOR_HEX_LEN} hexadecimal characters"
        ));
    }
    Ok(value)
}

pub(super) fn fetch_address_history(
    rpc: &str,
    address: &str,
    limit: usize,
    before: Option<&str>,
) -> Result<AddressHistoryResponse, String> {
    let mut path = format!("/explorer/address/{address}?include_emissions=false&limit={limit}");
    if let Some(cursor) = before {
        path.push_str("&before=");
        path.push_str(cursor);
    }
    http_get_json(rpc, &path)
}

pub(super) fn print_history_page(mut history: AddressHistoryResponse) -> Option<String> {
    println!("Address: {}", history.address);
    println!("Tip Height: {}", history.tip_height);

    let emission_count = history.emission_count;
    let next_cursor = history.next_cursor.take();
    history
        .activities
        .retain(|activity| activity.hash.is_some());

    println!("Transactions: {}", history.activities.len());
    println!("Emissions Hidden: {emission_count}");
    println!("More: {}", if next_cursor.is_some() { "yes" } else { "no" });

    if history.activities.is_empty() {
        println!("History: no canonical transactions on this page");
    } else {
        for (index, activity) in history.activities.into_iter().enumerate() {
            let confirmations = history
                .tip_height
                .saturating_sub(activity.height)
                .saturating_add(1);

            println!();
            println!("Transaction {}:", index + 1);
            println!("  Tx Hash: {}", activity.hash.as_deref().unwrap_or("-"));
            println!("  Block Hash: {}", activity.block_hash);
            println!("  Height: {}", activity.height);
            println!("  Confirmations: {confirmations}");
            println!("  Direction: {}", activity.direction);
            println!("  Type: {}", activity.activity_type);
            if let Some(currency) = activity.currency.as_deref() {
                println!("  Amount: {} atomic units {}", activity.amount, currency);
            } else {
                println!("  Amount: {}", format_amount(activity.amount));
            }
            if let (Some(currency), Some(amount)) = (activity.output_currency.as_deref(), activity.output_amount) {
                println!("  Converted to: {} {} atomic units", amount, currency);
            }
            for (currency, tax) in activity.protocol_tax {
                println!("  Protocol tax ({currency}): {tax} atomic units");
            }
            if let Some(message) = activity.message.as_deref() {
                println!("  Message: {message}");
            }
            println!("  Size: {} bytes", activity.size_bytes.unwrap_or(0));
        }
    }

    next_cursor
}
