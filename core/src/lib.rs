//! Nudge core engine.
//!
//! Everything platform apps need lives behind this crate's public API:
//! networking, crypto, discovery and file handling. No UI code here.

pub mod archive;
pub mod bench;
pub mod discovery;
pub mod error;
pub mod identity;
pub mod input;
pub mod link;
pub mod logging;
pub mod node;
pub mod pack;
pub mod pairing;
pub mod protocol;
pub mod realtime;
pub mod resume;
pub mod tls;
pub mod transfer;

pub use error::{Error, Result};
pub use identity::{Fingerprint, Identity, TrustedPeers};
pub use link::LinkKind;
pub use node::{
    Cable, Direction, HotspotInfo, Node, NodeConfig, NodeEvent, Outgoing, Peer, Receiving,
};
pub use tls::Tls;
pub use transfer::{ReceivedFile, TransferStats, receive_file, send_file};

/// Protocol version spoken by this build. Bumped on any wire-format change.
pub const PROTOCOL_VERSION: u16 = 7;

/// Crate version, for logs and the `--version` output of tools.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// TCP port a receiver listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 47_800;
