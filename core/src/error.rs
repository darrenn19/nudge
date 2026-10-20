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

    #[error("connection closed early: received {got} of {expected} bytes")]
    Truncated { got: u64, expected: u64 },
}

impl From<std::io::Error> for Error {
    /// Pairing failures arrive as I/O errors from the TLS stream; surface them
    /// as their own variants.
    fn from(e: std::io::Error) -> Self {
        crate::tls::classify_io_error(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
