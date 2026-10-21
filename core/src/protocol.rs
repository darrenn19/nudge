//! Wire messages. See `docs/PROTOCOL.md` for the full description.
//!
//! Every connection opens with a header (magic, version, request kind),
//! followed by either a file `Offer` or a `PairRequest`.
//!
//! Flow for one file:
//! 1. Sender → `Offer` (header, file name, size, modified time)
//! 2. Receiver → `Response::Accept { offset }` (or `Reject`)
//! 3. Sender → file bytes from `offset` to the end, streamed with no acks;
//!    each `CHUNK_SIZE` chunk is followed by its 32-byte BLAKE3 hash
//! 4. Receiver → `Response::Done` once the file is safely on disk
//!
//! Flow for pairing:
//! 1. Scanner → `PairRequest` (header, proof of the QR secret, device name)
//! 2. Code owner → `PairReply` (accept with its device name, or reject)
//!
//! All integers are big-endian.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{Error, PROTOCOL_VERSION, Result};

/// First bytes of every connection, so stray traffic fails fast.
pub const MAGIC: [u8; 4] = *b"NUDG";

/// Length of a BLAKE3 chunk hash on the wire.
pub const HASH_LEN: usize = 32;

/// Longest file name accepted on the wire, in bytes.
pub const MAX_NAME_LEN: usize = 1024;

/// Longest device name accepted on the wire, in bytes.
pub const MAX_DEVICE_NAME_LEN: usize = 255;

const KIND_FILE: u8 = 1;
const KIND_PAIR: u8 = 2;

/// Header that opens every connection.
fn put_header(buf: &mut Vec<u8>, kind: u8) {
    buf.extend_from_slice(&MAGIC);
    buf.extend_from_slice(&PROTOCOL_VERSION.to_be_bytes());
    buf.push(kind);
}

/// Read and check the header; returns the request kind.
async fn read_header<R: AsyncRead + Unpin>(r: &mut R) -> Result<u8> {
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
    Ok(r.read_u8().await?)
}

/// u16 length + UTF-8 bytes. Callers keep `s` under 64 KiB.
fn put_string(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(&(s.len() as u16).to_be_bytes());
    buf.extend_from_slice(s.as_bytes());
}

async fn read_string<R: AsyncRead + Unpin>(
    r: &mut R,
    max: usize,
    too_long: &'static str,
) -> Result<String> {
    let len = r.read_u16().await? as usize;
    if len > max {
        return Err(Error::Protocol(too_long));
    }
    let mut bytes = vec![0u8; len];
    r.read_exact(&mut bytes).await?;
    String::from_utf8(bytes).map_err(|_| Error::Protocol("text is not UTF-8"))
}

/// Longest prefix of `s` that fits in `max` bytes without splitting a character.
pub(crate) fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// What the connecting side wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    File(Offer),
    Pair(PairRequest),
}

impl Request {
    pub async fn read_from<R: AsyncRead + Unpin>(r: &mut R) -> Result<Self> {
        match read_header(r).await? {
            KIND_FILE => Ok(Self::File(Offer::read_body(r).await?)),
            KIND_PAIR => Ok(Self::Pair(PairRequest::read_body(r).await?)),
            _ => Err(Error::Protocol("unknown request kind")),
        }
    }
}

/// Sender's description of the file it wants to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub name: String,
    pub size: u64,
    /// Modification time in ns since the Unix epoch; 0 = unknown.
    /// Used with name and size to decide whether a transfer can resume.
    pub modified: u64,
}

impl Offer {
    pub async fn write_to<W: AsyncWrite + Unpin>(&self, w: &mut W) -> Result<()> {
        if self.name.len() > MAX_NAME_LEN {
            return Err(Error::BadFileName(self.name.clone()));
        }
        let mut buf = Vec::with_capacity(7 + 2 + self.name.len() + 8 + 8);
        put_header(&mut buf, KIND_FILE);
        put_string(&mut buf, &self.name);
        buf.extend_from_slice(&self.size.to_be_bytes());
        buf.extend_from_slice(&self.modified.to_be_bytes());
        w.write_all(&buf).await?;
        w.flush().await?;
        Ok(())
    }

    /// Read a whole file offer, header included.
    pub async fn read_from<R: AsyncRead + Unpin>(r: &mut R) -> Result<Self> {
        match read_header(r).await? {
            KIND_FILE => Self::read_body(r).await,
            _ => Err(Error::Protocol("expected a file offer")),
        }
    }

    async fn read_body<R: AsyncRead + Unpin>(r: &mut R) -> Result<Self> {
        let name = read_string(r, MAX_NAME_LEN, "file name too long").await?;
        let size = r.read_u64().await?;
        let modified = r.read_u64().await?;
        Ok(Self {
            name,
            size,
            modified,
        })
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

/// Sent by a device that scanned a pairing QR code. `proof` shows it knows
/// the code's one-time secret; see `pairing::proof`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairRequest {
    pub proof: [u8; 32],
    /// The scanning device's name, for the other side's device list.
    pub device_name: String,
}

impl PairRequest {
    pub async fn write_to<W: AsyncWrite + Unpin>(&self, w: &mut W) -> Result<()> {
        let name = truncate(&self.device_name, MAX_DEVICE_NAME_LEN);
        let mut buf = Vec::with_capacity(7 + 32 + 2 + name.len());
        put_header(&mut buf, KIND_PAIR);
        buf.extend_from_slice(&self.proof);
        put_string(&mut buf, name);
        w.write_all(&buf).await?;
        w.flush().await?;
        Ok(())
    }

    async fn read_body<R: AsyncRead + Unpin>(r: &mut R) -> Result<Self> {
        let mut proof = [0u8; 32];
        r.read_exact(&mut proof).await?;
        let device_name = read_string(r, MAX_DEVICE_NAME_LEN, "device name too long").await?;
        Ok(Self { proof, device_name })
    }
}

/// Answer to a `PairRequest`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairReply {
    Reject,
    /// Paired. Carries the answering device's name.
    Accept {
        device_name: String,
    },
}

impl PairReply {
    pub async fn write_to<W: AsyncWrite + Unpin>(&self, w: &mut W) -> Result<()> {
        let mut buf = Vec::new();
        match self {
            Self::Reject => buf.push(TAG_REJECT),
            Self::Accept { device_name } => {
                buf.push(TAG_ACCEPT);
                put_string(&mut buf, truncate(device_name, MAX_DEVICE_NAME_LEN));
            }
        }
        w.write_all(&buf).await?;
        w.flush().await?;
        Ok(())
    }

    pub async fn read_from<R: AsyncRead + Unpin>(r: &mut R) -> Result<Self> {
        match r.read_u8().await? {
            TAG_REJECT => Ok(Self::Reject),
            TAG_ACCEPT => Ok(Self::Accept {
                device_name: read_string(r, MAX_DEVICE_NAME_LEN, "device name too long").await?,
            }),
            _ => Err(Error::Protocol("unknown pairing reply")),
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
            modified: 1_760_000_000_123_456_789,
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
    async fn requests_dispatch_by_kind() {
        let offer = Offer {
            name: "a.txt".into(),
            size: 5,
            modified: 0,
        };
        let pair = PairRequest {
            proof: [7; 32],
            device_name: "Galaxy S25".into(),
        };
        let mut buf = Vec::new();
        offer.write_to(&mut buf).await.unwrap();
        assert_eq!(
            Request::read_from(&mut buf.as_slice()).await.unwrap(),
            Request::File(offer)
        );
        let mut buf = Vec::new();
        pair.write_to(&mut buf).await.unwrap();
        assert_eq!(
            Request::read_from(&mut buf.as_slice()).await.unwrap(),
            Request::Pair(pair)
        );
        // A pairing request is not a file offer.
        assert!(Offer::read_from(&mut buf.as_slice()).await.is_err());
    }

    #[tokio::test]
    async fn pair_replies_round_trip() {
        for reply in [
            PairReply::Reject,
            PairReply::Accept {
                device_name: "Work laptop".into(),
            },
        ] {
            let mut buf = Vec::new();
            reply.write_to(&mut buf).await.unwrap();
            assert_eq!(
                PairReply::read_from(&mut buf.as_slice()).await.unwrap(),
                reply
            );
        }
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("abc", 10), "abc");
        assert_eq!(truncate("héllo", 2), "h");
        assert_eq!(truncate("héllo", 3), "hé");
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
