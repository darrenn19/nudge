//! Wire messages. See `docs/PROTOCOL.md` for the full description.
//!
//! Every connection opens with a header (magic, version, request kind),
//! followed by either a file `Offer` or a `PairRequest`.
//!
//! Flow for one file:
//! 1. Sender → `Offer` (header, file name, size, modified time)
//! 2. Receiver → `Response::Asking` if its user is being asked (the bytes
//!    still go now: "send first, accept later"), then
//!    `Response::Accept { offset }` (or `Reject` / `Declined` / `NoAnswer`)
//! 3. Sender → file bytes from `offset` to the end, streamed with no acks;
//!    each `CHUNK_SIZE` chunk is followed by its 32-byte BLAKE3 hash
//! 4. Receiver → `Response::Done` once the file is saved, or `Held` once it's
//!    safely stored but its user hasn't answered yet (`Declined` / `NoAnswer`
//!    if they said no or never answered)
//!
//! After `Asking`, the sender learns the answer with an `AwaitAnswer`
//! request on a connection of its own: one per send, however many files.
//!
//! A direct link (the phone's hotspot) for the rest of a big transfer:
//! `Link(Ask)` from the device that would join → `LinkReply::Join` with the
//! network's details once the phone's hotspot is up (or `Refused`). Or
//! `Link(Offer)` from the phone → `LinkReply::Ready` (or `Refused`), then the
//! phone starts its hotspot and sends `LinkReply::Join` on the same
//! connection.
//!
//! Clipboard text between paired devices: `Clip` → `Response::Done` (or
//! `Reject` if that device doesn't sync its clipboard with this one). A
//! copied image the same way, with `ClipImage`.
//!
//! Screen mirroring between paired devices: `Mirror(ShowMe)` from the
//! device that wants to see the other's screen, or `Mirror(ShowYou)` from
//! the one offering its own → `MirrorReply`. The shared device's user is
//! always asked (`ShowMe`) or started it themselves (`ShowYou`); a locked
//! device never shares. After `Accepted`, the same connection carries the
//! real-time handshake (`realtime::Session`) and stays open for the session.
//! Builds from before mirroring close the connection on `Mirror`.
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

/// Longest clipboard text accepted on the wire, in bytes.
pub const MAX_CLIP_LEN: usize = 1 << 20;

/// Biggest copied image accepted on the wire, in bytes (a large PNG
/// screenshot is a few MB).
pub const MAX_CLIP_IMAGE_LEN: usize = 48 << 20;

/// Longest image type ("image/png") accepted on the wire, in bytes.
const MAX_MIME_LEN: usize = 64;

const KIND_FILE: u8 = 1;
const KIND_PAIR: u8 = 2;
const KIND_PAIR_ASK: u8 = 3;
const KIND_STATUS: u8 = 4;
const KIND_AWAIT: u8 = 5;
const KIND_CLIP: u8 = 6;
const KIND_LINK: u8 = 7;
/// A file offer whose data is a pack of files (see `pack`).
const KIND_PACK: u8 = 8;
const KIND_CLIP_IMAGE: u8 = 9;
const KIND_CANCEL: u8 = 10;
/// New in a compatible way: older builds of the same version refuse it.
const KIND_MIRROR: u8 = 11;

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
    /// Pair without a code: the other side's user confirms.
    PairAsk(PairAsk),
    /// Swap settings between paired devices.
    Status(Status),
    /// "What did your user say about this send?" Answered once they have.
    AwaitAnswer(AwaitAnswer),
    /// Put this text on your clipboard.
    Clip(Clip),
    /// Put this image on your clipboard.
    ClipImage(ClipImage),
    /// Move to a faster, direct network (the phone's hotspot).
    Link(Link),
    /// The sender's user stopped a send. No reply.
    Cancel(Cancel),
    /// Mirror a screen: answered with a `MirrorReply`.
    Mirror(Mirror),
}

impl Request {
    pub async fn read_from<R: AsyncRead + Unpin>(r: &mut R) -> Result<Self> {
        match read_header(r).await? {
            KIND_FILE => Ok(Self::File(Offer::read_body(r, false).await?)),
            KIND_PACK => Ok(Self::File(Offer::read_body(r, true).await?)),
            KIND_PAIR => Ok(Self::Pair(PairRequest::read_body(r).await?)),
            KIND_PAIR_ASK => Ok(Self::PairAsk(PairAsk::read_body(r).await?)),
            KIND_STATUS => Ok(Self::Status(Status::read_body(r).await?)),
            KIND_AWAIT => Ok(Self::AwaitAnswer(AwaitAnswer {
                batch: r.read_u64().await?,
            })),
            KIND_CLIP => Ok(Self::Clip(Clip::read_body(r).await?)),
            KIND_CLIP_IMAGE => Ok(Self::ClipImage(ClipImage::read_body(r).await?)),
            KIND_CANCEL => Ok(Self::Cancel(Cancel::read_body(r).await?)),
            KIND_LINK => match r.read_u8().await? {
                1 => Ok(Self::Link(Link::Ask)),
                2 => Ok(Self::Link(Link::Offer)),
                3 => Ok(Self::Link(Link::AskDirect)),
                _ => Err(Error::Protocol("unknown link request")),
            },
            KIND_MIRROR => match r.read_u8().await? {
                1 => Ok(Self::Mirror(Mirror::ShowMe)),
                2 => Ok(Self::Mirror(Mirror::ShowYou)),
                _ => Err(Error::Protocol("unknown mirror request")),
            },
            _ => Err(Error::Protocol("unknown request kind")),
        }
    }
}

/// "Can we pair?", from a device that saw this one nearby. No secret: the
/// user on this side confirms, after checking both screens show the same
/// code (see `pairing::compare_code`). Answered with a `PairReply`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairAsk {
    pub device_name: String,
}

impl PairAsk {
    pub async fn write_to<W: AsyncWrite + Unpin>(&self, w: &mut W) -> Result<()> {
        let name = truncate(&self.device_name, MAX_DEVICE_NAME_LEN);
        let mut buf = Vec::with_capacity(7 + 2 + name.len());
        put_header(&mut buf, KIND_PAIR_ASK);
        put_string(&mut buf, name);
        w.write_all(&buf).await?;
        w.flush().await?;
        Ok(())
    }

    async fn read_body<R: AsyncRead + Unpin>(r: &mut R) -> Result<Self> {
        let device_name = read_string(r, MAX_DEVICE_NAME_LEN, "device name too long").await?;
        Ok(Self { device_name })
    }
}

/// Settings one paired device tells another. Sent both ways: the reply is
/// the other side's own `Status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    /// Files from the device receiving this are accepted without asking.
    pub auto_accepts_you: bool,
    /// This phone is plugged into a computer by a cable that carries data,
    /// but isn't sharing a network on it (USB tethering is off), so files
    /// can't use it yet. Only phones say so.
    pub cable_unused: bool,
}

impl Status {
    /// As a request (with the header).
    pub async fn write_to<W: AsyncWrite + Unpin>(&self, w: &mut W) -> Result<()> {
        let mut buf = Vec::with_capacity(8);
        put_header(&mut buf, KIND_STATUS);
        buf.push(self.flags());
        w.write_all(&buf).await?;
        w.flush().await?;
        Ok(())
    }

    /// As the reply (no header).
    pub async fn write_reply<W: AsyncWrite + Unpin>(&self, w: &mut W) -> Result<()> {
        w.write_u8(self.flags()).await?;
        w.flush().await?;
        Ok(())
    }

    /// One byte: bit 0 `auto_accepts_you`, bit 1 `cable_unused`.
    fn flags(&self) -> u8 {
        u8::from(self.auto_accepts_you) | (u8::from(self.cable_unused) << 1)
    }

    pub async fn read_reply<R: AsyncRead + Unpin>(r: &mut R) -> Result<Self> {
        let flags = r.read_u8().await?;
        Ok(Self {
            auto_accepts_you: flags & 1 != 0,
            cable_unused: flags & 2 != 0,
        })
    }

    async fn read_body<R: AsyncRead + Unpin>(r: &mut R) -> Result<Self> {
        Self::read_reply(r).await
    }
}

/// Asks for the answer to a send (by batch ID) that the receiver is asking
/// its user about. Replied to with `Response::Approved`, `Declined` or
/// `NoAnswer`, once there is one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AwaitAnswer {
    pub batch: u64,
}

impl AwaitAnswer {
    pub async fn write_to<W: AsyncWrite + Unpin>(&self, w: &mut W) -> Result<()> {
        let mut buf = Vec::with_capacity(15);
        put_header(&mut buf, KIND_AWAIT);
        buf.extend_from_slice(&self.batch.to_be_bytes());
        w.write_all(&buf).await?;
        w.flush().await?;
        Ok(())
    }
}

/// "My user stopped sending this": the file on its way (or about to be),
/// and the rest of its batch. The receiver deletes what arrived and closes
/// any question about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cancel {
    /// The send's batch ID (0: a file sent on its own).
    pub batch: u64,
    /// The file being sent when it stopped, as offered.
    pub name: String,
    pub size: u64,
}

impl Cancel {
    pub async fn write_to<W: AsyncWrite + Unpin>(&self, w: &mut W) -> Result<()> {
        if self.name.len() > MAX_NAME_LEN {
            return Err(Error::BadFileName(self.name.clone()));
        }
        let mut buf = Vec::with_capacity(7 + 8 + 2 + self.name.len() + 8);
        put_header(&mut buf, KIND_CANCEL);
        buf.extend_from_slice(&self.batch.to_be_bytes());
        put_string(&mut buf, &self.name);
        buf.extend_from_slice(&self.size.to_be_bytes());
        w.write_all(&buf).await?;
        w.flush().await?;
        Ok(())
    }

    async fn read_body<R: AsyncRead + Unpin>(r: &mut R) -> Result<Self> {
        let batch = r.read_u64().await?;
        let name = read_string(r, MAX_NAME_LEN, "file name too long").await?;
        let size = r.read_u64().await?;
        Ok(Self { batch, name, size })
    }
}

/// A direct link request; see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// "Start your hotspot and tell me how to join it."
    Ask,
    /// "I'll start my hotspot: will you join it?"
    Offer,
    /// Like `Ask`, for screen mirroring: only a Wi-Fi Direct network on the
    /// phone's own channel will do. A slower hotspot (often 2.4 GHz, or a
    /// second channel the radio hops to) is worse than the router for live
    /// video. Builds from before it refuse it.
    AskDirect,
}

impl Link {
    pub async fn write_to<W: AsyncWrite + Unpin>(&self, w: &mut W) -> Result<()> {
        let mut buf = Vec::with_capacity(8);
        put_header(&mut buf, KIND_LINK);
        buf.push(match self {
            Self::Ask => 1,
            Self::Offer => 2,
            Self::AskDirect => 3,
        });
        w.write_all(&buf).await?;
        w.flush().await?;
        Ok(())
    }
}

/// A screen-mirroring request; see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mirror {
    /// "Show me your screen." Your user is asked.
    ShowMe,
    /// "My user wants to show you my screen."
    ShowYou,
}

impl Mirror {
    pub async fn write_to<W: AsyncWrite + Unpin>(&self, w: &mut W) -> Result<()> {
        let mut buf = Vec::with_capacity(8);
        put_header(&mut buf, KIND_MIRROR);
        buf.push(match self {
            Self::ShowMe => 1,
            Self::ShowYou => 2,
        });
        w.write_all(&buf).await?;
        w.flush().await?;
        Ok(())
    }
}

/// Answers to a `Mirror` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirrorReply {
    /// The real-time handshake follows on this connection.
    Accepted,
    /// The user said no.
    Declined,
    /// Nobody answered in time.
    NoAnswer,
    /// The device that would share its screen is locked.
    Locked,
    /// A question about mirroring from this device is already open.
    Busy,
    /// Not an answer yet: the device is locked, and shares once it's
    /// unlocked. The real reply follows on the same connection.
    WaitingForUnlock,
}

impl MirrorReply {
    pub async fn write_to<W: AsyncWrite + Unpin>(&self, w: &mut W) -> Result<()> {
        w.write_u8(match self {
            Self::Declined => 0,
            Self::Accepted => 1,
            Self::NoAnswer => 2,
            Self::Locked => 3,
            Self::Busy => 4,
            Self::WaitingForUnlock => 5,
        })
        .await?;
        w.flush().await?;
        Ok(())
    }

    pub async fn read_from<R: AsyncRead + Unpin>(r: &mut R) -> Result<Self> {
        match r.read_u8().await? {
            0 => Ok(Self::Declined),
            1 => Ok(Self::Accepted),
            2 => Ok(Self::NoAnswer),
            3 => Ok(Self::Locked),
            4 => Ok(Self::Busy),
            5 => Ok(Self::WaitingForUnlock),
            _ => Err(Error::Protocol("unknown mirror reply")),
        }
    }
}

/// How to join a phone's hotspot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hotspot {
    pub ssid: String,
    pub passphrase: String,
    /// WPA3 only (no WPA2 fallback).
    pub wpa3: bool,
    /// The phone's addresses on it.
    pub addrs: Vec<std::net::Ipv4Addr>,
}

/// Answers to a `Link` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkReply {
    Refused,
    /// Willing to join (the reply to `Offer`); `Join` follows.
    Ready,
    Join(Hotspot),
}

impl LinkReply {
    pub async fn write_to<W: AsyncWrite + Unpin>(&self, w: &mut W) -> Result<()> {
        let mut buf = Vec::new();
        match self {
            Self::Refused => buf.push(0),
            Self::Ready => buf.push(1),
            Self::Join(h) => {
                buf.push(2);
                put_string(&mut buf, truncate(&h.ssid, 64));
                put_string(&mut buf, truncate(&h.passphrase, 128));
                buf.push(u8::from(h.wpa3));
                let addrs = &h.addrs[..h.addrs.len().min(8)];
                buf.push(addrs.len() as u8);
                for a in addrs {
                    buf.extend_from_slice(&a.octets());
                }
            }
        }
        w.write_all(&buf).await?;
        w.flush().await?;
        Ok(())
    }

    pub async fn read_from<R: AsyncRead + Unpin>(r: &mut R) -> Result<Self> {
        match r.read_u8().await? {
            0 => Ok(Self::Refused),
            1 => Ok(Self::Ready),
            2 => {
                let ssid = read_string(r, 64, "network name too long").await?;
                let passphrase = read_string(r, 128, "passphrase too long").await?;
                let wpa3 = r.read_u8().await? != 0;
                let count = r.read_u8().await? as usize;
                if count > 8 {
                    return Err(Error::Protocol("too many addresses"));
                }
                let mut addrs = Vec::with_capacity(count);
                for _ in 0..count {
                    let mut o = [0u8; 4];
                    r.read_exact(&mut o).await?;
                    addrs.push(o.into());
                }
                Ok(Self::Join(Hotspot {
                    ssid,
                    passphrase,
                    wpa3,
                    addrs,
                }))
            }
            _ => Err(Error::Protocol("unknown link reply")),
        }
    }
}

/// Clipboard text for a paired device. Applied at once, without asking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    pub text: String,
}

impl Clip {
    pub async fn write_to<W: AsyncWrite + Unpin>(&self, w: &mut W) -> Result<()> {
        let text = truncate(&self.text, MAX_CLIP_LEN);
        let mut buf = Vec::with_capacity(7 + 4 + text.len());
        put_header(&mut buf, KIND_CLIP);
        buf.extend_from_slice(&(text.len() as u32).to_be_bytes());
        buf.extend_from_slice(text.as_bytes());
        w.write_all(&buf).await?;
        w.flush().await?;
        Ok(())
    }

    async fn read_body<R: AsyncRead + Unpin>(r: &mut R) -> Result<Self> {
        let len = r.read_u32().await? as usize;
        if len > MAX_CLIP_LEN {
            return Err(Error::Protocol("clipboard text too long"));
        }
        let mut bytes = vec![0u8; len];
        r.read_exact(&mut bytes).await?;
        let text = String::from_utf8(bytes).map_err(|_| Error::Protocol("text is not UTF-8"))?;
        Ok(Self { text })
    }
}

/// A copied image for a paired device's clipboard: its type ("image/png",
/// "image/jpeg") and bytes. Applied at once, without asking, like `Clip`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipImage {
    pub mime: String,
    pub data: Vec<u8>,
}

impl ClipImage {
    pub async fn write_to<W: AsyncWrite + Unpin>(&self, w: &mut W) -> Result<()> {
        if self.data.len() > MAX_CLIP_IMAGE_LEN {
            return Err(Error::Protocol("clipboard image too big"));
        }
        let mime = truncate(&self.mime, MAX_MIME_LEN);
        let mut head = Vec::with_capacity(7 + 2 + mime.len() + 4);
        put_header(&mut head, KIND_CLIP_IMAGE);
        put_string(&mut head, mime);
        head.extend_from_slice(&(self.data.len() as u32).to_be_bytes());
        w.write_all(&head).await?;
        w.write_all(&self.data).await?;
        w.flush().await?;
        Ok(())
    }

    async fn read_body<R: AsyncRead + Unpin>(r: &mut R) -> Result<Self> {
        let mime = read_string(r, MAX_MIME_LEN, "image type too long").await?;
        if !mime.starts_with("image/") {
            return Err(Error::Protocol("clipboard item is not an image"));
        }
        let len = r.read_u32().await? as usize;
        if len > MAX_CLIP_IMAGE_LEN {
            return Err(Error::Protocol("clipboard image too big"));
        }
        let mut data = vec![0u8; len];
        r.read_exact(&mut data).await?;
        Ok(Self { mime, data })
    }
}

/// The send a file belongs to: the receiver asks once per batch, and
/// shows how much is coming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Batch {
    /// Random per send; 0 = a file sent on its own.
    pub id: u64,
    /// Files in the batch.
    pub count: u32,
    /// Bytes in the whole batch.
    pub size: u64,
}

impl Batch {
    /// A file sent on its own.
    pub fn single(size: u64) -> Self {
        Self {
            id: 0,
            count: 1,
            size,
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
    pub batch: Batch,
    /// The data is a pack of files, unpacked on arrival (see `pack`).
    pub pack: bool,
}

impl Offer {
    pub async fn write_to<W: AsyncWrite + Unpin>(&self, w: &mut W) -> Result<()> {
        if self.name.len() > MAX_NAME_LEN {
            return Err(Error::BadFileName(self.name.clone()));
        }
        let mut buf = Vec::with_capacity(7 + 2 + self.name.len() + 8 + 8 + 20);
        put_header(&mut buf, if self.pack { KIND_PACK } else { KIND_FILE });
        put_string(&mut buf, &self.name);
        buf.extend_from_slice(&self.size.to_be_bytes());
        buf.extend_from_slice(&self.modified.to_be_bytes());
        buf.extend_from_slice(&self.batch.id.to_be_bytes());
        buf.extend_from_slice(&self.batch.count.to_be_bytes());
        buf.extend_from_slice(&self.batch.size.to_be_bytes());
        w.write_all(&buf).await?;
        w.flush().await?;
        Ok(())
    }

    /// Read a whole file offer, header included.
    pub async fn read_from<R: AsyncRead + Unpin>(r: &mut R) -> Result<Self> {
        match read_header(r).await? {
            KIND_FILE => Self::read_body(r, false).await,
            KIND_PACK => Self::read_body(r, true).await,
            _ => Err(Error::Protocol("expected a file offer")),
        }
    }

    async fn read_body<R: AsyncRead + Unpin>(r: &mut R, pack: bool) -> Result<Self> {
        let name = read_string(r, MAX_NAME_LEN, "file name too long").await?;
        let size = r.read_u64().await?;
        let modified = r.read_u64().await?;
        let batch = Batch {
            id: r.read_u64().await?,
            count: r.read_u32().await?,
            size: r.read_u64().await?,
        };
        Ok(Self {
            name,
            size,
            modified,
            batch,
            pack,
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
    /// The person at the receiver said no.
    Declined,
    /// Nobody answered in time.
    NoAnswer,
    /// The receiver doesn't have the sender paired (any more).
    NotPaired,
    /// The receiver is asking its user. The data goes anyway; the answer
    /// comes later (see `AwaitAnswer`).
    Asking,
    /// Every byte is safely stored, but the receiver's user hasn't
    /// answered yet. Kept if they accept, deleted if they decline.
    Held,
    /// The receiver's user said yes (the reply to `AwaitAnswer`).
    Approved,
    /// The receiver's user cancelled the send: don't send the rest.
    Cancelled,
}

const TAG_REJECT: u8 = 0;
const TAG_ACCEPT: u8 = 1;
const TAG_DONE: u8 = 2;
const TAG_DECLINED: u8 = 3;
const TAG_NO_ANSWER: u8 = 4;
const TAG_NOT_PAIRED: u8 = 5;
const TAG_ASKING: u8 = 6;
const TAG_HELD: u8 = 7;
const TAG_APPROVED: u8 = 8;
const TAG_CANCELLED: u8 = 9;

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
            Self::Declined => w.write_u8(TAG_DECLINED).await?,
            Self::NoAnswer => w.write_u8(TAG_NO_ANSWER).await?,
            Self::NotPaired => w.write_u8(TAG_NOT_PAIRED).await?,
            Self::Asking => w.write_u8(TAG_ASKING).await?,
            Self::Held => w.write_u8(TAG_HELD).await?,
            Self::Approved => w.write_u8(TAG_APPROVED).await?,
            Self::Cancelled => w.write_u8(TAG_CANCELLED).await?,
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
            TAG_DECLINED => Ok(Self::Declined),
            TAG_NO_ANSWER => Ok(Self::NoAnswer),
            TAG_NOT_PAIRED => Ok(Self::NotPaired),
            TAG_ASKING => Ok(Self::Asking),
            TAG_HELD => Ok(Self::Held),
            TAG_APPROVED => Ok(Self::Approved),
            TAG_CANCELLED => Ok(Self::Cancelled),
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
    async fn status_round_trips() {
        for (auto, cable) in [(false, false), (true, false), (false, true), (true, true)] {
            let status = Status {
                auto_accepts_you: auto,
                cable_unused: cable,
            };
            let mut buf = Vec::new();
            status.write_to(&mut buf).await.unwrap();
            let Request::Status(got) = Request::read_from(&mut buf.as_slice()).await.unwrap() else {
                panic!("not a status");
            };
            assert_eq!(got, status);
            let mut reply = Vec::new();
            status.write_reply(&mut reply).await.unwrap();
            assert_eq!(Status::read_reply(&mut reply.as_slice()).await.unwrap(), status);
        }
        // An older build's byte (auto-accept only) still reads the same.
        let old = Status::read_reply(&mut [1u8].as_slice()).await.unwrap();
        assert!(old.auto_accepts_you && !old.cable_unused);
    }

    #[tokio::test]
    async fn offer_round_trips() {
        let offer = Offer {
            name: "holiday 📷.jpg".into(),
            size: 123_456_789_012,
            modified: 1_760_000_000_123_456_789,
            batch: Batch {
                id: 0xDEAD_BEEF,
                count: 3,
                size: 123_456_789_999,
            },
            pack: false,
        };
        let mut buf = Vec::new();
        offer.write_to(&mut buf).await.unwrap();
        let back = Offer::read_from(&mut buf.as_slice()).await.unwrap();
        assert_eq!(back, offer);
        // A pack is the same offer, under its own kind.
        let pack = Offer {
            pack: true,
            ..offer
        };
        let mut buf = Vec::new();
        pack.write_to(&mut buf).await.unwrap();
        assert_eq!(
            Request::read_from(&mut buf.as_slice()).await.unwrap(),
            Request::File(pack)
        );
    }

    #[tokio::test]
    async fn responses_round_trip() {
        for resp in [
            Response::Reject,
            Response::Accept { offset: 42 },
            Response::Declined,
            Response::NoAnswer,
            Response::NotPaired,
            Response::Asking,
            Response::Held,
            Response::Approved,
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
            batch: Batch::single(5),
            pack: false,
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
    async fn clip_and_await_round_trip() {
        let clip = Clip {
            text: "https://example.com/ünïcode".into(),
        };
        let mut buf = Vec::new();
        clip.write_to(&mut buf).await.unwrap();
        assert_eq!(
            Request::read_from(&mut buf.as_slice()).await.unwrap(),
            Request::Clip(clip)
        );
        let ask = AwaitAnswer { batch: 0xFEED };
        let mut buf = Vec::new();
        ask.write_to(&mut buf).await.unwrap();
        assert_eq!(
            Request::read_from(&mut buf.as_slice()).await.unwrap(),
            Request::AwaitAnswer(ask)
        );
    }

    #[tokio::test]
    async fn clip_image_round_trips() {
        let image = ClipImage {
            mime: "image/png".into(),
            data: (0..100_000u32).map(|i| i as u8).collect(),
        };
        let mut buf = Vec::new();
        image.write_to(&mut buf).await.unwrap();
        assert_eq!(
            Request::read_from(&mut buf.as_slice()).await.unwrap(),
            Request::ClipImage(image)
        );
        // Only images.
        let mut buf = Vec::new();
        ClipImage {
            mime: "text/html".into(),
            data: vec![1],
        }
        .write_to(&mut buf)
        .await
        .unwrap();
        assert!(Request::read_from(&mut buf.as_slice()).await.is_err());
    }

    #[tokio::test]
    async fn link_messages_round_trip() {
        for link in [Link::Ask, Link::Offer, Link::AskDirect] {
            let mut buf = Vec::new();
            link.write_to(&mut buf).await.unwrap();
            assert_eq!(
                Request::read_from(&mut buf.as_slice()).await.unwrap(),
                Request::Link(link)
            );
        }
        for reply in [
            LinkReply::Refused,
            LinkReply::Ready,
            LinkReply::Join(Hotspot {
                ssid: "DIRECT-nudge".into(),
                passphrase: "s3cret pass".into(),
                wpa3: true,
                addrs: vec![[192, 168, 49, 1].into()],
            }),
        ] {
            let mut buf = Vec::new();
            reply.write_to(&mut buf).await.unwrap();
            assert_eq!(
                LinkReply::read_from(&mut buf.as_slice()).await.unwrap(),
                reply
            );
        }
    }

    #[tokio::test]
    async fn mirror_messages_round_trip() {
        for m in [Mirror::ShowMe, Mirror::ShowYou] {
            let mut buf = Vec::new();
            m.write_to(&mut buf).await.unwrap();
            assert_eq!(
                Request::read_from(&mut buf.as_slice()).await.unwrap(),
                Request::Mirror(m)
            );
        }
        for reply in [
            MirrorReply::Accepted,
            MirrorReply::Declined,
            MirrorReply::NoAnswer,
            MirrorReply::Locked,
            MirrorReply::Busy,
            MirrorReply::WaitingForUnlock,
        ] {
            let mut buf = Vec::new();
            reply.write_to(&mut buf).await.unwrap();
            assert_eq!(
                MirrorReply::read_from(&mut buf.as_slice()).await.unwrap(),
                reply
            );
        }
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
