//! The real-time channel: screen-mirroring video (and later audio and input)
//! between paired devices.
//!
//! Files go over TLS on TCP, where every byte arrives in order. For live
//! video that's wrong: one lost packet stalls everything behind it. This
//! channel uses UDP instead. Lost fragments are asked for again for a short
//! while (about 50 ms), then given up on, so a late frame never holds up the
//! picture.
//!
//! - **Setup:** over a TLS connection the two devices already have (so only
//!   paired devices get this far). A short handshake swaps UDP ports and
//!   addresses; the packet keys come from that TLS session (key export), so
//!   they're bound to both devices' certificates.
//! - **Packets:** AES-256-GCM, a key per direction, replays dropped
//!   (`crypto`).
//! - **Frames:** split into ~1150-byte fragments, paced, resent on request,
//!   put back together in order (`frames`).
//! - **Paths:** every network the devices share is tried; the cable wins,
//!   then the phone's hotspot, then the router (`paths`). If the path in use
//!   dies the channel moves to another within about half a second.
//!
//! The session ends when either side closes it, drops it, or closes the TLS
//! connection.

mod crypto;
mod frames;
mod paths;
mod rate;
mod session;
mod wire;

pub use crypto::OVERHEAD;
pub use frames::{Frame, MAX_FRAGMENT, OutFrame, ReceiveStats, SendStats};
pub use paths::PathInfo;
pub use rate::{RateChange, RateControl};
pub use session::{Config, Control, Event, Handle, Session, Stats};
