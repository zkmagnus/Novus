use super::*;
use novus::transaction::{Currency, CurrencyConversion, Race, StakeAction};

#[derive(Deserialize)]
struct AccountLookup {
    total: u128,
    nonce: u64,
}

#[derive(Deserialize)]
struct ExchangeRatePolicy {
    race_exchange_rate_units_per_gold: std::collections::BTreeMap<String, u128>,
    race_tax_modifier_percent_points: std::collections::BTreeMap<String, u8>,
}

pub(super) fn recipient_address(_rpc: &str, value: &str) -> Result<Address, String> {
    novus::crypto::address_from_string_for_race(value)
        .map_err(|error| format!("invalid recipient address: {error}"))
}

fn account_nonce(rpc: &str, wallet: &LoadedWallet) -> Result<u64, String> {
    let address =
        novus::crypto::address_to_string(&wallet.address(), wallet.address().race().prefix());
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
            "novus" => Currency::Novus,
            _ => return Err("--currency must be cp, dlt, dsc, gold, or novus".into()),
        },
    };
    if currency
        .required_race()
        .is_some_and(|race| race != wallet_race)
    {
        return Err("race currency must match the wallet's race".into());
    }
    if currency == Currency::Gold && Race::from(recipient.race()) != wallet_race {
        return Err("Gold can only be transferred to an account of the same race".into());
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
    let policy: ExchangeRatePolicy = http_get_json(rpc, "/fee-policy")?;
    let transaction = automatic_fee_transaction(currency == Currency::Novus, |fee| {
        let mut intent = AccountIntent {
            sender: wallet.address(),
            race: requested_race,
            register_race: match race_lookup(rpc, &wallet.address())? {
                None => true,
                Some(known) if known == requested_race => false,
                Some(_) => return Err("wallet account is already assigned to another race".into()),
            },
            nonce,
            transfers: vec![Transfer::in_currency(recipient, currency, amount)],
            conversion: None,
            stake_action: None,
            fee: Amount::from_atomic(fee),
            tax: Amount::ZERO,
            message: message.clone(),
        };
        intent.tax = calculate_signed_tax(&intent, &policy, wallet_race)?;
        wallet.0.sign_intent(intent)
    })?;
    submit_or_print_transaction(args, &transaction)
}

pub(super) fn convert_currency(args: &[String]) -> Result<(), String> {
    let path = option(args, "--wallet").unwrap_or(DEFAULT_WALLET_PATH);
    let rpc = option(args, "--rpc").unwrap_or(DEFAULT_RPC_ADDR);
    let amount = parse_amount(option(args, "--amount").ok_or("missing --amount")?)?;
    let currency = match option(args, "--currency")
        .ok_or("missing --currency")?
        .to_ascii_lowercase()
        .as_str()
    {
        "cp" => Currency::Cp,
        "dlt" | "dalant" => Currency::Dalant,
        "dsc" | "disena" => Currency::Disena,
        "gold" => Currency::Gold,
        "novus" => Currency::Novus,
        _ => return Err("--currency must be cp, dlt, dsc, gold, or novus".into()),
    };
    let wallet = load_wallet(path)?;
    let race = Race::from(wallet.address().race());
    if currency.required_race().is_some_and(|owner| owner != race) {
        return Err("currency must belong to the wallet's race".into());
    }
    let to = match option(args, "--to") {
        Some("cp") => Currency::Cp,
        Some("dlt") | Some("dalant") => Currency::Dalant,
        Some("dsc") | Some("disena") => Currency::Disena,
        Some("gold") => Currency::Gold,
        Some("novus") => Currency::Novus,
        Some(_) => return Err("--to must be cp, dlt, dsc, gold, or novus".into()),
        None if currency == Currency::Gold => Currency::Novus,
        None => Currency::Gold,
    };
    if to.required_race().is_some_and(|owner| owner != race) {
        return Err("output currency must belong to the wallet's race".into());
    }
    let policy: ExchangeRatePolicy = http_get_json(rpc, "/fee-policy")?;
    let race_name = match race {
        Race::Accretia => "accretia",
        Race::Bellato => "bellato",
        Race::Cora => "cora",
    };
    let rate = if (currency, to) == (Currency::Gold, Currency::Novus) {
        novus::transaction::GOLD_PER_NOVUS
    } else {
        *policy
            .race_exchange_rate_units_per_gold
            .get(race_name)
            .ok_or("node did not provide the wallet race exchange rate")?
    };
    novus::transaction::conversion_output_amount(amount.as_atomic(), currency, to, rate)
        .ok_or("conversion output amount overflow at the current exchange rate")?;
    let nonce = account_nonce(rpc, &wallet)?;
    let transaction = automatic_fee_transaction(false, |fee| {
        let mut intent = AccountIntent {
            sender: wallet.address(),
            race,
            register_race: match race_lookup(rpc, &wallet.address())? {
                None => true,
                Some(known) if known == race => false,
                Some(_) => return Err("wallet account is already assigned to another race".into()),
            },
            nonce,
            transfers: vec![],
            conversion: Some(CurrencyConversion {
                from: currency,
                to,
                amount,
                quote_rate: rate,
            }),
            stake_action: None,
            fee: Amount::from_atomic(fee),
            tax: Amount::ZERO,
            message: None,
        };
        intent.tax = calculate_signed_tax(&intent, &policy, race)?;
        wallet.0.sign_intent(intent)
    })?;
    submit_or_print_transaction(args, &transaction)
}

pub(super) fn stake(args: &[String]) -> Result<(), String> {
    let operation = args
        .first()
        .ok_or("usage: wallet stake <delegate|undelegate|withdraw|heartbeat> ...")?;
    let action = match operation.as_str() {
        "delegate" | "undelegate" => {
            let amount = parse_amount(option(args, "--amount").ok_or("missing --amount")?)?;
            if operation == "delegate" {
                StakeAction::Delegate { amount }
            } else {
                StakeAction::Undelegate { amount }
            }
        }
        "withdraw" => StakeAction::Withdraw {
            amount: parse_amount(option(args, "--amount").ok_or("missing --amount")?)?,
        },
        "heartbeat" => StakeAction::WarHeartbeat,
        _ => {
            return Err("stake action must be delegate, undelegate, withdraw, or heartbeat".into());
        }
    };

    let path = option(args, "--wallet").unwrap_or(DEFAULT_WALLET_PATH);
    let rpc = option(args, "--rpc").unwrap_or(DEFAULT_RPC_ADDR);
    let wallet = load_wallet(path)?;
    let race = Race::from(wallet.address().race());
    let nonce = account_nonce(rpc, &wallet)?;
    if let Some(known_race) = race_lookup(rpc, &wallet.address())?
        && known_race != race
    {
        return Err("wallet account is already assigned to another race".into());
    }
    let transaction = wallet.0.sign_intent(AccountIntent {
        sender: wallet.address(),
        race,
        register_race: false,
        nonce,
        transfers: vec![],
        conversion: None,
        stake_action: Some(action),
        fee: Amount::ZERO,
        tax: Amount::ZERO,
        message: None,
    })?;
    submit_or_print_transaction(args, &transaction)
}

fn calculate_signed_tax(
    intent: &AccountIntent,
    policy: &ExchangeRatePolicy,
    race: Race,
) -> Result<Amount, String> {
    let race_name = match race {
        Race::Accretia => "accretia",
        Race::Bellato => "bellato",
        Race::Cora => "cora",
    };
    let tax_rate = *policy
        .race_tax_modifier_percent_points
        .get(race_name)
        .ok_or("node did not provide the wallet race tax rate")?;
    let exchange_rate = policy
        .race_exchange_rate_units_per_gold
        .get(race_name)
        .copied()
        .unwrap_or(novus::transaction::RACE_CURRENCY_PER_GOLD);
    let taxes = novus::consensus::calculate_transfer_taxes_with_policy(
        intent,
        |_| tax_rate,
        |_| exchange_rate,
    )
    .map_err(|error| error.to_string())?;
    taxes.values().try_fold(Amount::ZERO, |sum, tax| {
        sum.checked_add(*tax)
            .ok_or_else(|| "transaction tax amount overflow".to_string())
    })
}

fn race_lookup(rpc: &str, address: &Address) -> Result<Option<novus::transaction::Race>, String> {
    #[derive(Deserialize)]
    struct RaceResponse {
        race: Option<String>,
    }
    let text = novus::crypto::address_to_string(address, address.race().prefix());
    let response: RaceResponse = http_get_json(rpc, &format!("/race/{text}"))?;
    response
        .race
        .map(|race| match race.as_str() {
            "accretia" => Ok(novus::transaction::Race::Accretia),
            "bellato" => Ok(novus::transaction::Race::Bellato),
            "cora" => Ok(novus::transaction::Race::Cora),
            _ => Err("node returned an unknown race".into()),
        })
        .transpose()
}

fn automatic_fee_transaction(
    charge_novus_fee: bool,
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
        let required = if charge_novus_fee { size } else { 0 };
        if fee == required {
            return Ok(transaction);
        }
        fee = required;
    }
    Err("automatic transaction fee did not converge".into())
}

fn submit_or_print_transaction(args: &[String], transaction: &Transaction) -> Result<(), String> {
    let chain = novus::genesis::chain_context().map_err(|error| error.to_string())?;
    if !transaction
        .verify_authorization(chain)
        .map_err(|error| error.to_string())?
    {
        return Err("wallet produced an invalid transaction authorization".into());
    }
    let bytes = canonical_bytes(transaction).map_err(|error| error.to_string())?;
    if let Some(action) = transaction.intent.stake_action {
        match action {
            StakeAction::Delegate { amount } => println!(
                "Stake action: delegate {} atomic units to the race pool",
                amount.as_atomic()
            ),
            StakeAction::Undelegate { amount } => println!(
                "Stake action: undelegate {} atomic units from the race pool",
                amount.as_atomic()
            ),
            StakeAction::Withdraw { amount } => {
                println!("Stake action: withdraw {} atomic units", amount.as_atomic())
            }
            StakeAction::WarHeartbeat => println!(
                "War heartbeat: register this wallet as a node participant for the current epoch"
            ),
        }
    }
    let tax_currency = transaction
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
    if let Some(tax_currency) = tax_currency {
        println!(
            "Tax: {} atomic units ({tax_currency:?})",
            transaction.intent.tax.as_atomic()
        );
    }
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
