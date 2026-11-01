use serde::{Deserialize, Serialize};

use crate::{HASH_SIZE, HashDomain, PublicKey, error::CryptoError, hash};
use borsh::{BorshDeserialize, BorshSerialize};

pub const ADDRESS_SIZE: usize = 20;
pub const ADDRESS_CHECKSUM_SIZE: usize = 4;

pub const ACCRETIA_ADDRESS_PREFIX: &str = "accretia";
pub const BELLATO_ADDRESS_PREFIX: &str = "bellato";
pub const CORA_ADDRESS_PREFIX: &str = "cora";
pub const ADDRESS_PREFIXES: [&str; 3] = ["accretia", "bellato", "cora"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize)]
pub enum Race {
    Accretia,
    Bellato,
    Cora,
}

impl Race {
    pub const fn code(self) -> u8 {
        match self { Self::Accretia => 0, Self::Bellato => 1, Self::Cora => 2 }
    }

    pub const fn from_code(code: u8) -> Option<Self> {
        match code { 0 => Some(Self::Accretia), 1 => Some(Self::Bellato), 2 => Some(Self::Cora), _ => None }
    }

    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Accretia => ACCRETIA_ADDRESS_PREFIX,
            Self::Bellato => BELLATO_ADDRESS_PREFIX,
            Self::Cora => CORA_ADDRESS_PREFIX,
        }
    }
}

const ADDRESS_PAYLOAD_SIZE: usize = ADDRESS_SIZE + ADDRESS_CHECKSUM_SIZE;

/// 24 bytes require at most 33 Base62 characters.
pub const ADDRESS_ENCODED_SIZE: usize = 33;

pub const ADDRESS_STRING_LEN: usize = ACCRETIA_ADDRESS_PREFIX.len() + ADDRESS_ENCODED_SIZE;

/// Full Base62 alphabet:
/// 0-9 A-Z a-z
const CHARACTER: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

const TOTAL_CHARACTER: u16 = CHARACTER.len() as u16;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    BorshSerialize,
    BorshDeserialize,
)]
pub struct Address(pub [u8; ADDRESS_SIZE]);

impl Address {
    pub const ZERO: Self = Self([0; ADDRESS_SIZE]);

    pub const fn new(bytes: [u8; ADDRESS_SIZE]) -> Self {
        Self(bytes)
    }

    pub const fn from_bytes(bytes: [u8; ADDRESS_SIZE]) -> Self {
        Self::new(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; ADDRESS_SIZE] {
        &self.0
    }

    pub const fn into_bytes(self) -> [u8; ADDRESS_SIZE] {
        self.0
    }
}

pub fn address_from_public_key(public_key: &PublicKey) -> Address {
    address_from_public_key_for_race(public_key, Race::Accretia)
}

pub fn address_from_public_key_for_race(public_key: &PublicKey, race: Race) -> Address {
    let mut material = Vec::with_capacity(1 + public_key.bytes.len());
    material.extend_from_slice(b"BELLY address v2");
    material.push(race.code());
    material.push(public_key.scheme as u8);
    material.extend_from_slice(&public_key.bytes);
    let mut address = address_from_key_material(&material);
    address.0[0] = (address.0[0] & 0x3f) | (race.code() << 6);
    address
}

pub fn address_matches_public_key(address: &Address, public_key: &PublicKey) -> bool {
    address_matches_public_key_for_race(address, public_key, address.race())
}

pub fn address_matches_public_key_for_race(address: &Address, public_key: &PublicKey, race: Race) -> bool {
    if address.race() != race { return false; }
    let mut material = Vec::with_capacity(1 + public_key.bytes.len());
    material.extend_from_slice(b"BELLY address v2");
    material.push(race.code());
    material.push(public_key.scheme as u8);
    material.extend_from_slice(&public_key.bytes);
    let mut expected = address_from_key_material(&material);
    expected.0[0] = (expected.0[0] & 0x3f) | (race.code() << 6);
    expected == *address
}

impl Address {
    pub const fn has_valid_race_code(&self) -> bool {
        self.0[0] >> 6 <= 2
    }

    pub const fn race(&self) -> Race {
        match self.0[0] >> 6 {
            0 => Race::Accretia,
            1 => Race::Bellato,
            _ => Race::Cora,
        }
    }
}

fn address_from_key_material(key_material: &[u8]) -> Address {
    let digest = hash::domain(HashDomain::Address, key_material);

    let mut address = [0_u8; ADDRESS_SIZE];

    address.copy_from_slice(&digest.as_bytes()[HASH_SIZE - ADDRESS_SIZE..]);

    Address(address)
}

pub fn address_to_string(address: &Address, prefix: &str) -> String {
    assert!(
        ADDRESS_PREFIXES.contains(&prefix),
        "unsupported address prefix"
    );
    assert_eq!(address.race().prefix(), prefix, "address prefix does not match embedded race");
    let checksum = address_checksum(address);


    let mut payload = [0_u8; ADDRESS_PAYLOAD_SIZE];

    payload[..ADDRESS_SIZE].copy_from_slice(address.as_bytes());

    payload[ADDRESS_SIZE..].copy_from_slice(&checksum);

    let encoded = belly_encode(&payload);

    let mut result = String::with_capacity(prefix.len() + ADDRESS_ENCODED_SIZE);

    result.push_str(prefix);
    result.push_str(&encoded);

    result
}

pub fn address_from_string(value: &str, prefix: &str) -> Result<Address, CryptoError> {
    if !ADDRESS_PREFIXES.contains(&prefix) {
        return Err(CryptoError::InvalidAddressEncoding);
    }
    let encoded = value
        .strip_prefix(prefix)
        .ok_or(CryptoError::InvalidAddressEncoding)?;
    if encoded.len() != ADDRESS_ENCODED_SIZE || value.len() != prefix.len() + ADDRESS_ENCODED_SIZE {
        return Err(CryptoError::InvalidAddressEncoding);
    }
    let payload = belly_decode(encoded)?;

    let mut address_bytes = [0_u8; ADDRESS_SIZE];

    address_bytes.copy_from_slice(&payload[..ADDRESS_SIZE]);

    let address = Address::from_bytes(address_bytes);

    let mut checksum = [0_u8; ADDRESS_CHECKSUM_SIZE];

    checksum.copy_from_slice(&payload[ADDRESS_SIZE..]);

    if checksum != address_checksum(&address) || address.race().prefix() != prefix {
        return Err(CryptoError::InvalidAddressEncoding);
    }

    // Enforce canonical encoding.
    if value != address_to_string(&address, prefix) {
        return Err(CryptoError::InvalidAddressEncoding);
    }

    Ok(address)
}

pub fn address_from_string_for_race(value: &str) -> Result<Address, CryptoError> {
    for prefix in ADDRESS_PREFIXES {
        if value.starts_with(prefix) {
            return address_from_string(value, prefix);
        }
    }
    Err(CryptoError::InvalidAddressEncoding)
}

fn address_checksum(address: &Address) -> [u8; ADDRESS_CHECKSUM_SIZE] {
    let digest = hash::domain(HashDomain::AddressChecksum, address.as_bytes());

    let mut checksum = [0_u8; ADDRESS_CHECKSUM_SIZE];

    checksum.copy_from_slice(&digest.as_bytes()[..ADDRESS_CHECKSUM_SIZE]);

    checksum
}

fn belly_encode(payload: &[u8; ADDRESS_PAYLOAD_SIZE]) -> String {
    let mut number = *payload;

    let mut encoded = [CHARACTER[0]; ADDRESS_ENCODED_SIZE];

    for position in (0..ADDRESS_ENCODED_SIZE).rev() {
        let mut remainder = 0_u16;

        for byte in &mut number {
            let value = (remainder << 8) | (*byte as u16);

            *byte = (value / TOTAL_CHARACTER) as u8;

            remainder = value % TOTAL_CHARACTER;
        }

        encoded[position] = CHARACTER[remainder as usize];
    }

    debug_assert!(number.iter().all(|byte| *byte == 0));

    String::from_utf8(encoded.to_vec()).expect("BELLY Base62 alphabet must be valid ASCII")
}

fn belly_decode(encoded: &str) -> Result<[u8; ADDRESS_PAYLOAD_SIZE], CryptoError> {
    if encoded.len() != ADDRESS_ENCODED_SIZE {
        return Err(CryptoError::InvalidAddressEncoding);
    }

    let mut payload = [0_u8; ADDRESS_PAYLOAD_SIZE];

    for character in encoded.bytes() {
        let digit = belly_digit(character).ok_or(CryptoError::InvalidAddressEncoding)?;

        let mut carry = digit as u16;

        // payload = payload * 62 + digit
        for byte in payload.iter_mut().rev() {
            let value = (*byte as u16) * TOTAL_CHARACTER + carry;

            *byte = value as u8;
            carry = value >> 8;
        }

        if carry != 0 {
            return Err(CryptoError::InvalidAddressEncoding);
        }
    }

    Ok(payload)
}

#[inline]
fn belly_digit(character: u8) -> Option<u8> {
    CHARACTER
        .iter()
        .position(|&candidate| candidate == character)
        .map(|index| index as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_roundtrip() {
        let address = Address([7; ADDRESS_SIZE]);

        let encoded = address_to_string(&address, ACCRETIA_ADDRESS_PREFIX);

        assert_eq!(encoded.len(), ADDRESS_STRING_LEN,);

        assert!(encoded.starts_with("accretia"),);

        assert_eq!(
            address_from_string(&encoded, ACCRETIA_ADDRESS_PREFIX),
            Ok(address),
        );
    }

    #[test]
    fn zero_address_roundtrip() {
        let address = Address::ZERO;

        let encoded = address_to_string(&address, ACCRETIA_ADDRESS_PREFIX);

        assert_eq!(encoded.len(), ADDRESS_STRING_LEN,);

        assert!(encoded.starts_with("accretia"),);

        assert_eq!(
            address_from_string(&encoded, ACCRETIA_ADDRESS_PREFIX),
            Ok(address),
        );
    }

    #[test]
    fn base62_preserves_leading_zeroes() {
        let mut payload = [0_u8; ADDRESS_PAYLOAD_SIZE];

        payload[ADDRESS_PAYLOAD_SIZE - 1] = 1;

        let encoded = belly_encode(&payload);

        assert_eq!(encoded.len(), ADDRESS_ENCODED_SIZE,);

        assert_eq!(belly_decode(&encoded).unwrap(), payload,);
    }

    #[test]
    fn maximum_payload_roundtrip() {
        let payload = [0xff_u8; ADDRESS_PAYLOAD_SIZE];

        let encoded = belly_encode(&payload);

        assert_eq!(encoded.len(), ADDRESS_ENCODED_SIZE,);

        assert_eq!(belly_decode(&encoded).unwrap(), payload,);
    }

    #[test]
    fn incorrect_checksum_is_rejected() {
        let address = Address([7; ADDRESS_SIZE]);

        let encoded = address_to_string(&address, ACCRETIA_ADDRESS_PREFIX);

        let payload_string = &encoded[ACCRETIA_ADDRESS_PREFIX.len()..];

        let mut payload = belly_decode(payload_string).unwrap();

        payload[ADDRESS_SIZE] ^= 0x01;

        let corrupted_payload = belly_encode(&payload);

        let corrupted = format!("{ACCRETIA_ADDRESS_PREFIX}{corrupted_payload}");

        assert_eq!(
            address_from_string(&corrupted, ACCRETIA_ADDRESS_PREFIX),
            Err(CryptoError::InvalidAddressEncoding,),
        );
    }

    #[test]
    fn incorrect_prefix_is_rejected() {
        let address = Address([7; ADDRESS_SIZE]);

        let encoded = address_to_string(&address, ACCRETIA_ADDRESS_PREFIX);

        let invalid = format!("X{}", &encoded[1..]);

        assert_eq!(
            address_from_string(&invalid, ACCRETIA_ADDRESS_PREFIX),
            Err(CryptoError::InvalidAddressEncoding,),
        );
    }

    #[test]
    fn alphabet_has_expected_size() {
        assert_eq!(CHARACTER.len(), 62);
    }

    #[test]
    fn address_length_is_expected() {
        assert_eq!(ADDRESS_SIZE, 20);
        assert_eq!(ADDRESS_CHECKSUM_SIZE, 4);
        assert_eq!(ADDRESS_ENCODED_SIZE, 33);
        assert_eq!(ADDRESS_STRING_LEN, 41);
    }

    #[test]
    fn all_faction_prefixes_round_trip() {
        let address = Address([0x5a; ADDRESS_SIZE]);
        for prefix in ADDRESS_PREFIXES {
            let encoded = address_to_string(&address, prefix);
            assert!(encoded.starts_with(prefix));
            assert_eq!(address_from_string(&encoded, prefix), Ok(address));
        }
    }

    #[test]
    fn runtime_prefix_must_match() {
        let address = Address([0x5a; ADDRESS_SIZE]);
        let encoded = address_to_string(&address, BELLATO_ADDRESS_PREFIX);
        assert_eq!(
            address_from_string(&encoded, CORA_ADDRESS_PREFIX),
            Err(CryptoError::InvalidAddressEncoding)
        );
    }
}
