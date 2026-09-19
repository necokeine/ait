//! Application use cases coordinating domain behavior through ports.

mod control;

pub use control::{LocalControlService, PermissionPolicyLimits};

#[cfg(test)]
extern crate self as ait_application;
#[cfg(test)]
#[path = "../tests/support/native.rs"]
mod native_fixture;
