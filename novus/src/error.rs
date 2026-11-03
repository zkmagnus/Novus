//! Central error definitions for the novus crate.

pub use blockchain_errors::*;

mod blockchain_errors {
    pub use crypto::CodecError;

    use std::{error::Error as StdError, fmt};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum BlockError {
        MissingEmission,
        UnexpectedEmission,
        BlockTooLarge,
        InvalidTransaction,
        DuplicateTransaction,
        InvalidEmission,
        InvalidMerkleRoot,
        InvalidStateRoot,
        InvalidBlockSize,
        Serialization(CodecError),
    }

    impl fmt::Display for BlockError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::MissingEmission => f.write_str("non-genesis block must contain emission"),
                Self::UnexpectedEmission => f.write_str("genesis block must not contain emission"),
                Self::BlockTooLarge => f.write_str("block serialized size exceeds limit"),
                Self::InvalidTransaction => f.write_str("block contains an invalid transaction"),
                Self::DuplicateTransaction => f.write_str("block contains a duplicate transaction"),
                Self::InvalidEmission => f.write_str("block emission is invalid"),
                Self::InvalidMerkleRoot => {
                    f.write_str("block merkle root does not match transactions")
                }
                Self::InvalidStateRoot => f.write_str("block state root does not match ledger"),
                Self::InvalidBlockSize => {
                    f.write_str("block header size does not cover canonical block size")
                }
                Self::Serialization(error) => write!(f, "block encoding failed: {error}"),
            }
        }
    }

    impl StdError for BlockError {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            match self {
                Self::Serialization(error) => Some(error),
                _ => None,
            }
        }
    }

    impl From<CodecError> for BlockError {
        fn from(error: CodecError) -> Self {
            Self::Serialization(error)
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ChainError {
        DuplicateBlock,
        InvalidHeight,
        InvalidParent,
        MissingBody,
        Serialization(CodecError),
    }

    impl fmt::Display for ChainError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::DuplicateBlock => f.write_str("block height already exists"),
                Self::InvalidHeight => f.write_str("block height does not extend chain tip"),
                Self::InvalidParent => f.write_str("block parent does not match chain tip"),
                Self::MissingBody => f.write_str("full block body is not retained"),
                Self::Serialization(error) => write!(f, "block encoding failed: {error}"),
            }
        }
    }

    impl StdError for ChainError {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            match self {
                Self::Serialization(error) => Some(error),
                _ => None,
            }
        }
    }

    impl From<CodecError> for ChainError {
        fn from(error: CodecError) -> Self {
            Self::Serialization(error)
        }
    }
}

pub use consensus_errors::*;

mod consensus_errors {
    use std::{error::Error as StdError, fmt};

    use crate::{
        blockchain::BlockError,
        consensus::{EmissionError, TimestampError},
    };

    #[derive(Debug)]
    pub enum ConsensusError {
        InvalidBlock(BlockError),
        InvalidEmission(EmissionError),
        InvalidDifficulty,
        InvalidTimestamp(TimestampError),
        UnexpectedDifficulty,
        PoWHashFailed,
        InvalidHeight,
        InvalidPreviousHash,
        GenesisRequired,
        WrongGenesis,
        InsufficientPoW,
        Serialization,
    }

    impl fmt::Display for ConsensusError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::InvalidBlock(error) => write!(f, "invalid block: {error}"),
                Self::InvalidEmission(error) => write!(f, "invalid emission: {error}"),
                Self::InvalidDifficulty => f.write_str("difficulty is outside allowed range"),
                Self::InvalidTimestamp(error) => write!(f, "invalid block timestamp: {error}"),
                Self::UnexpectedDifficulty => {
                    f.write_str("block difficulty does not match expected difficulty")
                }
                Self::PoWHashFailed => f.write_str("proof-of-work hash failed"),
                Self::InvalidHeight => f.write_str("block height does not extend tip"),
                Self::InvalidPreviousHash => f.write_str("block previous hash does not match tip"),
                Self::GenesisRequired => {
                    f.write_str("canonical chain must be initialized through validated genesis")
                }
                Self::WrongGenesis => {
                    f.write_str("genesis block does not match configured chain identity")
                }
                Self::InsufficientPoW => {
                    f.write_str("block hash does not satisfy proof-of-work difficulty")
                }
                Self::Serialization => f.write_str("consensus encoding failed"),
            }
        }
    }

    impl StdError for ConsensusError {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            match self {
                Self::InvalidBlock(error) => Some(error),
                Self::InvalidEmission(error) => Some(error),
                Self::InvalidTimestamp(error) => Some(error),
                _ => None,
            }
        }
    }

    impl From<EmissionError> for ConsensusError {
        fn from(error: EmissionError) -> Self {
            Self::InvalidEmission(error)
        }
    }

    impl From<BlockError> for ConsensusError {
        fn from(error: BlockError) -> Self {
            Self::InvalidBlock(error)
        }
    }
}

pub use ledger_errors::*;

mod ledger_errors {
    //! Ledger state-transition errors.

    use crate::ledger::AccountError;
    use std::{error::Error as StdError, fmt};

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum StateError {
        Account(AccountError),
        InvalidTransaction,
        AmountOverflow,
    }

    impl fmt::Display for StateError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Account(error) => write!(formatter, "account transition failed: {error:?}"),
                Self::InvalidTransaction => {
                    formatter.write_str("invalid transaction state transition")
                }
                Self::AmountOverflow => formatter.write_str("coin amount overflow"),
            }
        }
    }
    impl StdError for StateError {}
    impl From<AccountError> for StateError {
        fn from(error: AccountError) -> Self {
            Self::Account(error)
        }
    }
}

pub use transaction_errors::*;

mod transaction_errors {
    use std::{error::Error as StdError, fmt};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum IntentError {
        EmptyOutputs,
        ZeroAmount,
        MixedTransferCurrencies,
        InvalidConversionAmount,
        InvalidMessage,
        InvalidAssetCall,
        InvalidStakeAction,
        Encoding,
    }

    impl fmt::Display for IntentError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::EmptyOutputs => formatter.write_str("intent has no outputs"),
                Self::ZeroAmount => formatter.write_str("intent contains a zero amount"),
                Self::MixedTransferCurrencies => {
                    formatter.write_str("one transaction cannot transfer multiple currencies")
                }
                Self::InvalidConversionAmount => {
                    formatter.write_str("conversion quote must be nonzero")
                }
                Self::InvalidMessage => formatter.write_str(
                    "spend message must be 1-256 UTF-8 bytes without control characters",
                ),
                Self::InvalidAssetCall => formatter.write_str("asset call is structurally invalid"),
                Self::InvalidStakeAction => formatter.write_str("stake action has incompatible transaction fields"),
                Self::Encoding => formatter.write_str("intent encoding failed"),
            }
        }
    }

    impl StdError for IntentError {}

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum TransactionEncodingError {
        Encoding,
    }

    impl fmt::Display for TransactionEncodingError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Encoding => formatter.write_str("transaction encoding failed"),
            }
        }
    }

    impl StdError for TransactionEncodingError {}
}
