//! Which network path the real-time channel uses.
//!
//! Each side pings every address the other could be reached at (each of its
//! networks, plus the address the TLS connection came from). A path is alive
//! while its pongs keep coming. Packets go over the best live path: a cable
//! first, then Ethernet, the phone's hotspot, then the Wi-Fi router; the
//! lowest round trip among equals. The channel only moves to a better kind
//! of path, or when the current one dies, so it doesn't flip between two
//! similar ones.

use std::net::SocketAddr;

use crate::link::LinkKind;

/// How often to ping the path in use, and the others.
const PING_CURRENT_US: u64 = 100_000;
const PING_OTHERS_US: u64 = 250_000;
/// Before any pong: ping often, so the channel starts quickly.
const PING_STARTING_US: u64 = 30_000;
/// A path with no pong for this long is dead.
const DEAD_US: u64 = 500_000;

/// A network path to the other device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathInfo {
    pub remote: SocketAddr,
    pub kind: LinkKind,
    /// Smoothed round trip, µs.
    pub rtt_us: u64,
}

fn rank(kind: LinkKind) -> u8 {
    match kind {
        LinkKind::Usb => 0,
        LinkKind::Wired => 1,
        LinkKind::Hotspot => 2,
        LinkKind::Wifi => 3,
        LinkKind::Other => 4,
    }
}

struct Path {
    remote: SocketAddr,
    kind: LinkKind,
    srtt: Option<u64>,
    last_pong: Option<u64>,
    last_ping: Option<u64>,
}

impl Path {
    fn alive(&self, now: u64) -> bool {
        self.last_pong.is_some_and(|t| now.saturating_sub(t) < DEAD_US)
    }

    fn info(&self) -> PathInfo {
        PathInfo {
            remote: self.remote,
            kind: self.kind,
            rtt_us: self.srtt.unwrap_or(0),
        }
    }
}

#[derive(Default)]
pub(super) struct Paths {
    list: Vec<Path>,
    current: Option<SocketAddr>,
}

impl Paths {
    /// Adds a path to try (no-op if known).
    pub(super) fn add(&mut self, remote: SocketAddr, kind: LinkKind) {
        if !self.list.iter().any(|p| p.remote == remote) {
            self.list.push(Path {
                remote,
                kind,
                srtt: None,
                last_pong: None,
                last_ping: None,
            });
        }
    }

    /// What kind of path `remote` is, as this side's networks change.
    pub(super) fn set_kind(&mut self, remote: SocketAddr, kind: LinkKind) {
        if let Some(p) = self.list.iter_mut().find(|p| p.remote == remote) {
            p.kind = kind;
        }
    }

    /// Every path's address.
    pub(super) fn remotes(&self) -> Vec<SocketAddr> {
        self.list.iter().map(|p| p.remote).collect()
    }

    /// The paths that are alive.
    pub(super) fn alive(&self, now: u64) -> Vec<SocketAddr> {
        self.list.iter().filter(|p| p.alive(now)).map(|p| p.remote).collect()
    }

    pub(super) fn knows(&self, remote: SocketAddr) -> bool {
        self.list.iter().any(|p| p.remote == remote)
    }

    pub(super) fn on_pong(&mut self, remote: SocketAddr, rtt: u64, now: u64) {
        if let Some(p) = self.list.iter_mut().find(|p| p.remote == remote) {
            p.srtt = Some(p.srtt.map_or(rtt, |s| (s * 7 + rtt) / 8));
            p.last_pong = Some(now);
        }
    }

    /// Paths due a ping now.
    pub(super) fn due_pings(&mut self, now: u64) -> Vec<SocketAddr> {
        let current = self.current;
        let mut due = Vec::new();
        for p in &mut self.list {
            let every = if p.last_pong.is_none() {
                PING_STARTING_US
            } else if Some(p.remote) == current {
                PING_CURRENT_US
            } else {
                PING_OTHERS_US
            };
            if p.last_ping.is_none_or(|t| now.saturating_sub(t) >= every) {
                p.last_ping = Some(now);
                due.push(p.remote);
            }
        }
        due
    }

    /// When the next ping is due.
    pub(super) fn next_ping_at(&self) -> Option<u64> {
        self.list
            .iter()
            .map(|p| {
                let every = if p.last_pong.is_none() {
                    PING_STARTING_US
                } else if Some(p.remote) == self.current {
                    PING_CURRENT_US
                } else {
                    PING_OTHERS_US
                };
                p.last_ping.map_or(0, |t| t + every)
            })
            .min()
    }

    /// Re-picks the path; returns the new one if it changed (`None` inside:
    /// no path is alive any more).
    pub(super) fn choose(&mut self, now: u64) -> Option<Option<PathInfo>> {
        let best = self
            .list
            .iter()
            .filter(|p| p.alive(now))
            .min_by_key(|p| (rank(p.kind), p.srtt.unwrap_or(u64::MAX)));
        let current = self.current.and_then(|c| self.list.iter().find(|p| p.remote == c && p.alive(now)));
        let pick = match (current, best) {
            (Some(c), Some(b)) if rank(b.kind) >= rank(c.kind) => c,
            (_, Some(b)) => b,
            (_, None) => {
                return self.current.take().map(|_| None);
            }
        };
        if self.current == Some(pick.remote) {
            return None;
        }
        self.current = Some(pick.remote);
        Some(Some(pick.info()))
    }

    pub(super) fn current(&self) -> Option<PathInfo> {
        let c = self.current?;
        self.list.iter().find(|p| p.remote == c).map(Path::info)
    }

    /// Every path and whether it's alive.
    pub(super) fn all(&self, now: u64) -> Vec<(PathInfo, bool)> {
        self.list.iter().map(|p| (p.info(), p.alive(now))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(last: u8) -> SocketAddr {
        SocketAddr::from(([192, 168, 1, last], 5000))
    }

    #[test]
    fn prefers_the_cable_then_the_lowest_rtt() {
        let mut p = Paths::default();
        p.add(addr(1), LinkKind::Wifi);
        p.add(addr(2), LinkKind::Wifi);
        assert_eq!(p.choose(0), None, "nothing alive yet");

        p.on_pong(addr(1), 8_000, 10);
        p.on_pong(addr(2), 3_000, 10);
        assert_eq!(p.choose(10).unwrap().unwrap().remote, addr(2));

        // A cable appears: switch to it even though it's slower here.
        p.add(addr(3), LinkKind::Usb);
        p.on_pong(addr(3), 9_000, 20);
        assert_eq!(p.choose(20).unwrap().unwrap().kind, LinkKind::Usb);

        // An equally good kind doesn't make it switch back and forth.
        p.on_pong(addr(1), 1_000, 30);
        assert_eq!(p.choose(30), None);
    }

    #[test]
    fn falls_back_when_the_path_dies() {
        let mut p = Paths::default();
        p.add(addr(1), LinkKind::Usb);
        p.add(addr(2), LinkKind::Wifi);
        p.on_pong(addr(1), 1_000, 0);
        p.on_pong(addr(2), 5_000, 0);
        assert_eq!(p.choose(0).unwrap().unwrap().kind, LinkKind::Usb);

        // The cable is unplugged: only Wi-Fi keeps answering.
        p.on_pong(addr(2), 5_000, DEAD_US);
        assert_eq!(p.choose(DEAD_US).unwrap().unwrap().kind, LinkKind::Wifi);

        // Then nothing at all.
        assert_eq!(p.choose(3 * DEAD_US), Some(None));
        assert_eq!(p.choose(3 * DEAD_US), None);
    }

    #[test]
    fn pings_often_until_answered() {
        let mut p = Paths::default();
        p.add(addr(1), LinkKind::Wifi);
        assert_eq!(p.due_pings(0), [addr(1)]);
        assert!(p.due_pings(PING_STARTING_US - 1).is_empty());
        assert_eq!(p.due_pings(PING_STARTING_US), [addr(1)]);
        p.on_pong(addr(1), 1_000, PING_STARTING_US);
        p.choose(PING_STARTING_US);
        assert!(p.due_pings(PING_STARTING_US + PING_CURRENT_US - 1).is_empty());
        assert_eq!(p.next_ping_at(), Some(PING_STARTING_US + PING_CURRENT_US));
    }
}
