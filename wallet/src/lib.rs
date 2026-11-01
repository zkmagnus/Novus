use novus::{
    crypto::{
        Address, PublicKey, Signature, SigningSeed, address_from_public_key_for_race, address_from_string_for_race,
        address_to_string, hash_bytes,
    },
    transaction::{AccountIntent, TransactionAuthorization, Transaction},
};
use bip39::{Language, Mnemonic};

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

pub const BIP39_MNEMONIC_DEFAULT_WORDS: usize = 12;
pub const BIP39_MNEMONIC_12_ENTROPY_BYTES: usize = 16;
pub const BIP39_MNEMONIC_24_ENTROPY_BYTES: usize = 32;

#[derive(Debug)]
pub struct Wallet {
    pub mnemonic: Option<String>,
    pub address: Address,
    pub public_key: PublicKey,
    signing_seed: SigningSeed,
}

impl Drop for Wallet {
    fn drop(&mut self) {
        self.mnemonic.zeroize();
    }
}

#[derive(Deserialize, Serialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
struct WalletFile {
    address: String,
    mnemonic: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(alias = "signature_nakama")]
    signature_scheme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    public_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    private_key: Option<String>,
}

#[derive(Deserialize)]
struct WalletHeader {
    address: String,
}

pub fn wallet_address_from_file_bytes(bytes: &[u8]) -> Result<Address, String> {
    let header: WalletHeader = serde_json::from_slice(bytes)
        .map_err(|error| format!("failed to parse wallet: {error}"))?;
    address_from_string_for_race(&header.address)
        .map_err(|error| format!("invalid wallet address: {error}"))
}

pub fn wallet_file_bytes(wallet: &Wallet) -> Result<Zeroizing<Vec<u8>>, String> {
    let mnemonic = wallet
        .mnemonic
        .as_deref()
        .ok_or_else(|| "wallet has no mnemonic recovery material".to_string())?;
    decode_bip39_mnemonic(mnemonic)?;
    let wallet_file = WalletFile {
        address: address_to_string(&wallet.address, wallet.address.race().prefix()),
        mnemonic: mnemonic.to_string(),
        signature_scheme: Some(wallet.scheme().as_str().to_string()),
        public_key: Some(hex::encode(&wallet.public_key.bytes)),
        private_key: Some({
            let seed = wallet.signing_seed.dangerous_export_seed();
            hex::encode(&seed[..])
        }),
    };
    serde_json::to_vec_pretty(&wallet_file)
        .map(Zeroizing::new)
        .map_err(|error| format!("failed to encode wallet file: {error}"))
}

pub fn wallet_from_file_bytes(bytes: &[u8]) -> Result<Wallet, String> {
    let wallet_file: WalletFile = serde_json::from_slice(bytes)
        .map_err(|error| format!("failed to parse wallet: {error}"))?;
    let scheme = wallet_file
        .signature_scheme
        .as_deref()
        .ok_or("wallet file does not contain a signature scheme")?
        .parse::<Signature>()
        .map_err(str::to_string)?;
    let stored_address = address_from_string_for_race(&wallet_file.address)
    .map_err(|error| format!("invalid wallet address: {error}"))?;
    let mut wallet = wallet_from_bip39_mnemonic_for_race(&wallet_file.mnemonic, scheme, stored_address.race())?;
    if wallet.address != stored_address {
        return Err("wallet address does not match its mnemonic and signature scheme".to_string());
    }
    if let Some(public_key) = wallet_file.public_key.as_deref()
        && public_key != hex::encode(&wallet.public_key.bytes)
    {
        return Err("wallet public key does not match its mnemonic and signature scheme".into());
    }
    if let Some(private_key) = wallet_file.private_key.as_deref() {
        let expected_private_key = {
            let seed = wallet.signing_seed.dangerous_export_seed();
            Zeroizing::new(hex::encode(&seed[..]))
        };
        if private_key != expected_private_key.as_str() {
            return Err(
                "wallet private key does not match its mnemonic and signature scheme".into(),
            );
        }
    }
    wallet.mnemonic = Some(wallet_file.mnemonic.clone());
    Ok(wallet)
}

pub fn wallet_file_signature_scheme(bytes: &[u8]) -> Result<Option<Signature>, String> {
    let wallet_file: WalletFile = serde_json::from_slice(bytes)
        .map_err(|error| format!("failed to parse wallet: {error}"))?;
    wallet_file
        .signature_scheme
        .as_deref()
        .map(|scheme| scheme.parse::<Signature>().map_err(str::to_string))
        .transpose()
}

pub fn generate_bip39_mnemonic(words: usize) -> Result<Zeroizing<String>, String> {
    let entropy_len = match words {
        12 => BIP39_MNEMONIC_12_ENTROPY_BYTES,
        24 => BIP39_MNEMONIC_24_ENTROPY_BYTES,
        _ => return Err("mnemonic words must be 12 or 24".to_string()),
    };
    let mut entropy = Zeroizing::new(vec![0_u8; entropy_len]);
    getrandom::fill(&mut entropy)
        .map_err(|error| format!("secure random generation failed: {error}"))?;
    encode_bip39_mnemonic(&entropy).map(Zeroizing::new)
}

pub fn wallet_from_bip39_mnemonic(
    phrase: &str,
    scheme: Signature,
) -> Result<Wallet, String> {
    wallet_from_bip39_mnemonic_for_race(phrase, scheme, novus::crypto::Race::Accretia)
}

pub fn wallet_from_bip39_mnemonic_for_race(
    phrase: &str,
    scheme: Signature,
    race: novus::crypto::Race,
) -> Result<Wallet, String> {
    let entropy = decode_bip39_mnemonic(phrase)?;
    let mut tag = Vec::from(b"XPARQ_WALLET_SIGNATURE_ACCOUNT".as_slice());
    tag.push(scheme as u8);
    let seed = tagged_wallet_hash(&tag, &entropy);
    let mut boxed_seed = Box::new([0_u8; 32]);
    boxed_seed.copy_from_slice(seed.as_ref());
    let signing_seed = SigningSeed::new(scheme, boxed_seed);
    let public_key = signing_seed.public_key();
    Ok(Wallet {
        mnemonic: None,
        address: address_from_public_key_for_race(&public_key, race),
        public_key,
        signing_seed,
    })
}

pub fn encode_bip39_mnemonic(entropy: &[u8]) -> Result<String, String> {
    Mnemonic::from_entropy_in(Language::English, entropy)
        .map(|mnemonic| mnemonic.to_string())
        .map_err(|error| format!("failed to encode mnemonic: {error}"))
}

pub fn decode_bip39_mnemonic(phrase: &str) -> Result<Zeroizing<Vec<u8>>, String> {
    let normalized = Zeroizing::new(
        phrase
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>()
            .join(" "),
    );
    let word_count = normalized.split_whitespace().count();
    if !matches!(word_count, 12 | 24) {
        return Err("invalid bip39 mnemonic: expected 12 or 24 words".to_string());
    }
    Mnemonic::parse_in_normalized(Language::English, &normalized)
        .map(|mnemonic| Zeroizing::new(mnemonic.to_entropy()))
        .map_err(|error| format!("invalid bip39 mnemonic: {error}"))
}

fn tagged_wallet_hash(tag: &[u8], bytes: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut payload = Zeroizing::new(Vec::with_capacity(tag.len() + bytes.len()));
    payload.extend_from_slice(tag);
    payload.extend_from_slice(bytes);
    Zeroizing::new(hash_bytes(&payload).0)
}

impl Wallet {
    pub const fn scheme(&self) -> Signature {
        self.signing_seed.scheme()
    }

    pub fn sign_intent(&self, intent: AccountIntent) -> Result<Transaction, String> {
        let chain = novus::genesis::chain_context().map_err(|error| error.to_string())?;
        let commitment = intent
            .authorization_commitment(chain)
            .map_err(|error| error.to_string())?;
        let signature = self.signing_seed.sign(commitment.as_bytes());
        let authorization = TransactionAuthorization {
            public_key: self.public_key.clone(),
            signature,
        };
        Ok(Transaction { intent, authorization })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /* Legacy wallet tests removed with the account-ledger migration.
    #[test]
    fn wallet_file_roundtrip_preserves_signing_identity() {
        let mnemonic = encode_bip39_mnemonic(&[7; BIP39_MNEMONIC_12_ENTROPY_BYTES]).unwrap();
        let mut wallet = wallet_from_bip39_mnemonic(&mnemonic).unwrap();
        wallet.mnemonic = Some(mnemonic.clone());
        let encoded = wallet_file_bytes(&wallet).unwrap();
        let decoded = wallet_from_file_bytes(&encoded).unwrap();

        assert_eq!(decoded.address, wallet.address);
        assert_eq!(decoded.public_key, wallet.public_key);

        let json: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(json.as_object().unwrap().len(), 2);
        assert_eq!(json.get("mnemonic").unwrap(), &mnemonic);
        assert_eq!(
            json.get("address").unwrap().as_str(),
            Some(wallet_address_string(&wallet).as_str())
        );
        assert!(json.get("secret_key").is_none());
        assert!(encoded.len() < 512);
    }

    #[test]
    fn wallet_address_reader_accepts_legacy_version_field() {
        let mnemonic = encode_bip39_mnemonic(&[8; BIP39_MNEMONIC_12_ENTROPY_BYTES]).unwrap();
        let wallet = wallet_from_bip39_mnemonic(&mnemonic).unwrap();
        let encoded = serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "address": wallet_address_string(&wallet),
            "mnemonic": mnemonic,
        }))
        .unwrap();

        assert_eq!(wallet_address_from_file_bytes(&encoded), Ok(wallet.address));
    }

    #[test]
    fn mnemonic_restore_preserves_signing_identity() {
        let mnemonic = encode_bip39_mnemonic(&[9; BIP39_MNEMONIC_12_ENTROPY_BYTES]).unwrap();
        let mut first = wallet_from_bip39_mnemonic(&mnemonic).unwrap();
        first.mnemonic = Some(mnemonic.clone());
        let first_file = wallet_file_bytes(&first).unwrap();

        let mut restored = wallet_from_bip39_mnemonic(&mnemonic).unwrap();
        restored.mnemonic = Some(mnemonic);
        let restored_file = wallet_file_bytes(&restored).unwrap();

        assert_eq!(first.address, restored.address);
        assert_eq!(first.public_key, restored.public_key);
        assert_eq!(
            wallet_from_file_bytes(&first_file).unwrap().address,
            wallet_from_file_bytes(&restored_file).unwrap().address
        );
    }

    */
    #[test]
    fn mnemonic_derives_distinct_recoverable_nakama_addresses() {
        let mnemonic = encode_bip39_mnemonic(&[12; BIP39_MNEMONIC_12_ENTROPY_BYTES]).unwrap();
        let nakamas = [Signature::Schnorr];
        let first =
            nakamas.map(|nakama| wallet_from_bip39_mnemonic(&mnemonic, nakama).unwrap());
        let second =
            nakamas.map(|nakama| wallet_from_bip39_mnemonic(&mnemonic, nakama).unwrap());
        for (left, right) in first.iter().zip(&second) {
            assert_eq!(left.address, right.address);
        }
        let unique = first
            .iter()
            .map(|wallet| wallet.address)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), nakamas.len());
    }

    #[test]
    fn nakama_wallet_file_roundtrip_preserves_nakama_and_identity() {
        let mnemonic = encode_bip39_mnemonic(&[13; BIP39_MNEMONIC_12_ENTROPY_BYTES]).unwrap();
        for nakama in [Signature::Schnorr] {
            let mut wallet = wallet_from_bip39_mnemonic(&mnemonic, nakama).unwrap();
            wallet.mnemonic = Some(mnemonic.clone());
            let bytes = wallet_file_bytes(&wallet).unwrap();
            let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(json["public_key"], hex::encode(&wallet.public_key.bytes));
            let expected_private_key = {
                let seed = wallet.signing_seed.dangerous_export_seed();
                Zeroizing::new(hex::encode(&seed[..]))
            };
            assert_eq!(
                json["private_key"].as_str(),
                Some(expected_private_key.as_str())
            );
            assert_eq!(wallet_file_signature_scheme(&bytes).unwrap(), Some(nakama));
            let restored = wallet_from_file_bytes(&bytes).unwrap();
            assert_eq!(restored.scheme(), nakama);
            assert_eq!(restored.address, wallet.address);
            assert_eq!(restored.public_key, wallet.public_key);
        }
    }

    #[test]
    fn nakama_wallet_file_rejects_keys_that_do_not_match_recovery_material() {
        let mnemonic = encode_bip39_mnemonic(&[14; BIP39_MNEMONIC_12_ENTROPY_BYTES]).unwrap();
        let mut wallet = wallet_from_bip39_mnemonic(&mnemonic, Signature::Schnorr).unwrap();
        wallet.mnemonic = Some(mnemonic);
        let bytes = wallet_file_bytes(&wallet).unwrap();
        let mut json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        json["public_key"] = serde_json::Value::String("00".repeat(wallet.public_key.bytes.len()));
        let tampered_public = serde_json::to_vec(&json).unwrap();
        assert!(
            wallet_from_file_bytes(&tampered_public)
                .unwrap_err()
                .contains("public key does not match")
        );

        json["public_key"] = serde_json::Value::String(hex::encode(&wallet.public_key.bytes));
        json["private_key"] = serde_json::Value::String("00".repeat(32));
        let tampered_private = serde_json::to_vec(&json).unwrap();
        assert!(
            wallet_from_file_bytes(&tampered_private)
                .unwrap_err()
                .contains("private key does not match")
        );
    }
}
