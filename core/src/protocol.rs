//! Wire messages. See `docs/PROTOCOL.md` for the full description.
//!
//! Flow for one file:
//! 1. Sender → `Offer` (magic, version, file name, size)
//! 2. Receiver → `Response::Accept { offset }` (or `Reject`)
//! 3. Sender → raw file bytes from `offset` to the end, streamed with no acks
//! 4. Receiver → `Response::Done` once the file is safely on disk
//!
//! All integers are big-endian.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{Error, PROTOCOL_VERSION, Result};

/// First bytes of every connection, so stray traffic fails fast.
pub const MAGIC: [u8; 4] = *b"NUDG";

/// Longest file name accepted on the wire, in bytes.
pub const MAX_NAME_LEN: usize = 1024;

/// Sender's description of the file it wants to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub name: String,
    pub size: u64,
}

impl Offer {
    pub async fn write_to<W: AsyncWrite + Unpin>(&self, w: &mut W) -> Result<()> {
        let name = self.name.as_bytes();
        if name.len() > MAX_NAME_LEN {
            return Err(Error::BadFileName(self.name.clone()));
        }
        let mut buf = Vec::with_capacity(4 + 2 + 2 + name.len() + 8);
        buf.extend_from_slice(&MAGIC);
        buf.extend_from_slice(&PROTOCOL_VERSION.to_be_bytes());
        buf.extend_from_slice(&(name.len() as u16).to_be_bytes());
        buf.extend_from_slice(name);
        buf.extend_from_slice(&self.size.to_be_bytes());
        w.write_all(&buf).await?;
        w.flush().await?;
        Ok(())
    }

    pub async fn read_from<R: AsyncRead + Unpin>(r: &mut R) -> Result<Self> {
        let mut magic = [0u8; 4];
        r.read_exact(&mut magic).await?;
        if magic != MAGIC {
            return Err(Error::Protocol("not a Nudge connection"));
        }
        let version = r.read_u16().await?;
        if version != PROTOCOL_VERSION {
            return Err(Error::Version {
                peer: version,
                ours: PROTOCOL_VERSION,
            });
        }
        let len = r.read_u16().await? as usize;
        if len > MAX_NAME_LEN {
            return Err(Error::Protocol("file name too long"));
        }
        let mut name = vec![0u8; len];
        r.read_exact(&mut name).await?;
        let name =
            String::from_utf8(name).map_err(|_| Error::Protocol("file name is not UTF-8"))?;
        let size = r.read_u64().await?;
        Ok(Self { name, size })
    }
}

/// Receiver's replies to the sender.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Response {
    Reject,
    /// Start streaming from this byte offset (0 for a fresh transfer).
    Accept {
        offset: u64,
    },
    /// Every byte was written and flushed to disk.
    Done,
}

const TAG_REJECT: u8 = 0;
const TAG_ACCEPT: u8 = 1;
const TAG_DONE: u8 = 2;

impl Response {
    pub async fn write_to<W: AsyncWrite + Unpin>(&self, w: &mut W) -> Result<()> {
        match *self {
            Self::Reject => w.write_u8(TAG_REJECT).await?,
            Self::Accept { offset } => {
                let mut buf = [0u8; 9];
                buf[0] = TAG_ACCEPT;
                buf[1..].copy_from_slice(&offset.to_be_bytes());
                w.write_all(&buf).await?;
            }
            Self::Done => w.write_u8(TAG_DONE).await?,
        }
        w.flush().await?;
        Ok(())
    }

    pub async fn read_from<R: AsyncRead + Unpin>(r: &mut R) -> Result<Self> {
        match r.read_u8().await? {
            TAG_REJECT => Ok(Self::Reject),
            TAG_ACCEPT => Ok(Self::Accept {
                offset: r.read_u64().await?,
            }),
            TAG_DONE => Ok(Self::Done),
            _ => Err(Error::Protocol("unknown response tag")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn offer_round_trips() {
        let offer = Offer {
            name: "holiday 📷.jpg".into(),
            size: 123_456_789_012,
        };
        let mut buf = Vec::new();
        offer.write_to(&mut buf).await.unwrap();
        let back = Offer::read_from(&mut buf.as_slice()).await.unwrap();
        assert_eq!(back, offer);
    }

    #[tokio::test]
    async fn responses_round_trip() {
        for resp in [
            Response::Reject,
            Response::Accept { offset: 42 },
            Response::Done,
        ] {
            let mut buf = Vec::new();
            resp.write_to(&mut buf).await.unwrap();
            assert_eq!(
                Response::read_from(&mut buf.as_slice()).await.unwrap(),
                resp
            );
        }
    }

    #[tokio::test]
    async fn rejects_wrong_magic() {
        let err = Offer::read_from(&mut &b"HTTP/1.1 200 OK"[..])
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Protocol(_)));
    }

    #[tokio::test]
    async fn rejects_other_version() {
        let mut buf = MAGIC.to_vec();
        buf.extend_from_slice(&999u16.to_be_bytes());
        let err = Offer::read_from(&mut buf.as_slice()).await.unwrap_err();
        assert!(matches!(err, Error::Version { peer: 999, .. }));
    }
}
