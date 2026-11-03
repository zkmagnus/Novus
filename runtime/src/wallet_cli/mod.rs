use std::{fs, path::Path};

use novus::{
    consensus::{Amount, DECIMALS},
    crypto::{Address, Signature, canonical_bytes},
    transaction::{AccountIntent, Transaction, Transfer},
};
use serde::Deserialize;
use wallet::{
    Wallet, generate_bip39_mnemonic, wallet_address_from_file_bytes, wallet_file_bytes,
    wallet_from_bip39_mnemonic_for_race, wallet_from_file_bytes,
};
use zeroize::{Zeroize, Zeroizing};

const DEFAULT_WALLET_PATH: &str = "wallet.json";
const DEFAULT_HISTORY_LIMIT: usize = 50;
const MAX_HISTORY_LIMIT: usize = 250;
const HISTORY_CURSOR_HEX_LEN: usize = 34;

struct LoadedWallet(Wallet);

impl LoadedWallet {
    fn address(&self) -> Address {
        self.0.address
    }
}
#[cfg(feature = "mainnet")]
const DEFAULT_RPC_ADDR: &str = "127.0.0.1:10593";

#[derive(Deserialize)]
struct BalanceResponse {
    total: u128,
    nonce: u64,
    #[serde(default)]
    balances: std::collections::BTreeMap<String, u128>,
}

#[derive(Deserialize)]
struct NodeStatusResponse {
    #[serde(default)]
    race_block_rewards: std::collections::BTreeMap<String, u128>,
    #[serde(default)]
    tax_vault: std::collections::BTreeMap<String, u128>,
}

#[derive(Deserialize)]
struct AddressHistoryResponse {
    address: String,
    tip_height: u64,
    emission_count: usize,
    activities: Vec<AddressActivity>,
    #[serde(default)]
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
struct AddressActivity {
    height: u64,
    block_hash: String,
    hash: Option<String>,
    #[serde(rename = "type")]
    activity_type: String,
    direction: String,
    amount: u128,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    output_currency: Option<String>,
    #[serde(default)]
    output_amount: Option<u128>,
    #[serde(default)]
    protocol_tax: std::collections::BTreeMap<String, u128>,
    #[serde(default)]
    message: Option<String>,
    size_bytes: Option<usize>,
}

#[derive(Deserialize)]
struct SubmitTransactionResponse {
    hash: String,
}

pub fn run_for_race(
    mut args: Vec<String>,
    default_race: novus::crypto::Race,
) -> Result<(), String> {
    if let Err(error) = memory::harden_process_memory() {
        eprintln!("node wallet: warning: process memory hardening failed: {error}");
    }
    let result = match args.first().map(String::as_str) {
        None => Err("missing wallet command; use `wallet help`".into()),
        Some("new") => create_wallet(&args[1..], default_race),
        Some("restore") => restore_wallet(&args[1..], default_race),
        Some("address") => print_address(&args[1..]),
        Some("balance") => print_balance(&args[1..]),
        Some("history") => print_history(&args[1..]),
        Some("sign-spend") | Some("send") => sign_spend(&args[1..]),
        Some("convert") => transaction::convert_currency(&args[1..]),
        Some("stake") => transaction::stake(&args[1..]),
        Some("version") | Some("--version") | Some("-V") => {
            println!("Novus wallet {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("help") | Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        Some(command) => Err(format!("unknown command `{command}`")),
    };
    args.zeroize();
    result
}

mod balance;
mod cli;
mod history;
mod memory;
mod rpc;
mod transaction;
mod util;
mod wallet_file;

use wallet_file::{load_wallet, write_wallet};

use cli::{create_wallet, print_address, restore_wallet};
use util::{format_amount, has_flag, option, parse_amount, print_help};

use balance::print_balance;
use history::print_history;
use rpc::{http_get_json, http_post_bytes};
use transaction::sign_spend;

#[cfg(test)]
use history::{parse_history_limit, validate_history_cursor};

#[cfg(test)]
mod tests;
