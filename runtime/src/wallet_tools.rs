use novus::crypto::Race;
use std::{fs::OpenOptions, io::Write, path::Path};
use wallet::{generate_bip39_mnemonic, wallet_file_bytes, wallet_from_bip39_mnemonic_for_race};
use zeroize::Zeroizing;

pub fn create_wallet(race: Race, words: usize) -> Result<(String, Zeroizing<Vec<u8>>), String> {
    let mnemonic = generate_bip39_mnemonic(words)?;
    let mut wallet =
        wallet_from_bip39_mnemonic_for_race(&mnemonic, novus::crypto::Signature::Schnorr, race)?;
    wallet.mnemonic = Some(mnemonic.to_string());
    let address = novus::crypto::address_to_string(&wallet.address, race.prefix());
    Ok((address, wallet_file_bytes(&wallet)?))
}

pub fn write_wallet_file(path: impl AsRef<Path>, bytes: &[u8]) -> Result<(), String> {
    let path = path.as_ref();
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("create wallet file {}: {error}", path.display()))?;
    file.write_all(bytes)
        .map_err(|error| format!("write wallet file {}: {error}", path.display()))
}
