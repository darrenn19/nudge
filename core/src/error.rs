//! Error type shared by the whole core.

use crate::identity::Fingerprint;

/// Anything that can go wrong in a transfer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(std::io::Error),

    #[error("TLS error: {0}")]
    Tls(#[from] rustls::Error),

    #[error("the other device ({0}) is not trusted")]
    UntrustedPeer(Fingerprint),

    #[error("the other device doesn't trust this one yet")]
    NotTrustedByPeer,

    #[error("couldn't create device identity: {0}")]
    Identity(String),

    #[error("not a valid device fingerprint: {0:?}")]
    BadFingerprint(String),

    #[error("protocol error: {0}")]
    Protocol(&'static str),

    #[error("peer speaks protocol v{peer}, this build speaks v{ours}")]
    Version { peer: u16, ours: u16 },

    #[error("invalid file name: {0:?}")]
    BadFileName(String),

    #[error("the receiver rejected the transfer")]
    Rejected,

    #[error("data corrupted: chunk {chunk} failed its BLAKE3 check")]
    ChunkMismatch { chunk: u64 },

    #[error("connection closed early: received {got} of {expected} bytes")]
    Truncated { got: u64, expected: u64 },

    #[error("not a valid pairing code: {0}")]
    BadPairingCode(String),

    #[error("the other device didn't accept the pairing code (it may have expired)")]
    PairingRejected,

    #[error("couldn't reach the other device")]
    Unreachable,

    #[error("they declined")]
    Declined,

    /// The user on this device said no to files that were arriving.
    #[error("you declined")]
    YouDeclined,

    #[error("no answer")]
    NoAnswer,

    #[error("the browser page was closed")]
    BrowserGone,

    #[error("couldn't share with browsers: {0}")]
    WebShare(String),

    #[error("device discovery failed: {0}")]
    Discovery(String),
}

impl From<std::io::Error> for Error {
    /// Pairing failures arrive as I/O errors from the TLS stream; surface them
    /// as their own variants.
    fn from(e: std::io::Error) -> Self {
        crate::tls::classify_io_error(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
