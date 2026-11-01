//! Canonical novus block and chain representation.
//!
//! This module owns block encoding, block-local structural validation, Merkle
//! commitments, and the canonical linear chain container. State-dependent
//! Direct transaction validation belongs to `consensus` + `ledger`.

pub mod block;
pub mod chain;
pub mod merkle;

pub use {
    crate::error::{BlockError, ChainError, CodecError},
    block::*,
    chain::Chain,
    merkle::{MerkleHash, MerkleInclusionProof},
};
