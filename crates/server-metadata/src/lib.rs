//! Project/Workspace metadata and automation: wire types, services, storage and local scripts.
//!
//! The host owns transports and task scheduling. This crate has no dependencies on other server crates.

pub mod local;
pub mod model;
pub mod ports;
pub mod protocol;
pub mod rpc;
pub mod service;
pub mod storage;
