//! Nudge core engine.
//!
//! Everything platform apps need lives behind this crate's public API:
//! networking, crypto, discovery and file handling. No UI code here.

pub mod error;
pub mod identity;
pub mod protocol;
pub mod tls;
pub mod transfer;

pub use error::{Error, Result};
pub use identity::{Fingerprint, Identity, TrustedPeers};
pub use tls::Tls;
pub use transfer::{ReceivedFile, TransferStats, receive_file, send_file};

/// Protocol version spoken by this build. Bumped on any wire-format change.
pub const PROTOCOL_VERSION: u16 = 1;

/// Crate version, for logs and the `--version` output of tools.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// TCP port a receiver listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 47_800;
