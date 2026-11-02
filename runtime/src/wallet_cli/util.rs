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
    format!("{whole}.{fraction:0width$}", width = DECIMALS as usize)
}

pub(super) fn print_help() {
    println!(
        r#"node wallet new [--wallet PATH] [--words 12|24] [--race accretia|bellato|cora]
node wallet restore --mnemonic PHRASE [--wallet PATH] [--race accretia|bellato|cora]
node wallet address [--wallet PATH]
node wallet balance [--wallet PATH] [--rpc ADDRESS]
node wallet history [--limit 1..=250] [--before CURSOR]
node wallet send --to ADDRESS --amount AMOUNT [--currency cp|dlt|dsc|gold|novus] [--message TEXT] [--rpc ADDRESS] [--wallet PATH] [--offline]
node wallet convert --amount AMOUNT --currency CURRENCY [--to CURRENCY] [--rpc ADDRESS] [--wallet PATH] [--offline]
node wallet version

Convert examples:
  node wallet convert --amount 1000 --currency cp --to gold
  node wallet convert --amount 1000 --currency gold --to novus  (1,000 Gold = 1 Novus)
  node wallet convert --amount 1 --currency gold --to cp|dlt|dsc

Amounts use coin units with up to 6 decimal places. New wallets default to the node's race. Gold transfers are restricted to accounts of the sender's race; Novus transfers work across races. Transfer relay fees are in Gold except Novus transfers, which pay in Novus and are credited directly to the block miner. Conversion relay fees use the source currency. Chip War tax adjustments for the sender race apply to all taxable transactions. Taxes use the source currency; Novus transfers have 0% tax. Novus cannot be converted back to Gold. Wallet transactions use sequential nonces and BIP340 Schnorr signatures."#
    );
}
