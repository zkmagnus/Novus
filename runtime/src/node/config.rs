use super::*;

impl RunConfig {
    pub(super) fn parse(args: &[String]) -> Result<Self, String> {
        let mut database = PathBuf::from(default_database());
        let mut p2p_listen = default_p2p_listen().to_string();
        let mut rpc_listen = default_rpc_listen().to_string();
        let mut peers = Vec::new();
        let mut miner = None;
        let mut public_addr = None;
        let mut nat_traversal = false;
        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "--data" => {
                    index += 1;
                    database = PathBuf::from(args.get(index).ok_or("missing value for --data")?);
                }
                "--p2p" => {
                    index += 1;
                    p2p_listen = args.get(index).ok_or("missing value for --p2p")?.clone();
                }
                "--rpc" => {
                    index += 1;
                    rpc_listen = args.get(index).ok_or("missing value for --rpc")?.clone();
                }
                "--peer" => {
                    index += 1;
                    peers.push(args.get(index).ok_or("missing value for --peer")?.clone());
                }
                "--miner" => {
                    index += 1;
                    miner = Some(args.get(index).ok_or("missing value for --miner")?.clone());
                }
                "--public-addr" => {
                    index += 1;
                    public_addr = Some(
                        args.get(index)
                            .ok_or("missing value for --public-addr")?
                            .parse()
                            .map_err(|_| "invalid --public-addr socket address")?,
                    );
                }
                "--nat-traversal" => nat_traversal = true,
                option => return Err(format!("unknown node run option `{option}`")),
            }
            index += 1;
        }
        Ok(Self {
            database,
            p2p_listen,
            rpc_listen,
            peers,
            miner,
            public_addr,
            nat_traversal,
        })
    }
}

pub(super) fn configure_public_address(config: &RunConfig) -> Result<(), String> {
    if config.public_addr.is_some() && config.nat_traversal {
        return Err("use either --public-addr or --nat-traversal, not both".into());
    }
    if let Some(address) = config.public_addr {
        if !is_admissible_discovered_peer(&address) {
            return Err("--public-addr must be a public, non-zero socket address".into());
        }
        set_advertised_peer(Some(address))?;
    }
    if !config.nat_traversal {
        return Ok(());
    }
    let listener: SocketAddr = config
        .p2p_listen
        .parse()
        .map_err(|_| "--nat-traversal requires a numeric P2P listen address")?;
    let mapping = crate::nat::map_tcp_listener(listener, DEFAULT_NAT_LEASE)?;
    if !is_admissible_discovered_peer(&mapping.public_addr) {
        return Err("NAT gateway returned a non-public address".into());
    }
    set_advertised_peer(Some(mapping.public_addr))?;
    println!(
        "nat: mapped public_addr={} lease_secs={}",
        mapping.public_addr,
        mapping.lease.as_secs()
    );
    thread::spawn(move || {
        loop {
            thread::sleep(mapping.lease / 2);
            match crate::nat::map_tcp_listener(listener, DEFAULT_NAT_LEASE) {
                Ok(refreshed) if is_admissible_discovered_peer(&refreshed.public_addr) => {
                    if let Err(error) = set_advertised_peer(Some(refreshed.public_addr)) {
                        eprintln!("node: update NAT public address: {error}");
                    }
                }
                Ok(_) => eprintln!("node: NAT refresh returned a non-public address"),
                Err(error) => eprintln!("node: NAT mapping refresh failed: {error}"),
            }
        }
    });
    Ok(())
}

pub(super) fn advertised_peer() -> Result<Option<SocketAddr>, String> {
    ADVERTISED_PEER
        .get_or_init(|| RwLock::new(None))
        .read()
        .map(|address| *address)
        .map_err(|_| "advertised peer lock is poisoned".into())
}

pub(super) fn set_advertised_peer(address: Option<SocketAddr>) -> Result<(), String> {
    *ADVERTISED_PEER
        .get_or_init(|| RwLock::new(None))
        .write()
        .map_err(|_| "advertised peer lock is poisoned")? = address;
    Ok(())
}

pub(super) fn print_network_info() -> Result<(), String> {
    println!("genesis: {}", hex::encode(EXPECTED_GENESIS_HASH.0));
    println!(
        "chain_spec: {}",
        hex::encode(chain_spec_hash().map_err(|error| error.to_string())?.0)
    );
    println!("p2p_protocol: {P2P_PROTOCOL_VERSION}");
    println!("pow: {}", novus::consensus::POW_ALGORITHM);
    println!("difficulty: {}", novus::consensus::DIFFICULTY_ALGORITHM);
    Ok(())
}

pub(super) fn print_help() {
    println!(
        r#"node run [--data PATH] [--p2p ADDRESS] [--rpc ADDRESS] [--peer ADDRESS]... [--miner ADDRESS] [--public-addr ADDRESS | --nat-traversal]
node network [data-dir] [listen-address] [peer-address...]
node rpc [data-dir] [listen-address]
node p2p-listen [data-dir] [listen-address]
node peer [data-dir] <peer-address>
node info
node check [data-dir]
node balance [data-dir] <address>
node account [data-dir] <address> (alias: balance)
node wallet new [--wallet PATH] [--words 12|24]
node wallet restore --mnemonic PHRASE [--wallet PATH]
node wallet address|balance|history [OPTIONS]
node wallet send --to ADDRESS --amount AMOUNT [OPTIONS]
node wallet convert --amount AMOUNT --currency CURRENCY [--to CURRENCY] [OPTIONS]

Convert examples (run with `node wallet convert`):
  --amount 1000 --currency cp --to gold
  --amount 1000 --currency gold --to novus (1,000 Gold = 1 Novus)
  --amount 1 --currency gold --to cp|dlt|dsc

node mempool [data-dir]
node mine-block [data-dir] <miner-address>
node submit-transaction [data-dir] <transaction-hex>
node submit-block [data-dir] <block-hex>
node version"#
    );
}

pub(super) fn database_path(path: Option<&str>) -> PathBuf {
    PathBuf::from(path.unwrap_or(default_database()))
}

#[cfg(feature = "mainnet")]
pub(super) fn default_p2p_listen() -> &'static str {
    "0.0.0.0:10999"
}

#[cfg(feature = "mainnet")]
pub(super) fn default_rpc_listen() -> &'static str {
    "127.0.0.1:10593"
}

#[cfg(feature = "mainnet")]
pub(super) fn default_database() -> &'static str {
    "./data/mainnet"
}
