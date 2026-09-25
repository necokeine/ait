//! Agent configuration, durable runtime directory and native Provider session coordination.
//!
//! Pure Agent values remain in `server-domain`; HTTP/WS transports belong to the host. This crate owns the bounded native Provider worker.

pub mod capabilities;
pub mod dispatch;
pub mod local;
pub mod ports;
pub mod protocol;
pub mod rpc;
pub mod service;
pub mod storage;

#[cfg(all(test, unix))]
mod test_support;

/// Provider-owned physical connection observers.
pub mod connection;
