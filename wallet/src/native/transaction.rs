use super::*;
use novus::transaction::{Currency, CurrencyConversion, Race};

#[derive(Deserialize)]
struct AccountLookup {
    total: u128,
    nonce: u64,
}

#[derive(Deserialize)]
struct ExchangeRatePolicy {
    race_exchange_rate_units_per_gold: std::collections::BTreeMap<String, u128>,
}

pub(super) fn recipient_address(_rpc: &str, value: &str) -> Result<Address, String> {
    novus::crypto::address_from_string_for_race(value)
        .map_err(|error| format!("invalid recipient address: {error}"))
}

fn account_nonce(rpc: &str, wallet: &LoadedWallet) -> Result<u64, String> {
    let address = novus::crypto::address_to_string(&wallet.address(), wallet.address().race().prefix());
    let account: AccountLookup = http_get_json(rpc, &format!("/balance/{address}"))?;
    let _balance = account.total;
    Ok(account.nonce)
}

pub(super) fn sign_spend(args: &[String]) -> Result<(), String> {
    let path = option(args, "--wallet").unwrap_or(DEFAULT_WALLET_PATH);
    let rpc = option(args, "--rpc").unwrap_or(DEFAULT_RPC_ADDR);
    let recipient = recipient_address(rpc, option(args, "--to").ok_or("missing --to")?)?;
    let amount = parse_amount(option(args, "--amount").ok_or("missing --amount")?)?;
    let message = option(args, "--message").map(str::to_string);
    let wallet = load_wallet(path)?;
    let wallet_race: Race = wallet.address().race().into();
    let currency = match option(args, "--currency").map(str::to_ascii_lowercase) {
        None => Currency::for_race_reward(wallet_race),
        Some(value) => match value.as_str() {
            "cp" => Currency::Cp,
            "dlt" | "dalant" => Currency::Dalant,
            "dsc" | "disena" => Currency::Disena,
            "gold" => Currency::Gold,
            _ => return Err("--currency must be cp, dlt, dsc, or gold".into()),
        },
    };
    if currency.required_race().is_some_and(|race| race != wallet_race) {
        return Err("race currency must match the wallet's race".into());
    }
    let requested_race = match option(args, "--race") {
        None => wallet_race,
        Some("accretia") => Race::Accretia,
        Some("bellato") => Race::Bellato,
        Some("cora") => Race::Cora,
        Some(_) => return Err("--race must be accretia, bellato, or cora".into()),
    };
    if requested_race != wallet_race {
        return Err("--race does not match the race embedded in this wallet address".into());
    }
    let nonce = account_nonce(rpc, &wallet)?;
    let transaction = automatic_fee_transaction(|fee| {
        let intent = AccountIntent {
            sender: wallet.address(),
            race: requested_race,
            register_race: match race_lookup(rpc, &wallet.address())? {
                None => true,
                Some(known) if known == requested_race => false,
                Some(_) => return Err("wallet account is already assigned to another race".into()),
            },
            nonce,
            transfers: vec![Transfer::in_currency(recipient, currency, amount)],
            fee: Amount::from_atomic(fee),
            message: message.clone(),
        };
        wallet
            .0
            .sign_intent(intent)
    })?;
    submit_or_print_transaction(args, &transaction, &wallet.0.public_key)
}

pub(super) fn convert_to_gold(args: &[String]) -> Result<(), String> {
    let path = option(args, "--wallet").unwrap_or(DEFAULT_WALLET_PATH);
    let rpc = option(args, "--rpc").unwrap_or(DEFAULT_RPC_ADDR);
    let amount = parse_amount(option(args, "--amount").ok_or("missing --amount")?)?;
    let currency = match option(args, "--currency").ok_or("missing --currency")?.to_ascii_lowercase().as_str() {
        "cp" => Currency::Cp,
        "dlt" | "dalant" => Currency::Dalant,
        "dsc" | "disena" => Currency::Disena,
        "gold" => Currency::Gold,
        _ => return Err("--currency must be cp, dlt, dsc, or gold".into()),
    };
    let wallet = load_wallet(path)?;
    let race = Race::from(wallet.address().race());
    if currency != Currency::Gold && currency.required_race() != Some(race) {
        return Err("currency must belong to the wallet's race".into());
    }
    let policy: ExchangeRatePolicy = http_get_json(rpc, "/fee-policy")?;
    let race_name = match race { Race::Accretia => "accretia", Race::Bellato => "bellato", Race::Cora => "cora" };
    let rate = *policy.race_exchange_rate_units_per_gold.get(race_name)
        .ok_or("node did not provide the wallet race exchange rate")?;
    novus::transaction::exchange_output_amount(amount.as_atomic(), currency == Currency::Gold, rate)
        .ok_or("conversion output amount overflow at the current exchange rate")?;
    let nonce = account_nonce(rpc, &wallet)?;
    let transaction = automatic_fee_transaction(|fee| {
        let intent = AccountIntent {
            sender: wallet.address(),
            race,
            register_race: match race_lookup(rpc, &wallet.address())? {
                None => true,
                Some(known) if known == race => false,
                Some(_) => return Err("wallet account is already assigned to another race".into()),
            },
            nonce,
            transfers: vec![],
            conversion: Some(CurrencyConversion { from: currency, amount, quote_rate: rate }),
            fee: Amount::from_atomic(fee),
            message: None,
        };
        wallet.0.sign_intent(intent)
    })?;
    submit_or_print_transaction(args, &transaction, &wallet.0.public_key)
}

fn race_lookup(rpc: &str, address: &Address) -> Result<Option<novus::transaction::Race>, String> {
    #[derive(Deserialize)]
    struct RaceResponse { race: Option<String> }
    let text = novus::crypto::address_to_string(address, address.race().prefix());
    let response: RaceResponse = http_get_json(rpc, &format!("/race/{text}"))?;
    response.race.map(|race| match race.as_str() {
        "accretia" => Ok(novus::transaction::Race::Accretia),
        "bellato" => Ok(novus::transaction::Race::Bellato),
        "cora" => Ok(novus::transaction::Race::Cora),
        _ => Err("node returned an unknown race".into()),
    }).transpose()
}

fn automatic_fee_transaction(
    mut build: impl FnMut(u128) -> Result<Transaction, String>,
) -> Result<Transaction, String> {
    let mut fee = 0_u128;
    for _ in 0..8 {
        let transaction = build(fee)?;
        let size = u128::try_from(
            canonical_bytes(&transaction)
                .map_err(|error| error.to_string())?
                .len(),
        )
        .map_err(|_| "transaction size overflow")?;
        let required = size
            .checked_mul(10)
            .ok_or("automatic transaction fee overflow")?;
        if fee == required {
            return Ok(transaction);
        }
        fee = required;
    }
    Err("automatic transaction fee did not converge".into())
}

fn submit_or_print_transaction(
    args: &[String],
    transaction: &Transaction,
    public_key: &novus::crypto::PublicKey,
) -> Result<(), String> {
    let chain = novus::genesis::chain_context().map_err(|error| error.to_string())?;
    if !transaction
        .verify_authorization(chain)
        .map_err(|error| error.to_string())?
    {
        return Err("wallet produced an invalid transaction authorization".into());
    }
    let bytes = canonical_bytes(transaction).map_err(|error| error.to_string())?;
    if has_flag(args, "--offline") {
        println!("Transaction Hex: {}", hex::encode(&bytes));
        println!("Bytes: {}", bytes.len());
        return Ok(());
    }
    let rpc = option(args, "--rpc").unwrap_or(DEFAULT_RPC_ADDR);
    let response: SubmitTransactionResponse = http_post_bytes(rpc, "/transaction", &bytes)?;
    println!("Tx Hash: {}", response.hash);
    println!("Bytes: {}", bytes.len());
    Ok(())
}
