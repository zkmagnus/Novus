use std::io::{self, Read, Write};

use borsh::{BorshDeserialize, BorshSerialize};
use secp256k1::{
    Keypair, Message, Secp256k1, SecretKey, XOnlyPublicKey, schnorr::Signature as SchnorrSignature,
};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

pub const SCHNORR_PUBLIC_KEY_SIZE: usize = 32;
pub const SCHNORR_SIGNATURE_SIZE: usize = 64;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize,
)]
#[repr(u8)]
#[borsh(use_discriminant = true)]
pub enum SignatureScheme {
    Schnorr = 3,
}

pub type Signature = SignatureScheme;

impl SignatureScheme {
    pub const fn as_str(self) -> &'static str {
        "schnorr"
    }
    pub const fn supported(self) -> bool {
        true
    }
    pub const fn public_key_size(self) -> usize {
        SCHNORR_PUBLIC_KEY_SIZE
    }
    pub const fn signature_size(self) -> usize {
        SCHNORR_SIGNATURE_SIZE
    }
}

impl std::str::FromStr for SignatureScheme {
    type Err = &'static str;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().replace(['-', '_'], "").as_str() {
            "schnorr" | "bip340" | "schnorrbip340" => Ok(Self::Schnorr),
            _ => Err("unknown signature scheme"),
        }
    }
}

impl std::fmt::Display for SignatureScheme {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicKey {
    pub scheme: SignatureScheme,
    pub bytes: Vec<u8>,
}

impl PublicKey {
    pub const fn scheme(&self) -> SignatureScheme {
        self.scheme
    }
    pub fn is_valid_encoding(&self) -> bool {
        self.bytes.len() == self.scheme().public_key_size()
            && XOnlyPublicKey::from_slice(&self.bytes).is_ok()
    }
}

impl BorshSerialize for PublicKey {
    fn serialize<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        if !self.is_valid_encoding() {
            return Err(invalid_length(
                "public key",
                self.scheme.public_key_size(),
                self.bytes.len(),
            ));
        }
        self.scheme.serialize(writer)?;
        self.bytes.serialize(writer)
    }
}

impl BorshDeserialize for PublicKey {
    fn deserialize_reader<R: Read>(reader: &mut R) -> io::Result<Self> {
        let scheme = SignatureScheme::deserialize_reader(reader)?;
        let length = u32::deserialize_reader(reader)? as usize;
        let expected = scheme.public_key_size();
        if length != expected {
            return Err(invalid_length("public key", expected, length));
        }
        let mut bytes = vec![0; expected];
        reader.read_exact(&mut bytes)?;
        if XOnlyPublicKey::from_slice(&bytes).is_err() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid Schnorr public key",
            ));
        }
        Ok(Self { scheme, bytes })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionSignature {
    pub scheme: SignatureScheme,
    pub bytes: Vec<u8>,
}

impl TransactionSignature {
    pub const fn scheme(&self) -> SignatureScheme {
        self.scheme
    }
    pub fn is_valid_encoding(&self) -> bool {
        self.bytes.len() == self.scheme().signature_size()
            && SchnorrSignature::from_slice(&self.bytes).is_ok()
    }
}

impl BorshSerialize for TransactionSignature {
    fn serialize<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        if !self.is_valid_encoding() {
            return Err(invalid_length(
                "signature",
                self.scheme.signature_size(),
                self.bytes.len(),
            ));
        }
        self.scheme.serialize(writer)?;
        self.bytes.serialize(writer)
    }
}

impl BorshDeserialize for TransactionSignature {
    fn deserialize_reader<R: Read>(reader: &mut R) -> io::Result<Self> {
        let scheme = SignatureScheme::deserialize_reader(reader)?;
        let length = u32::deserialize_reader(reader)? as usize;
        let expected = scheme.signature_size();
        if length != expected {
            return Err(invalid_length("signature", expected, length));
        }
        let mut bytes = vec![0; expected];
        reader.read_exact(&mut bytes)?;
        if SchnorrSignature::from_slice(&bytes).is_err() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid Schnorr signature",
            ));
        }
        Ok(Self { scheme, bytes })
    }
}

pub struct SigningSeed {
    scheme: SignatureScheme,
    seed: Box<[u8; 32]>,
}

impl Drop for SigningSeed {
    fn drop(&mut self) {
        self.seed.as_mut().zeroize();
    }
}
impl ZeroizeOnDrop for SigningSeed {}

impl std::fmt::Debug for SigningSeed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SigningSeed")
            .field("scheme", &self.scheme)
            .field("seed", &"[REDACTED]")
            .finish()
    }
}

impl SigningSeed {
    pub fn new(scheme: SignatureScheme, seed: Box<[u8; 32]>) -> Self {
        Self { scheme, seed }
    }
    pub const fn scheme(&self) -> SignatureScheme {
        self.scheme
    }
    pub fn public_key(&self) -> PublicKey {
        public_key_from_seed(self.scheme, self.seed.as_ref())
    }
    pub fn sign(&self, message: &[u8]) -> TransactionSignature {
        sign_from_seed(self.scheme, self.seed.as_ref(), message)
    }
    pub fn dangerous_export_seed(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(*self.seed)
    }
    pub fn destroy(self) {
        drop(self);
    }
}

pub fn public_key_from_seed(scheme: SignatureScheme, seed: &[u8; 32]) -> PublicKey {
    let secp = Secp256k1::signing_only();
    let secret = SecretKey::from_slice(seed).expect("32-byte Schnorr seed must be a valid scalar");
    let keypair = Keypair::from_secret_key(&secp, &secret);
    let (public, _) = XOnlyPublicKey::from_keypair(&keypair);
    PublicKey {
        scheme,
        bytes: public.serialize().to_vec(),
    }
}

pub fn sign_from_seed(
    scheme: SignatureScheme,
    seed: &[u8; 32],
    message: &[u8],
) -> TransactionSignature {
    let secp = Secp256k1::signing_only();
    let secret = SecretKey::from_slice(seed).expect("32-byte Schnorr seed must be a valid scalar");
    let keypair = Keypair::from_secret_key(&secp, &secret);
    let digest = message_digest(message);
    let message = Message::from_digest(digest);
    let signature = secp.sign_schnorr_no_aux_rand(&message, &keypair);
    TransactionSignature {
        scheme,
        bytes: signature.as_ref().to_vec(),
    }
}

pub fn verify(public_key: &PublicKey, message: &[u8], signature: &TransactionSignature) -> bool {
    if public_key.scheme() != signature.scheme()
        || !public_key.is_valid_encoding()
        || !signature.is_valid_encoding()
    {
        return false;
    }
    let Ok(key) = XOnlyPublicKey::from_slice(&public_key.bytes) else {
        return false;
    };
    let Ok(signature) = SchnorrSignature::from_slice(&signature.bytes) else {
        return false;
    };
    let secp = Secp256k1::verification_only();
    let message = Message::from_digest(message_digest(message));
    secp.verify_schnorr(&signature, &message, &key).is_ok()
}

fn message_digest(message: &[u8]) -> [u8; 32] {
    use sha3::{Digest, Sha3_256};
    let mut hash = Sha3_256::new();
    hash.update(b"BELLY BIP340 message");
    hash.update((message.len() as u64).to_le_bytes());
    hash.update(message);
    hash.finalize().into()
}

fn invalid_length(kind: &str, expected: usize, actual: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("invalid {kind} length: expected {expected}, got {actual}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    const MESSAGE: &[u8] = b"belly schnorr signature test";

    #[test]
    fn bip340_sign_and_verify() {
        let seed = SigningSeed::new(Signature::Schnorr, Box::new([31; 32]));
        let public = seed.public_key();
        let signature = seed.sign(MESSAGE);
        assert_eq!(public.bytes.len(), 32);
        assert_eq!(signature.bytes.len(), 64);
        assert!(verify(&public, MESSAGE, &signature));
        assert!(!verify(&public, b"tampered", &signature));
    }

    #[test]
    fn scheme_names_and_sizes() {
        assert_eq!("bip340".parse::<Signature>().unwrap(), Signature::Schnorr);
        assert_eq!(Signature::Schnorr.public_key_size(), 32);
        assert_eq!(Signature::Schnorr.signature_size(), 64);
        assert!("falcon512".parse::<Signature>().is_err());
    }

    #[test]
    fn deterministic_public_key_and_borsh_round_trip() {
        let first = public_key_from_seed(Signature::Schnorr, &[11; 32]);
        let second = public_key_from_seed(Signature::Schnorr, &[11; 32]);
        assert_eq!(first, second);
        let seed = SigningSeed::new(Signature::Schnorr, Box::new([41; 32]));
        let public = seed.public_key();
        let signature = seed.sign(MESSAGE);
        let decoded_public = PublicKey::try_from_slice(&borsh::to_vec(&public).unwrap()).unwrap();
        let decoded_signature =
            TransactionSignature::try_from_slice(&borsh::to_vec(&signature).unwrap()).unwrap();
        assert_eq!(decoded_public, public);
        assert_eq!(decoded_signature, signature);
        assert!(verify(&decoded_public, MESSAGE, &decoded_signature));
    }
}
