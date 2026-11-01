//! Race-specific currency definitions.

pub mod accretia {
    pub mod coin;
}
pub mod bellato {
    pub mod coin;
}
pub mod cora {
    pub mod coin;
}

pub use crate::transaction::{Currency, Race};
