//! Canonical token management messages; registration is an uncorrelated event.

use serde::Deserialize;

/// Methods requiring an installed persistent token store.
pub const CAPABILITIES: &[&str] = &["push.register", "push.unregister.request"];

/// Token registration/revocation payload. Debug is omitted to prevent accidental disclosure.
#[derive(Deserialize)]
pub struct TokenRequest {
    /// Opaque provider token; whitespace is normalized by the lease service.
    pub token: String,
}
