use super::history::{fetch_address_history, parse_history_limit, print_history_page};
use super::*;

pub(super) fn interactive_menu() -> Result<(), String> {
    loop {
        println!();
        println!("novus Wallet");
        println!("1. Create Wallet");
        println!("2. Import Wallet");
        println!("3. Show Address");
        println!("4. Show Balance");
        println!("5. Transaction History");
        println!("6. Account history");
        println!("7. Transfer");
        println!("8. Account details");
        println!("9. Explorer");
        println!("10. Exit");

        match prompt("Select")?.as_str() {
            "1" => {
                let path = prompt_default("Wallet file", DEFAULT_WALLET_PATH)?;
                let words = prompt_default("Mnemonic words (12 or 24)", "12")?;
                let mut args = vec!["--wallet".into(), path, "--words".into(), words];
                create_wallet(&args)?;
            }
            "2" => {
                let path = prompt_default("Wallet file", DEFAULT_WALLET_PATH)?;
                let phrase = prompt("Mnemonic")?;
                let mut args = vec!["--wallet".into(), path, "--mnemonic".into(), phrase];
                restore_wallet(&args)?;
            }
            "3" => {
                let path = prompt_default("Wallet file", DEFAULT_WALLET_PATH)?;
                let rpc = prompt_default("RPC", DEFAULT_RPC_ADDR)?;
                print_address(&["--wallet".into(), path, "--rpc".into(), rpc])?;
            }
            "4" => {
                let path = prompt_default("Wallet file", DEFAULT_WALLET_PATH)?;
                let rpc = prompt_default("RPC", DEFAULT_RPC_ADDR)?;
                print_balance(&["--wallet".into(), path, "--rpc".into(), rpc])?;
            }
            "5" => interactive_history()?,
            "6" => interactive_history()?,
            "7" => interactive_spend()?,
            "8" => interactive_block_explorer()?,
            "9" => interactive_block_explorer()?,
            "10" | "exit" | "quit" => return Ok(()),
            choice => println!("Unknown selection `{choice}`"),
        }
    }
}

fn interactive_history() -> Result<(), String> {
    let path = prompt_default("Wallet file", DEFAULT_WALLET_PATH)?;
    let rpc = prompt_default("RPC", DEFAULT_RPC_ADDR)?;
    let limit_text = prompt_default("Entries per page", &DEFAULT_HISTORY_LIMIT.to_string())?;
    let limit = parse_history_limit(&limit_text)?;
    let bytes =
        Zeroizing::new(fs::read(&path).map_err(|error| format!("failed to read {path}: {error}"))?);
    let raw_address = wallet_address_from_file_bytes(&bytes)?;
    let address = novus::crypto::address_to_string(&raw_address, raw_address.race().prefix());

    let mut before: Option<String> = None;
    let mut previous = Vec::<Option<String>>::new();

    loop {
        println!();
        let history = fetch_address_history(&rpc, &address, limit, before.as_deref())?;
        let next_cursor = print_history_page(history);

        println!();
        if next_cursor.is_some() {
            println!("N. Next Page");
        }
        if before.is_some() {
            println!("P. Previous Page");
        }
        println!("Q. Back");

        match prompt("Select")?.to_ascii_lowercase().as_str() {
            "n" | "next" if next_cursor.is_some() => {
                previous.push(before.clone());
                before = next_cursor;
            }
            "p" | "previous" if before.is_some() => {
                before = previous.pop().unwrap_or(None);
            }
            "q" | "back" | "quit" => return Ok(()),
            choice => println!("Unknown selection `{choice}`"),
        }
    }
}

fn interactive_spend() -> Result<(), String> {
    let rpc = prompt_default("RPC", DEFAULT_RPC_ADDR)?;
    let recipient = prompt("Recipient address")?;
    super::transaction::recipient_address(&rpc, &recipient)?;
    let amount = prompt("BELLY amount")?;
    let message = prompt("Message (optional, public)")?;
    let mut args = vec![
        "--to".into(),
        recipient,
        "--amount".into(),
        amount,
        "--rpc".into(),
        rpc,
    ];
    if !message.is_empty() {
        args.extend(["--message".into(), message]);
    }
    args.extend([
        "--wallet".into(),
        prompt_default("Wallet file", DEFAULT_WALLET_PATH)?,
    ]);
    sign_spend(&args)
}

fn interactive_block_explorer() -> Result<(), String> {
    let rpc = prompt_default("RPC", DEFAULT_RPC_ADDR)?;
    println!("1. Address activity");
    println!("2. Transaction by Hash");
    println!("3. Latest blocks");
    println!("4. Block by height");
    let response: serde_json::Value = match prompt("Select")?.as_str() {
        "1" => {
            let address = prompt("Address")?;
            novus::crypto::address_from_string_for_race(&address)
                .map_err(|_| "invalid address".to_string())?;
            http_get_json(&rpc, &format!("/explorer/address/{address}"))?
        }
        "2" => {
            let hash = prompt("Hash")?;
            if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err("Tx Hash must be 64 hexadecimal characters".into());
            }
            http_get_json(&rpc, &format!("/explorer/transaction/{hash}"))?
        }
        "3" => http_get_json(&rpc, "/blocks/latest")?,
        "4" => {
            let height = prompt("Block height")?;
            if height.parse::<u64>().is_err() {
                return Err("block height must be an unsigned integer".into());
            }
            http_get_json(&rpc, &format!("/block/{height}"))?
        }
        choice => return Err(format!("unknown explorer selection `{choice}`")),
    };
    print_human_json(&response);
    Ok(())
}

fn prompt(label: &str) -> Result<String, String> {
    print!("{label}: ");
    io::stdout().flush().map_err(|error| error.to_string())?;
    let mut value = String::new();
    io::stdin()
        .read_line(&mut value)
        .map_err(|error| format!("failed to read input: {error}"))?;
    Ok(value.trim().to_string())
}

fn prompt_default(label: &str, default: &str) -> Result<String, String> {
    let value = prompt(&format!("{label} [{default}]"))?;
    Ok(if value.is_empty() {
        default.to_string()
    } else {
        value
    })
}

pub(super) fn create_wallet(args: &[String]) -> Result<(), String> {
    let path = option(args, "--wallet").unwrap_or(DEFAULT_WALLET_PATH);
    let words = option(args, "--words")
        .unwrap_or("12")
        .parse::<usize>()
        .map_err(|_| "--words must be 12 or 24".to_string())?;
    let mnemonic = generate_bip39_mnemonic(words)?;
    let scheme = Signature::Schnorr;
    let race = parse_race(option(args, "--race").unwrap_or("accretia"))?;
    let mut wallet = wallet_from_bip39_mnemonic_for_race(&mnemonic, scheme, race)?;
    wallet.mnemonic = Some(mnemonic.to_string());
    let address = wallet.address;
    write_wallet(path, &wallet)?;
    println!("signature_scheme: {scheme}");
    println!(
        "address: {}",
    novus::crypto::address_to_string(&address, race.prefix())
    );
    println!("mnemonic: {}", mnemonic.as_str());
    println!("wallet: {path}");
    Ok(())
}

pub(super) fn restore_wallet(args: &[String]) -> Result<(), String> {
    let path = option(args, "--wallet").unwrap_or(DEFAULT_WALLET_PATH);
    let phrase = option(args, "--mnemonic").ok_or("missing --mnemonic")?;
    let scheme = Signature::Schnorr;
    let race = parse_race(option(args, "--race").unwrap_or("accretia"))?;
    let mut wallet = wallet_from_bip39_mnemonic_for_race(phrase, scheme, race)?;
    wallet.mnemonic = Some(phrase.to_string());
    let address = wallet.address;
    write_wallet(path, &wallet)?;
    println!("signature_scheme: {scheme}");
    println!(
        "address: {}",
        novus::crypto::address_to_string(&address, race.prefix())
    );
    println!("wallet: {path}");
    Ok(())
}

fn parse_race(value: &str) -> Result<novus::crypto::Race, String> {
    match value {
        "accretia" => Ok(novus::crypto::Race::Accretia),
        "bellato" => Ok(novus::crypto::Race::Bellato),
        "cora" => Ok(novus::crypto::Race::Cora),
        _ => Err("--race must be accretia, bellato, or cora".into()),
    }
}

pub(super) fn print_address(args: &[String]) -> Result<(), String> {
    let path = option(args, "--wallet").unwrap_or(DEFAULT_WALLET_PATH);
    let bytes =
        Zeroizing::new(fs::read(path).map_err(|error| format!("failed to read {path}: {error}"))?);
    let address = wallet_address_from_file_bytes(&bytes)?;
    let address = novus::crypto::address_to_string(&address, address.race().prefix());
    println!("Address: {address}");
    Ok(())
}

fn human_label(key: &str) -> String {
    key.split('_')
        .map(|part| match part {
            "id" => "ID".to_string(),
            "tx" => "TX".to_string(),
            "BELLY" => "BELLY".to_string(),
            "bountyhunter" => "BountyHunter".to_string(),
            other => {
                let mut chars = other.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn print_human_json(value: &serde_json::Value) {
    print_human_json_value(None, value, 0);
}

fn print_human_json_value(label: Option<&str>, value: &serde_json::Value, indent: usize) {
    let padding = " ".repeat(indent);

    match value {
        serde_json::Value::Null => {
            if let Some(label) = label {
                println!("{padding}{}: -", human_label(label));
            } else {
                println!("{padding}-");
            }
        }
        serde_json::Value::Bool(value) => {
            if let Some(label) = label {
                println!("{padding}{}: {value}", human_label(label));
            } else {
                println!("{padding}{value}");
            }
        }
        serde_json::Value::Number(value) => {
            if let Some(label) = label {
                println!("{padding}{}: {value}", human_label(label));
            } else {
                println!("{padding}{value}");
            }
        }
        serde_json::Value::String(value) => {
            if let Some(label) = label {
                println!("{padding}{}: {value}", human_label(label));
            } else {
                println!("{padding}{value}");
            }
        }
        serde_json::Value::Array(values) => {
            let child_indent = if let Some(label) = label {
                println!("{padding}{}:", human_label(label));
                indent + 2
            } else {
                indent
            };

            if values.is_empty() {
                println!("{}None", " ".repeat(child_indent));
                return;
            }

            for (index, value) in values.iter().enumerate() {
                let item = format!("Item {}", index + 1);
                print_human_json_value(Some(&item), value, child_indent);
            }
        }
        serde_json::Value::Object(values) => {
            let child_indent = if let Some(label) = label {
                println!("{padding}{}:", human_label(label));
                indent + 2
            } else {
                indent
            };

            for (key, value) in values {
                print_human_json_value(Some(key), value, child_indent);
            }
        }
    }
}
