use borsh::{BorshDeserialize, BorshSerialize};
use secp256k1::hashes::{Hash as BitcoinHash, sha256};
use serde::de::{Error as DeError, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha3::{Digest, Sha3_256};
use static_assertions::const_assert_eq;
use std::{error::Error, fmt};

pub const HASH_SIZE: usize = 32;
pub const POW_HASH_SIZE: usize = HASH_SIZE;
const_assert_eq!(HASH_SIZE, 32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HashParseError;

impl fmt::Display for HashParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("hash has invalid hexadecimal length or encoding")
    }
}

impl Error for HashParseError {}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize,
)]
pub struct Hash(pub [u8; HASH_SIZE]);

impl Hash {
    pub const ZERO: Self = Self([0; HASH_SIZE]);

    pub const fn from_bytes(bytes: [u8; HASH_SIZE]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; HASH_SIZE] {
        &self.0
    }

    pub const fn into_bytes(self) -> [u8; HASH_SIZE] {
        self.0
    }
}

impl Serialize for Hash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct HashVisitor;

        impl<'de> Visitor<'de> for HashVisitor {
            type Value = Hash;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "{HASH_SIZE} hash bytes")
            }

            fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E>
            where
                E: DeError,
            {
                let bytes: [u8; HASH_SIZE] = value
                    .try_into()
                    .map_err(|_| E::invalid_length(value.len(), &self))?;

                Ok(Hash(bytes))
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut bytes = [0_u8; HASH_SIZE];

                for (index, byte) in bytes.iter_mut().enumerate() {
                    *byte = seq
                        .next_element()?
                        .ok_or_else(|| DeError::invalid_length(index, &self))?;
                }

                Ok(Hash(bytes))
            }
        }

        deserializer.deserialize_bytes(HashVisitor)
    }
}

pub fn format(prefix: &str, hash: &Hash, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter.write_str(prefix)?;

    for byte in hash.as_bytes() {
        write!(formatter, "{byte:02x}")?;
    }

    Ok(())
}

pub fn parse(prefix: &str, value: &str) -> Result<Hash, HashParseError> {
    let encoded = value.strip_prefix(prefix).ok_or(HashParseError)?;

    if encoded.len() != HASH_SIZE * 2 {
        return Err(HashParseError);
    }

    let encoded = encoded.as_bytes();
    let mut bytes = [0_u8; HASH_SIZE];

    for (index, byte) in bytes.iter_mut().enumerate() {
        let offset = index * 2;
        let high = hex_nibble(encoded[offset]).ok_or(HashParseError)?;
        let low = hex_nibble(encoded[offset + 1]).ok_or(HashParseError)?;
        *byte = (high << 4) | low;
    }

    Ok(Hash::from_bytes(bytes))
}

const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

macro_rules! hash_newtype {
    ($name:ident) => {
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
        pub struct $name(pub [u8; HASH_SIZE]);

        impl $name {
            pub const ZERO: Self = Self([0; HASH_SIZE]);

            pub const fn as_hash(self) -> Hash {
                Hash::from_bytes(self.0)
            }

            pub const fn as_bytes(&self) -> &[u8; HASH_SIZE] {
                &self.0
            }

            pub const fn into_bytes(self) -> [u8; HASH_SIZE] {
                self.0
            }
        }

        impl From<Hash> for $name {
            fn from(hash: Hash) -> Self {
                Self(hash.into_bytes())
            }
        }

        impl From<$name> for Hash {
            fn from(hash: $name) -> Self {
                Hash::from_bytes(hash.0)
            }
        }

        impl PartialEq<Hash> for $name {
            fn eq(&self, other: &Hash) -> bool {
                self.0 == other.0
            }
        }

        impl PartialEq<$name> for Hash {
            fn eq(&self, other: &$name) -> bool {
                self.0 == other.0
            }
        }
    };
}

hash_newtype!(BlockHash);
hash_newtype!(TransactionHash);
hash_newtype!(MerkleHash);
hash_newtype!(StateRoot);
hash_newtype!(PreviousHash);
hash_newtype!(PoWHash);

impl From<BlockHash> for PreviousHash {
    fn from(hash: BlockHash) -> Self {
        Self(hash.0)
    }
}

impl PartialEq<BlockHash> for PreviousHash {
    fn eq(&self, other: &BlockHash) -> bool {
        self.0 == other.0
    }
}

impl PartialEq<PreviousHash> for BlockHash {
    fn eq(&self, other: &PreviousHash) -> bool {
        self.0 == other.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashDomain {
    Transaction,
    AccountIntent,
    Header,
    ChainSpec,
    MerkleNode,
    ProtocolState,
    PoWSeed,
    PoWSalt,
    Address,
    AddressChecksum,
    Emission,
    Raw,
}

impl HashDomain {
    fn tag(self) -> &'static [u8] {
        match self {
            HashDomain::Transaction => b"NOVUS_HASH_TX",
            HashDomain::AccountIntent => b"RFONLINE_ACCOUNT_INTENT_V1",
            HashDomain::Header => b"NOVUS_HASH_BLOCK_HEADER",
            HashDomain::ChainSpec => b"NOVUS_HASH_CHAIN_SPEC",
            HashDomain::MerkleNode => b"NOVUS_HASH_MERKLE_NODE",
            HashDomain::ProtocolState => b"NOVUS_HASH_PROTOCOL_STATE",
            HashDomain::PoWSeed => b"NOVUS_POW_SEED",
            HashDomain::PoWSalt => b"NOVUS_POW_SALT",
            HashDomain::Address => b"NOVUS_HASH_ADDRESS",
            HashDomain::AddressChecksum => b"NOVUS_HASH_ADDRESS_CHECKSUM",
            HashDomain::Emission => b"NOVUS_COIN_EMISSION",
            HashDomain::Raw => b"NOVUS_HASH_RAW",
        }
    }
}

pub fn hash_bytes(bytes: &[u8]) -> Hash {
    domain(HashDomain::Raw, bytes)
}

/// Computes Bitcoin-style SHA256d and returns the digest in canonical
/// big-endian order for proof-of-work target comparison.
pub fn bitcoin_double_sha256(bytes: &[u8]) -> PoWHash {
    let first = sha256::Hash::hash(bytes);
    let mut digest = sha256::Hash::hash(first.as_ref()).to_byte_array();
    digest.reverse();
    PoWHash(digest)
}

pub fn domain(domain: HashDomain, bytes: &[u8]) -> Hash {
    let mut hasher = Sha3_256::new();

    hasher.update(domain.tag());
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);

    let digest = hasher.finalize();

    let mut hash = [0_u8; HASH_SIZE];
    hash.copy_from_slice(&digest);

    Hash(hash)
}

pub fn domain_hash(domain: HashDomain, bytes: &[u8]) -> Hash {
    self::domain(domain, bytes)
}
