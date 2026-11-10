//! The Purlay wire contract: types, routes and constants generated from the server's
//! contract bundle (see `Contract/contract.lock`), plus the few primitives written by hand.
//!
//! Never write a wire fact by hand: regenerate with `cargo xtask generate`.

mod generated;
mod primitives;

pub use generated::constants;
pub use generated::endpoints;
pub use generated::models;
pub use primitives::*;

/// The contract version this build speaks (`X-Purlay-Client: … contract/<n>`).
pub const CONTRACT_VERSION: i64 = constants::CONTRACT_VERSION;

/// Helpers for `Option<Option<T>>` fields: absent = `None`, `null` = `Some(None)`.
pub mod double_option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<T: Serialize, S: Serializer>(v: &Option<Option<T>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(Some(x)) => x.serialize(s),
            _ => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
        Ok(Some(Option::<T>::deserialize(d)?))
    }
}

#[cfg(test)]
mod tests;
