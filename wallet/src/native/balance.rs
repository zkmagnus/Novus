use super::*;

pub(super) fn print_balance(args: &[String]) -> Result<(), String> {
    let path = option(args, "--wallet").unwrap_or(DEFAULT_WALLET_PATH);
    let rpc = option(args, "--rpc").unwrap_or(DEFAULT_RPC_ADDR);
    let bytes =
        Zeroizing::new(fs::read(path).map_err(|error| format!("failed to read {path}: {error}"))?);
    let raw_address = wallet_address_from_file_bytes(&bytes)?;
    let address = novus::crypto::address_to_string(&raw_address, raw_address.race().prefix());
    let balance: BalanceResponse = http_get_json(rpc, &format!("/balance/{address}"))?;
    let status: NodeStatusResponse = http_get_json(rpc, "/status")?;

    println!("Address: {address}");
    println!("Available: {}", format_amount(balance.total));
    for (currency, amount) in &balance.balances {
        println!("{}: {}", currency.to_uppercase(), format_amount(*amount));
    }
    println!("Nonce: {}", balance.nonce);
    println!("Total Mined: {}", format_amount(status.total_mined));
    println!("Total Burned: {}", format_amount(status.total_burned));
    println!("Total Supply: {}", format_amount(status.total_supply));
    for (currency, amount) in status.race_block_rewards {
        println!("Total {currency} block rewards: {amount} atomic units");
    }
    for (currency, amount) in status.tax_vault {
        println!("{currency} tax vault: {amount} atomic units");
    }

    Ok(())
}
