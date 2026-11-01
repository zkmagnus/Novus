use super::*;

pub(super) fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
}

pub(super) fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|argument| argument == name)
}

pub(super) fn parse_amount(value: &str) -> Result<Amount, String> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if fraction.len() > DECIMALS as usize || whole.is_empty() {
        return Err(format!("invalid novus amount `{value}`"));
    }
    let whole = whole
        .parse::<u128>()
        .map_err(|_| format!("invalid novus amount `{value}`"))?;
    let mut fraction_text = fraction.to_string();
    fraction_text.extend(std::iter::repeat_n('0', DECIMALS as usize - fraction.len()));
    let fraction = fraction_text
        .parse::<u128>()
        .map_err(|_| format!("invalid novus amount `{value}`"))?;
    let units = whole
        .checked_mul(Amount::ATOMIC_UNITS_PER_COIN)
        .and_then(|n| n.checked_add(fraction))
        .ok_or_else(|| "novus amount overflow".to_string())?;
    if units == 0 {
        return Err("novus amount must be positive".into());
    }
    Ok(Amount::from_atomic(units))
}

pub(super) fn format_amount(units: u128) -> String {
    let whole = units / Amount::ATOMIC_UNITS_PER_COIN;
    let fraction = units % Amount::ATOMIC_UNITS_PER_COIN;
    format!(
        "{whole}.{fraction:0width$} novus",
        width = DECIMALS as usize
    )
}

pub(super) fn print_help() {
    println!(
        "wallet [menu]\nwallet new [--wallet PATH] [--words 12|24] [--race accretia|bellato|cora]\nwallet restore --mnemonic PHRASE [--wallet PATH] [--race accretia|bellato|cora]\nwallet address [--wallet PATH]\nwallet balance [--wallet PATH] [--rpc ADDRESS]\nwallet history [--limit 1..=250] [--before CURSOR]\nwallet sign-spend --to ADDRESS --amount AMOUNT [--currency cp|dlt|dsc|gold] [--message TEXT] [--rpc ADDRESS] [--wallet PATH] [--offline]\nwallet convert --amount AMOUNT --currency cp|dlt|dsc|gold [--rpc ADDRESS] [--wallet PATH] [--offline]\nwallet version\n\nwallet convert uses the current Chip War quote, based on 100 race-currency units per Gold and adjusted in 5% steps within a 20% band. Gold redemption pays the wallet race's currency from that nation's reserve. Transaction fees are paid in the sender race's native currency and held for a miner of that race. Wallet transactions use account balances and sequential nonces, signed with BIP340 Schnorr. The wallet estimates a fee of 10 atomic units per transaction byte.\nRunning without a command opens the interactive menu."
    );
}
