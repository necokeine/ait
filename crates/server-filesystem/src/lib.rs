//! Filesystem, Git, Forge and repository provisioning capabilities.
//! Blocking services and adapters; the host owns connections and task scheduling.

pub mod capabilities;
pub mod dispatch;
pub mod local;
pub mod ports;
pub mod protocol;
pub mod rpc;
pub mod service;

/// Connection-owned observers and request integration.
pub mod connection;
