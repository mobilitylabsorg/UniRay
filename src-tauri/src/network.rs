//! The real network beneath the VPN, as Android reports it.
//!
//! The core resolves the proxy servers over encrypted DNS (DoH) sent straight
//! out the physical interface (see [`uniray_core::model::Settings::direct_dns`]
//! and `config.rs`), so resolution no longer depends on which network the
//! device is on and never trusts the operator's resolver, which may hijack the
//! server's address. All that is left for this module is a diagnostic: naming
//! the underlying network in the log. Kept platform-independent so it is
//! tested everywhere; only the mobile core uses it.
#![cfg_attr(desktop, allow(dead_code))]

use serde::Deserialize;

/// The real network under the VPN, as Android reports it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct NetworkInfo {
    /// `wifi`, `ethernet`, `cellular`, `other` or `none`.
    #[serde(rename = "type")]
    pub kind: String,
}
