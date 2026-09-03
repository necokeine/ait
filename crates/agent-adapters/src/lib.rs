//! Agent-level adapters for complete external harnesses.
//!
//! Unlike a model provider, an agent harness may own a conversation protocol,
//! execute commands, edit files, request approvals, and emit rich progress
//! events. This crate normalizes those behaviors without giving the adapter
//! ownership of the host application's Message tree or Run lifecycle.

#![allow(missing_docs)]

pub mod codex;
pub mod error;
pub mod protocol;

pub use error::{AdapterError, AdapterErrorKind};
pub use protocol::*;
