//! Everything a Purlay client does that is not drawing: the API, sign-in, the realtime
//! connection and engine, the chat state, the durable outbox and uploads.
//!
//! The crate runs on tokio. The UI talks to it through [`session::Session`] and reads the
//! shared [`store::Store`]; nothing here knows about GPUI.

pub mod api;
pub mod auth;
pub mod clock;
pub mod engine;
mod generated;
pub mod handshake;
pub mod ids;
pub mod outbox;
pub mod prefs;
pub mod realtime;
pub mod session;
pub mod store;

pub use purlay_contract as contract;
pub use purlay_markdown as markdown;
