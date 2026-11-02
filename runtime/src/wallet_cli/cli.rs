use super::*;

pub(super) fn create_wallet(
    args: &[String],
    default_race: novus::crypto::Race,
) -> Result<(), String> {
    let path = option(args, "--wallet").unwrap_or(DEFAULT_WALLET_PATH);
    let words = option(args, "--words")
        .unwrap_or("12")
        .parse::<usize>()
        .map_err(|_| "--words must be 12 or 24".to_string())?;
    let mnemonic = generate_bip39_mnemonic(words)?;
    let scheme = Signature::Schnorr;
    let race = option(args, "--race")
        .map(parse_race)
        .transpose()?
        .unwrap_or(default_race);
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

pub(super) fn restore_wallet(
    args: &[String],
    default_race: novus::crypto::Race,
) -> Result<(), String> {
    let path = option(args, "--wallet").unwrap_or(DEFAULT_WALLET_PATH);
    let phrase = option(args, "--mnemonic").ok_or("missing --mnemonic")?;
    let scheme = Signature::Schnorr;
    let race = option(args, "--race")
        .map(parse_race)
        .transpose()?
        .unwrap_or(default_race);
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
