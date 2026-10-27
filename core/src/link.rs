//! Which network path a connection takes, how fast each path has been, and
//! picking the fastest.
//!
//! A phone and a PC can share more than one network at once: the Wi-Fi
//! router, a USB cable (USB tethering shows up as a network), or the phone's
//! own hotspot. Every address a device is reachable at belongs to one of
//! them. This module tells them apart by the local interface that reaches
//! the address, and keeps measured speeds per device and path, so choices
//! come from real numbers rather than guesses.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use crate::identity::Fingerprint;

/// A kind of network path, fastest-typical first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkKind {
    /// A USB cable (USB tethering).
    Usb,
    /// Ethernet. On Windows a USB-tethered phone also shows up as Ethernet.
    Wired,
    /// Straight to the phone's own hotspot: one hop over the air.
    Hotspot,
    /// Through the Wi-Fi router: every byte crosses the air twice.
    Wifi,
    Other,
}

impl LinkKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Usb => "usb",
            Self::Wired => "wired",
            Self::Hotspot => "hotspot",
            Self::Wifi => "wifi",
            Self::Other => "other",
        }
    }

    /// The name in the speeds file. The direct link was once only a
    /// local-only hotspot, which phones put on 2.4 GHz; its speeds were
    /// stored as "hotspot" and say nothing about today's 5 GHz Wi-Fi Direct
    /// group, so they're left behind.
    fn stored_name(self) -> &'static str {
        match self {
            Self::Hotspot => "direct",
            other => other.name(),
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "usb" => Self::Usb,
            "wired" => Self::Wired,
            "direct" => Self::Hotspot,
            "wifi" => Self::Wifi,
            "other" => Self::Other,
            _ => return None,
        })
    }

    /// Speed to expect, in bytes per second, before any has been measured.
    /// Deliberately modest: a measured number replaces it after one
    /// transfer.
    fn assumed(self) -> f64 {
        const MB: f64 = 1e6;
        match self {
            Self::Usb => 35.0 * MB,
            Self::Wired => 35.0 * MB,
            Self::Hotspot => 25.0 * MB,
            Self::Wifi => 12.0 * MB,
            Self::Other => 5.0 * MB,
        }
    }
}

/// What kind of path an interface is, from its name: Android's `wlan0`,
/// `rndis0`, `swlan0`..., Windows' "Wi-Fi", "Ethernet 3"...
pub fn classify(interface: &str) -> LinkKind {
    let n = interface.to_ascii_lowercase();
    let numbered = |prefix: &str| {
        n.strip_prefix(prefix)
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
    };
    if n.starts_with("rndis") || n.starts_with("usb") || n.starts_with("ncm") || n.contains("ndis")
    {
        LinkKind::Usb
    } else if n.starts_with("swlan")
        || n.starts_with("p2p")
        || n.starts_with("softap")
        || n.starts_with("ap_")
        || numbered("ap")
        || (numbered("wlan") && n != "wlan0")
    {
        // The phone's own access point (Samsung swlan0, Pixel wlan1, ap0),
        // or the Wi-Fi Direct group it owns (p2p-wlan0-0).
        LinkKind::Hotspot
    } else if n.starts_with("wlan")
        || n.starts_with("wl")
        || n.contains("wi-fi")
        || n.contains("wifi")
        || n.contains("wireless")
    {
        LinkKind::Wifi
    } else if n.starts_with("eth") || n.contains("ethernet") || n.starts_with("lan") {
        LinkKind::Wired
    } else {
        LinkKind::Other
    }
}

/// One of this device's IPv4 networks.
#[derive(Debug, Clone, Copy)]
pub struct LocalNet {
    pub ip: Ipv4Addr,
    pub netmask: Ipv4Addr,
    pub kind: LinkKind,
}

impl LocalNet {
    pub fn contains(&self, other: Ipv4Addr) -> bool {
        let mask = u32::from(self.netmask);
        u32::from(self.ip) & mask == u32::from(other) & mask
    }
}

/// This device's IPv4 networks that are up, loopback excluded.
pub fn local_nets() -> Vec<LocalNet> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback() && i.is_oper_up())
        .filter_map(|i| match &i.addr {
            if_addrs::IfAddr::V4(v4) => Some(LocalNet {
                ip: v4.ip,
                netmask: v4.netmask,
                kind: classify(&i.name),
            }),
            if_addrs::IfAddr::V6(_) => None,
        })
        .collect()
}

/// The path to `peer` (an address of another device): the kind of the
/// local network it's on, if any.
pub fn kind_toward(nets: &[LocalNet], peer: IpAddr) -> Option<LinkKind> {
    let IpAddr::V4(v4) = peer else {
        return None;
    };
    nets.iter().find(|n| n.contains(v4)).map(|n| n.kind)
}

/// The path a connection takes, from its local address.
pub fn kind_of_local(nets: &[LocalNet], local: IpAddr) -> LinkKind {
    match local {
        IpAddr::V4(v4) => nets
            .iter()
            .find(|n| n.ip == v4)
            .map_or(LinkKind::Other, |n| n.kind),
        IpAddr::V6(_) => LinkKind::Other,
    }
}

/// Whether moving the rest of a transfer to a faster path pays off: it must
/// be at least 20% faster and finish at least ten seconds sooner, setup
/// included. Speeds in bytes per second.
pub fn worth_switching(remaining: u64, now_speed: f64, new_speed: f64, setup: Duration) -> bool {
    if now_speed <= 0.0 || new_speed < now_speed * 1.2 {
        return false;
    }
    let remaining = remaining as f64;
    let stay = remaining / now_speed;
    let switch = setup.as_secs_f64() + remaining / new_speed;
    stay - switch >= 10.0
}

/// Transfers shorter than this say little about a path's speed.
const MIN_SAMPLE: u64 = 16 << 20;

/// Measured speeds per device and path, kept beside the identity.
pub struct Speeds {
    file: PathBuf,
    known: Mutex<HashMap<(Fingerprint, LinkKind), f64>>,
}

impl Speeds {
    const FILE: &'static str = "speeds.txt";

    pub fn load(dir: &Path) -> Self {
        let file = dir.join(Self::FILE);
        let known = std::fs::read_to_string(&file)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| {
                let mut parts = line.split_whitespace();
                let fp = parts.next()?.parse().ok()?;
                let kind = LinkKind::parse(parts.next()?)?;
                let speed: f64 = parts.next()?.parse().ok()?;
                (speed.is_finite() && speed > 0.0).then_some(((fp, kind), speed))
            })
            .collect();
        Self {
            file,
            known: Mutex::new(known),
        }
    }

    /// Bytes per second last seen to `peer` over `kind`.
    pub fn measured(&self, peer: &Fingerprint, kind: LinkKind) -> Option<f64> {
        self.lock().get(&(*peer, kind)).copied()
    }

    /// What to expect: measured, or else a modest guess.
    pub fn expected(&self, peer: &Fingerprint, kind: LinkKind) -> f64 {
        self.measured(peer, kind).unwrap_or_else(|| kind.assumed())
    }

    /// Record a finished transfer. Short ones are ignored; the rest are
    /// blended with what was known, so one slow moment doesn't stick.
    pub fn record(&self, peer: Fingerprint, kind: LinkKind, bytes: u64, elapsed: Duration) {
        if bytes < MIN_SAMPLE || elapsed.is_zero() {
            return;
        }
        let speed = bytes as f64 / elapsed.as_secs_f64();
        let text = {
            let mut known = self.lock();
            let blended = known
                .get(&(peer, kind))
                .map_or(speed, |old| 0.5 * old + 0.5 * speed);
            known.insert((peer, kind), blended);
            let mut text = String::from("# Measured speeds: device, path, bytes per second.\n");
            for ((fp, kind), speed) in known.iter() {
                text.push_str(&format!("{fp} {} {speed:.0}\n", kind.stored_name()));
            }
            text
        };
        let _ = std::fs::write(&self.file, text);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(Fingerprint, LinkKind), f64>> {
        self.known.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_interfaces() {
        assert_eq!(classify("wlan0"), LinkKind::Wifi);
        assert_eq!(classify("Wi-Fi"), LinkKind::Wifi);
        assert_eq!(classify("Wi-Fi 2"), LinkKind::Wifi);
        assert_eq!(classify("rndis0"), LinkKind::Usb);
        assert_eq!(classify("ncm0"), LinkKind::Usb);
        assert_eq!(classify("swlan0"), LinkKind::Hotspot);
        assert_eq!(classify("wlan1"), LinkKind::Hotspot);
        assert_eq!(classify("ap0"), LinkKind::Hotspot);
        assert_eq!(classify("p2p-wlan0-0"), LinkKind::Hotspot);
        assert_eq!(classify("p2p0"), LinkKind::Hotspot);
        assert_eq!(classify("Ethernet 3"), LinkKind::Wired);
        assert_eq!(classify("eth0"), LinkKind::Wired);
        assert_eq!(classify("rmnet_data0"), LinkKind::Other);
    }

    #[test]
    fn finds_the_network_an_address_is_on() {
        let nets = [
            LocalNet {
                ip: Ipv4Addr::new(192, 168, 1, 20),
                netmask: Ipv4Addr::new(255, 255, 255, 0),
                kind: LinkKind::Wifi,
            },
            LocalNet {
                ip: Ipv4Addr::new(192, 168, 42, 129),
                netmask: Ipv4Addr::new(255, 255, 255, 0),
                kind: LinkKind::Usb,
            },
        ];
        let usb = IpAddr::V4(Ipv4Addr::new(192, 168, 42, 7));
        assert_eq!(kind_toward(&nets, usb), Some(LinkKind::Usb));
        let elsewhere = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
        assert_eq!(kind_toward(&nets, elsewhere), None);
        assert_eq!(
            kind_of_local(&nets, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20))),
            LinkKind::Wifi
        );
    }

    #[test]
    fn switches_only_when_it_pays() {
        const MB: f64 = 1e6;
        let setup = Duration::from_secs(8);
        // 2 GB left at 10 MB/s (200 s) vs 25 MB/s (8 + 80 s): switch.
        assert!(worth_switching(2_000_000_000, 10.0 * MB, 25.0 * MB, setup));
        // 50 MB left: 5 s vs 10 s. Stay.
        assert!(!worth_switching(50_000_000, 10.0 * MB, 25.0 * MB, setup));
        // Barely faster: stay, however much is left.
        assert!(!worth_switching(u64::MAX / 2, 10.0 * MB, 11.0 * MB, setup));
    }

    #[test]
    fn remembers_speeds() {
        let dir = tempfile::tempdir().unwrap();
        let fp = Fingerprint::from_bytes([3; 32]);
        let speeds = Speeds::load(dir.path());
        assert_eq!(speeds.measured(&fp, LinkKind::Wifi), None);
        speeds.record(fp, LinkKind::Wifi, 100_000_000, Duration::from_secs(10));
        speeds.record(fp, LinkKind::Wifi, 1_000, Duration::from_secs(1)); // too short
        speeds.record(fp, LinkKind::Hotspot, 100_000_000, Duration::from_secs(4));
        let again = Speeds::load(dir.path());
        assert_eq!(again.measured(&fp, LinkKind::Wifi), Some(10_000_000.0));
        assert_eq!(again.measured(&fp, LinkKind::Hotspot), Some(25_000_000.0));
    }

    #[test]
    fn forgets_old_hotspot_speeds() {
        let dir = tempfile::tempdir().unwrap();
        let fp = Fingerprint::from_bytes([3; 32]);
        std::fs::write(
            dir.path().join(Speeds::FILE),
            format!("{fp} hotspot 9000000\n{fp} wifi 20000000\n"),
        )
        .unwrap();
        let speeds = Speeds::load(dir.path());
        assert_eq!(speeds.measured(&fp, LinkKind::Hotspot), None);
        assert_eq!(speeds.measured(&fp, LinkKind::Wifi), Some(20_000_000.0));
    }
}
