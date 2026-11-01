use borsh::{BorshDeserialize, BorshSerialize};

use crypto::{
    Address, HASH_SIZE, HashDomain, TransactionSignature, PublicKey, TransactionHash,
    address_matches_public_key_for_race, canonical_bytes, domain, verify,
};

use crate::transaction::{AccountIntent, TransactionEncodingError};
use common::ChainContext;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize,
)]
pub struct AuthorizationCommitment([u8; HASH_SIZE]);

impl AuthorizationCommitment {
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

impl AccountIntent {
    pub fn authorization_commitment(
        &self,
        chain: ChainContext,
    ) -> Result<AuthorizationCommitment, TransactionEncodingError> {
        let bytes = canonical_bytes(&(chain.genesis_hash, self))
            .map_err(|_| TransactionEncodingError::Encoding)?;
        Ok(AuthorizationCommitment::from_bytes(
            domain(HashDomain::AccountIntent, &bytes).into_bytes(),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TransactionAuthorization {
    pub public_key: PublicKey,
    pub signature: TransactionSignature,
}

impl TransactionAuthorization {
    pub fn verify(
        &self,
        sender: Address,
        commitment: &AuthorizationCommitment,
        public_key: &PublicKey,
    ) -> bool {
        if &self.public_key != public_key {
            return false;
        }
        if public_key.scheme() != self.signature.scheme() {
            return false;
        }

        if !public_key.scheme().supported() {
            return false;
        }

        if !address_matches_public_key_for_race(&sender, public_key, sender.race()) {
            return false;
        }

        verify(public_key, commitment.as_bytes(), &self.signature)
    }
}

/// Signed Novus transaction.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Transaction {
    pub intent: AccountIntent,
    pub authorization: TransactionAuthorization,
}

impl Transaction {
    /// Checks the public key embedded in this transaction.
    pub fn verify_authorization(
        &self,
        chain: ChainContext,
    ) -> Result<bool, TransactionEncodingError> {
        let commitment = self.intent.authorization_commitment(chain)?;

        Ok(self.authorization.verify(self.intent.sender, &commitment, &self.authorization.public_key))
    }

    pub fn transaction_id(&self) -> Result<TransactionHash, TransactionEncodingError> {
        let bytes = canonical_bytes(self).map_err(|_| TransactionEncodingError::Encoding)?;

        Ok(TransactionHash(
            domain(HashDomain::Transaction, &bytes).into_bytes(),
        ))
    }

    pub fn id(&self) -> Result<[u8; HASH_SIZE], TransactionEncodingError> {
        Ok(self.transaction_id()?.into_bytes())
    }
}
