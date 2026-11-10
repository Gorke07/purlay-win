//! The socket client (`ARCHITECTURE.md` §5.4), ported from the web client's
//! `apps/web/src/realtime/` with the same rules and the same tests.
//!
//! [`connection::Connection`] is sans-IO: it is told what happened (opened, a message, a
//! close, a timer, a session check) and answers with [`connection::Out`] actions. The tokio
//! [`driver`] performs those with a real socket and real timers.

pub mod backoff;
pub mod connection;
pub mod cursors;
pub mod driver;
