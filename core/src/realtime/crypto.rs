//! Packet encryption for the real-time channel.
//!
//! Keys come from the TLS session the two devices already share (TLS key
//! export, RFC 8446 §7.5), so only the two paired devices that completed
//! that handshake can read or forge packets. Each direction has its own
//! AES-256-GCM key. The nonce is the packet's sequence number, which only
//! goes up, so a key never sees the same nonce twice.
//!
//! Packet: marker u8 | session tag u32 | sequence u64 | ciphertext | 16-byte tag.
//! The 13-byte header is authenticated but not encrypted.

use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};

/// First byte of every real-time packet.
pub(super) const MARKER: u8 = 0xA7;
pub(super) const HEADER: usize = 13;
pub(super) const TAG: usize = 16;
/// Bytes a packet adds around its payload.
pub const OVERHEAD: usize = HEADER + TAG;

/// How far back an out-of-order packet may arrive and still be accepted.
const WINDOW: u64 = 128;

fn nonce(seq: u64) -> Nonce {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&seq.to_be_bytes());
    Nonce::assume_unique_for_key(n)
}

fn header(tag: u32, seq: u64) -> [u8; HEADER] {
    let mut h = [0u8; HEADER];
    h[0] = MARKER;
    h[1..5].copy_from_slice(&tag.to_be_bytes());
    h[5..].copy_from_slice(&seq.to_be_bytes());
    h
}

/// Encrypts this side's packets.
pub(super) struct Sealer {
    key: LessSafeKey,
    tag: u32,
    seq: u64,
}

impl Sealer {
    pub(super) fn new(key: &[u8; 32], tag: u32) -> Self {
        let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).expect("32-byte key"));
        Self { key, tag, seq: 0 }
    }

    /// A packet carrying `payload`.
    pub(super) fn seal(&mut self, payload: &[u8]) -> Vec<u8> {
        self.seq += 1;
        let h = header(self.tag, self.seq);
        let mut out = Vec::with_capacity(OVERHEAD + payload.len());
        out.extend_from_slice(&h);
        out.extend_from_slice(payload);
        let mut body = out.split_off(HEADER);
        self.key
            .seal_in_place_append_tag(nonce(self.seq), Aad::from(h), &mut body)
            .expect("payload fits AES-GCM limits");
        out.append(&mut body);
        out
    }
}

/// Decrypts the other side's packets and drops replays.
pub(super) struct Opener {
    key: LessSafeKey,
    tag: u32,
    /// Highest sequence number accepted, and which of the `WINDOW` before it
    /// have been seen (bit i = `highest - i`).
    highest: u64,
    seen: u128,
}

impl Opener {
    pub(super) fn new(key: &[u8; 32], tag: u32) -> Self {
        let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).expect("32-byte key"));
        Self {
            key,
            tag,
            highest: 0,
            seen: 0,
        }
    }

    /// The payload, or `None` if the packet is forged, damaged, from another
    /// session, or a replay.
    pub(super) fn open<'a>(&mut self, packet: &'a mut [u8]) -> Option<&'a [u8]> {
        if packet.len() < OVERHEAD || packet[0] != MARKER {
            return None;
        }
        let (h, body) = packet.split_at_mut(HEADER);
        if u32::from_be_bytes(h[1..5].try_into().unwrap()) != self.tag {
            return None;
        }
        let seq = u64::from_be_bytes(h[5..].try_into().unwrap());
        if seq == 0 || self.already_seen(seq) {
            return None;
        }
        let h: [u8; HEADER] = (&*h).try_into().unwrap();
        let plain = self.key.open_in_place(nonce(seq), Aad::from(h), body).ok()?;
        // Only now, once it's proven genuine, does it move the window.
        self.mark(seq);
        Some(plain)
    }

    fn already_seen(&self, seq: u64) -> bool {
        if seq > self.highest {
            return false;
        }
        let back = self.highest - seq;
        back >= WINDOW || self.seen & (1 << back) != 0
    }

    fn mark(&mut self, seq: u64) {
        if seq > self.highest {
            let shift = seq - self.highest;
            self.seen = if shift >= WINDOW { 0 } else { self.seen << shift };
            self.highest = seq;
        }
        self.seen |= 1 << (self.highest - seq);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (Sealer, Opener) {
        let key = [7u8; 32];
        (Sealer::new(&key, 42), Opener::new(&key, 42))
    }

    #[test]
    fn round_trip() {
        let (mut s, mut o) = pair();
        let mut p = s.seal(b"hello");
        assert_eq!(p.len(), 5 + OVERHEAD);
        assert_eq!(o.open(&mut p).unwrap(), b"hello");
    }

    #[test]
    fn rejects_tampering_and_other_sessions() {
        let (mut s, mut o) = pair();
        let mut p = s.seal(b"hello");
        p[HEADER] ^= 1;
        assert!(o.open(&mut p).is_none());

        let mut p = s.seal(b"hello");
        p[8] ^= 1; // the sequence number is authenticated too
        assert!(o.open(&mut p).is_none());

        let mut other = Sealer::new(&[7u8; 32], 43).seal(b"hello");
        assert!(o.open(&mut other).is_none());

        let mut wrong_key = Sealer::new(&[8u8; 32], 42).seal(b"hello");
        assert!(o.open(&mut wrong_key).is_none());
    }

    #[test]
    fn rejects_replays_but_allows_reordering() {
        let (mut s, mut o) = pair();
        let packets: Vec<Vec<u8>> = (0..5).map(|i| s.seal(&[i])).collect();
        // Arrive as 0, 3, 1, 4, 2: all fine.
        for i in [0, 3, 1, 4, 2] {
            let mut p = packets[i].clone();
            assert_eq!(o.open(&mut p).unwrap(), &[i as u8]);
        }
        // Any of them again: refused.
        for p in &packets {
            assert!(o.open(&mut p.clone()).is_none());
        }
        // Far too old: refused even if never seen.
        let old = s.seal(b"old");
        for _ in 0..WINDOW + 5 {
            let mut p = s.seal(b"x");
            o.open(&mut p).unwrap();
        }
        assert!(o.open(&mut old.clone()).is_none());
    }

    #[test]
    fn forged_packets_dont_move_the_window() {
        let (mut s, mut o) = pair();
        let mut forged = s.seal(b"a");
        forged[5..13].copy_from_slice(&1_000_000u64.to_be_bytes());
        assert!(o.open(&mut forged).is_none());
        // A genuine packet with a low number still gets in.
        let mut p = s.seal(b"b");
        assert_eq!(o.open(&mut p).unwrap(), b"b");
    }
}
