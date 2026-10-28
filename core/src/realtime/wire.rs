//! Messages inside the encrypted packets. All integers are big-endian.
//!
//! | type | body |
//! |---|---|
//! | 1 Ping | sender's clock µs u64 |
//! | 2 Pong | the ping's clock u64, replier's clock u64 |
//! | 3 Fragment | stream u8, frame u32, index u16, count u16, flags u8, capture µs u64, data |
//! | 4 Nack | stream u8, frame u32, n u16 (0xFFFF = the whole frame), n × index u16 |
//! | 5 KeyRequest | stream u8 |
//! | 6 Bye | (none) |

pub(super) const PING: u8 = 1;
pub(super) const PONG: u8 = 2;
pub(super) const FRAGMENT: u8 = 3;
pub(super) const NACK: u8 = 4;
pub(super) const KEY_REQUEST: u8 = 5;
pub(super) const BYE: u8 = 6;
/// "My addresses now": sent every second, so a network that appears during
/// a session (a cable plugged in, the phone's hotspot joined) becomes a path.
pub(super) const ADDRS: u8 = 7;
/// Most addresses in one `ADDRS`.
pub(super) const MAX_ADDRS: usize = 16;

/// Fragment flags.
pub(super) const F_KEY: u8 = 1;
/// A resend, answering a NACK.
pub(super) const F_RESENT: u8 = 2;
/// The encoder heals lost frames by itself (intra-refresh): no keyframe needed.
pub(super) const F_INTRA_REFRESH: u8 = 4;

pub(super) const FRAGMENT_HEADER: usize = 1 + 1 + 4 + 2 + 2 + 1 + 8;
/// `n` in a NACK meaning "every fragment of the frame".
pub(super) const WHOLE_FRAME: u16 = 0xFFFF;
/// Most fragment indexes one NACK lists.
pub(super) const MAX_NACK_LIST: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Message<'a> {
    Ping(u64),
    Pong { echo: u64, theirs: u64 },
    Fragment(Fragment<'a>),
    /// `None`: the whole frame.
    Nack { stream: u8, frame: u32, missing: Option<Vec<u16>> },
    KeyRequest { stream: u8 },
    Bye,
    Addrs(Vec<std::net::Ipv4Addr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Fragment<'a> {
    pub stream: u8,
    pub frame: u32,
    pub index: u16,
    pub count: u16,
    pub flags: u8,
    pub capture_us: u64,
    pub data: &'a [u8],
}

pub(super) fn ping(t: u64) -> Vec<u8> {
    let mut m = vec![PING];
    m.extend_from_slice(&t.to_be_bytes());
    m
}

pub(super) fn pong(echo: u64, theirs: u64) -> Vec<u8> {
    let mut m = vec![PONG];
    m.extend_from_slice(&echo.to_be_bytes());
    m.extend_from_slice(&theirs.to_be_bytes());
    m
}

pub(super) fn fragment(f: &Fragment<'_>) -> Vec<u8> {
    let mut m = Vec::with_capacity(FRAGMENT_HEADER + f.data.len());
    m.push(FRAGMENT);
    m.push(f.stream);
    m.extend_from_slice(&f.frame.to_be_bytes());
    m.extend_from_slice(&f.index.to_be_bytes());
    m.extend_from_slice(&f.count.to_be_bytes());
    m.push(f.flags);
    m.extend_from_slice(&f.capture_us.to_be_bytes());
    m.extend_from_slice(f.data);
    m
}

pub(super) fn nack(stream: u8, frame: u32, missing: Option<&[u16]>) -> Vec<u8> {
    let mut m = vec![NACK, stream];
    m.extend_from_slice(&frame.to_be_bytes());
    match missing {
        None => m.extend_from_slice(&WHOLE_FRAME.to_be_bytes()),
        Some(list) => {
            let list = &list[..list.len().min(MAX_NACK_LIST)];
            m.extend_from_slice(&(list.len() as u16).to_be_bytes());
            for i in list {
                m.extend_from_slice(&i.to_be_bytes());
            }
        }
    }
    m
}

pub(super) fn key_request(stream: u8) -> Vec<u8> {
    vec![KEY_REQUEST, stream]
}

pub(super) fn bye() -> Vec<u8> {
    vec![BYE]
}

/// kind, count u8, count × IPv4.
pub(super) fn addrs(ips: &[std::net::Ipv4Addr]) -> Vec<u8> {
    let ips = &ips[..ips.len().min(MAX_ADDRS)];
    let mut m = vec![ADDRS, ips.len() as u8];
    for ip in ips {
        m.extend_from_slice(&ip.octets());
    }
    m
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_be_bytes([b[i], b[i + 1]])
}
fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_be_bytes(b[i..i + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], i: usize) -> u64 {
    u64::from_be_bytes(b[i..i + 8].try_into().unwrap())
}

/// `None` for anything malformed (it was genuine, so this means a bug or a
/// newer peer; either way it's ignored).
pub(super) fn parse(m: &[u8]) -> Option<Message<'_>> {
    let (&kind, b) = m.split_first()?;
    Some(match kind {
        PING if b.len() >= 8 => Message::Ping(u64_at(b, 0)),
        PONG if b.len() >= 16 => Message::Pong {
            echo: u64_at(b, 0),
            theirs: u64_at(b, 8),
        },
        FRAGMENT if m.len() >= FRAGMENT_HEADER => {
            let f = Fragment {
                stream: b[0],
                frame: u32_at(b, 1),
                index: u16_at(b, 5),
                count: u16_at(b, 7),
                flags: b[9],
                capture_us: u64_at(b, 10),
                data: &m[FRAGMENT_HEADER..],
            };
            if f.count == 0 || f.index >= f.count {
                return None;
            }
            Message::Fragment(f)
        }
        NACK if b.len() >= 7 => {
            let n = u16_at(b, 5);
            let missing = if n == WHOLE_FRAME {
                None
            } else {
                let n = (n as usize).min((b.len() - 7) / 2);
                Some((0..n).map(|i| u16_at(b, 7 + i * 2)).collect())
            };
            Message::Nack {
                stream: b[0],
                frame: u32_at(b, 1),
                missing,
            }
        }
        KEY_REQUEST if !b.is_empty() => Message::KeyRequest { stream: b[0] },
        BYE => Message::Bye,
        ADDRS if !b.is_empty() => {
            let n = (b[0] as usize).min(MAX_ADDRS).min((b.len() - 1) / 4);
            Message::Addrs(
                (0..n)
                    .map(|i| std::net::Ipv4Addr::new(b[1 + i * 4], b[2 + i * 4], b[3 + i * 4], b[4 + i * 4]))
                    .collect(),
            )
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        assert_eq!(parse(&ping(5)), Some(Message::Ping(5)));
        assert_eq!(parse(&pong(5, 9)), Some(Message::Pong { echo: 5, theirs: 9 }));
        let f = Fragment {
            stream: 2,
            frame: 77,
            index: 3,
            count: 9,
            flags: F_KEY,
            capture_us: 123_456,
            data: b"pixels",
        };
        assert_eq!(parse(&fragment(&f)), Some(Message::Fragment(f)));
        assert_eq!(
            parse(&nack(1, 8, Some(&[2, 5]))),
            Some(Message::Nack { stream: 1, frame: 8, missing: Some(vec![2, 5]) })
        );
        assert_eq!(
            parse(&nack(1, 8, None)),
            Some(Message::Nack { stream: 1, frame: 8, missing: None })
        );
        assert_eq!(parse(&key_request(3)), Some(Message::KeyRequest { stream: 3 }));
        assert_eq!(parse(&bye()), Some(Message::Bye));
        let ips = vec![[192, 168, 49, 1].into(), [10, 0, 0, 7].into()];
        assert_eq!(parse(&addrs(&ips)), Some(Message::Addrs(ips)));
    }

    #[test]
    fn rejects_malformed() {
        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&[PING, 1, 2]), None);
        assert_eq!(parse(&[99]), None);
        let bad = Fragment {
            stream: 0,
            frame: 1,
            index: 4,
            count: 4, // index must be below count
            flags: 0,
            capture_us: 0,
            data: b"",
        };
        assert_eq!(parse(&fragment(&bad)), None);
    }
}
