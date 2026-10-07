//! Platform-independent logic of UniRay.
//!
//! Nothing here touches the network, the filesystem or Tauri: the crate turns
//! share links and subscription responses into [`model::Server`]s and servers
//! plus settings into an Xray JSON config.

pub mod config;
pub mod error;
pub mod hwid;
pub mod links;
pub mod model;
pub mod routing;
pub mod subscription;

pub use error::{Error, Result};
