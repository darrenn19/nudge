//! Finding other Nudge devices on the local network.
//!
//! Two methods run side by side, and their results are merged per device:
//! - **mDNS / DNS-SD:** service `_nudge._tcp.local.`, with the device's
//!   fingerprint, name and platform in its TXT record.
//! - **UDP broadcast** on `DISCOVERY_PORT`, for networks that drop multicast.
//!   Each device broadcasts a small beacon every `BEACON_INTERVAL` and
//!   answers queries with a unicast beacon.
//!
//! Discovery only says who is nearby and where. It proves nothing: the
//! fingerprint a device advertises is checked by TLS when connecting.
//! Must be started inside a tokio runtime.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::identity::Fingerprint;
use crate::protocol::truncate;
use crate::{PROTOCOL_VERSION, Result};

/// mDNS service type Nudge devices advertise.
pub const SERVICE_TYPE: &str = "_nudge._tcp.local.";

/// UDP port for broadcast beacons.
pub const DISCOVERY_PORT: u16 = 47_801;

/// How often each device broadcasts its beacon.
pub const BEACON_INTERVAL: Duration = Duration::from_secs(5);

/// A device found only by beacon counts as gone after this long without one.
const BEACON_EXPIRY: Duration = Duration::from_secs(16);

const BEACON_MAGIC: [u8; 4] = *b"NDGD";
const BEACON_VERSION: u8 = 1;
const KIND_ANNOUNCE: u8 = 1;
const KIND_QUERY: u8 = 2;
const KIND_GOODBYE: u8 = 3;

/// Longest device name put in a beacon or TXT record.
const MAX_ADVERTISED_NAME: usize = 200;

/// How this device presents itself to others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    pub fingerprint: Fingerprint,
    pub device_name: String,
    /// Short platform tag such as "windows" or "android", for picking an icon.
    pub platform: String,
    /// TCP port this device accepts transfers on.
    pub port: u16,
}

/// Another device seen on the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredPeer {
    pub fingerprint: Fingerprint,
    pub device_name: String,
    pub platform: String,
    /// Addresses to try, best first (private IPv4 before others).
    pub addrs: Vec<IpAddr>,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryEvent {
    /// A device appeared, or its details (name, addresses) changed.
    Found(DiscoveredPeer),
    /// A device left or stopped answering.
    Lost(Fingerprint),
}

/// Knobs for discovery; the defaults suit real use.
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    pub mdns: bool,
    pub broadcast: bool,
    /// Port beacons are sent to and (if free) received on.
    pub udp_port: u16,
    /// Extra addresses to send beacons to, beyond every interface's
    /// broadcast address. Lets tests run two devices on one machine.
    pub extra_targets: Vec<SocketAddr>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            mdns: true,
            broadcast: true,
            udp_port: DISCOVERY_PORT,
            extra_targets: Vec::new(),
        }
    }
}

/// Running discovery. Stops (and tells others it left) when dropped.
pub struct Discovery {
    mdns: Option<(ServiceDaemon, String)>,
    beacon: Option<Arc<Beacon>>,
    tasks: Vec<JoinHandle<()>>,
}

impl Discovery {
    /// Start advertising `me` and looking for others. Events arrive on the
    /// returned channel. Fails only if neither method could start.
    pub fn start(
        me: Announcement,
        config: DiscoveryConfig,
    ) -> Result<(Self, mpsc::UnboundedReceiver<DiscoveryEvent>)> {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let (sightings_tx, sightings_rx) = mpsc::unbounded_channel();
        let mut tasks = vec![tokio::spawn(merge(me.fingerprint, sightings_rx, events_tx))];

        let mut first_error = None;
        let mdns = if config.mdns {
            match start_mdns(&me, sightings_tx.clone()) {
                Ok((daemon, fullname, task)) => {
                    tasks.push(task);
                    Some((daemon, fullname))
                }
                Err(e) => {
                    first_error = Some(e);
                    None
                }
            }
        } else {
            None
        };
        let beacon = if config.broadcast {
            match Beacon::bind(&me, &config) {
                Ok(beacon) => {
                    let beacon = Arc::new(beacon);
                    tasks.push(tokio::spawn(beacon.clone().run(sightings_tx)));
                    Some(beacon)
                }
                Err(e) => {
                    first_error.get_or_insert(e);
                    None
                }
            }
        } else {
            None
        };
        if mdns.is_none()
            && beacon.is_none()
            && let Some(e) = first_error
        {
            tasks.iter().for_each(JoinHandle::abort);
            return Err(e);
        }
        Ok((
            Self {
                mdns,
                beacon,
                tasks,
            },
            events_rx,
        ))
    }

    /// Ask nearby devices to announce themselves now, e.g. when a device
    /// list opens. Cheap; safe to call often.
    pub fn refresh(&self) {
        if let Some(beacon) = &self.beacon {
            beacon.send_all(KIND_QUERY);
        }
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        if let Some(beacon) = &self.beacon {
            beacon.send_all(KIND_GOODBYE);
        }
        if let Some((daemon, fullname)) = self.mdns.take() {
            let _ = daemon.unregister(&fullname);
            let _ = daemon.shutdown();
        }
        self.tasks.iter().for_each(JoinHandle::abort);
    }
}

/// One method's report about one device.
#[derive(Debug)]
enum Sighting {
    Mdns {
        /// mDNS instance name, needed to match the later removal.
        fullname: String,
        peer: DiscoveredPeer,
    },
    MdnsGone {
        fullname: String,
    },
    Beacon(DiscoveredPeer),
    BeaconGone(Fingerprint),
}

#[derive(Default)]
struct Entry {
    mdns: Option<DiscoveredPeer>,
    beacon: Option<(DiscoveredPeer, Instant)>,
    reported: Option<DiscoveredPeer>,
}

impl Entry {
    /// What both methods together say about the device, if it's still here.
    fn merged(&self, now: Instant) -> Option<DiscoveredPeer> {
        let beacon = self
            .beacon
            .as_ref()
            .filter(|(_, seen)| now.duration_since(*seen) < BEACON_EXPIRY)
            .map(|(p, _)| p);
        let mut peer = self.mdns.clone().or_else(|| beacon.cloned())?;
        if let Some(b) = beacon {
            peer.addrs.extend(b.addrs.iter().copied());
            if peer.device_name.is_empty() {
                peer.device_name.clone_from(&b.device_name);
            }
        }
        peer.addrs = rank_addrs(peer.addrs);
        Some(peer)
    }
}

/// Merge sightings from both methods into Found/Lost events.
async fn merge(
    own: Fingerprint,
    mut sightings: mpsc::UnboundedReceiver<Sighting>,
    events: mpsc::UnboundedSender<DiscoveryEvent>,
) {
    let mut table: HashMap<Fingerprint, Entry> = HashMap::new();
    let mut mdns_names: HashMap<String, Fingerprint> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    loop {
        let touched: Vec<Fingerprint> = tokio::select! {
            sighting = sightings.recv() => {
                let Some(sighting) = sighting else { return };
                let now = Instant::now();
                match sighting {
                    Sighting::Mdns { fullname, peer } => {
                        let fp = peer.fingerprint;
                        mdns_names.insert(fullname, fp);
                        table.entry(fp).or_default().mdns = Some(peer);
                        vec![fp]
                    }
                    Sighting::MdnsGone { fullname } => match mdns_names.remove(&fullname) {
                        Some(fp) => {
                            if let Some(e) = table.get_mut(&fp) {
                                e.mdns = None;
                            }
                            vec![fp]
                        }
                        None => vec![],
                    },
                    Sighting::Beacon(peer) => {
                        let fp = peer.fingerprint;
                        table.entry(fp).or_default().beacon = Some((peer, now));
                        vec![fp]
                    }
                    Sighting::BeaconGone(fp) => {
                        if let Some(e) = table.get_mut(&fp) {
                            e.beacon = None;
                        }
                        vec![fp]
                    }
                }
            }
            _ = tick.tick() => table.keys().copied().collect(),
        };

        let now = Instant::now();
        for fp in touched {
            if fp == own {
                continue;
            }
            let Some(entry) = table.get_mut(&fp) else {
                continue;
            };
            let merged = entry.merged(now);
            if merged == entry.reported {
                continue;
            }
            let event = match &merged {
                Some(peer) => DiscoveryEvent::Found(peer.clone()),
                None => DiscoveryEvent::Lost(fp),
            };
            entry.reported = merged;
            if events.send(event).is_err() {
                return;
            }
            if entry.reported.is_none() && entry.mdns.is_none() {
                table.remove(&fp);
            }
        }
    }
}

/// Best address first: private IPv4 (home Wi-Fi, hotspots), other IPv4,
/// then IPv6. Link-local IPv6 is dropped: it needs a scope ID to dial.
fn rank_addrs(addrs: Vec<IpAddr>) -> Vec<IpAddr> {
    let mut seen = HashSet::new();
    let mut addrs: Vec<IpAddr> = addrs
        .into_iter()
        .filter(|a| !a.is_unspecified() && !a.is_multicast())
        .filter(|a| !matches!(a, IpAddr::V6(v6) if v6.is_unicast_link_local()))
        .filter(|a| seen.insert(*a))
        .collect();
    addrs.sort_by_key(|a| match a {
        IpAddr::V4(v4) if v4.is_private() => 0,
        IpAddr::V4(v4) if v4.is_loopback() => 2,
        IpAddr::V4(_) => 1,
        IpAddr::V6(_) => 3,
    });
    addrs
}

// --- mDNS -------------------------------------------------------------------

fn start_mdns(
    me: &Announcement,
    sightings: mpsc::UnboundedSender<Sighting>,
) -> Result<(ServiceDaemon, String, JoinHandle<()>)> {
    let mdns_err = |e: mdns_sd::Error| crate::Error::Discovery(e.to_string());
    let daemon = ServiceDaemon::new().map_err(mdns_err)?;

    // The instance name must be unique on the network; the fingerprint is.
    let id = &me.fingerprint.to_string()[..16];
    let name = truncate(&me.device_name, MAX_ADVERTISED_NAME).to_owned();
    let properties = [
        ("fp", me.fingerprint.to_string()),
        ("n", name),
        ("pl", me.platform.clone()),
        ("v", PROTOCOL_VERSION.to_string()),
    ];
    let service = ServiceInfo::new(
        SERVICE_TYPE,
        &format!("nudge-{id}"),
        &format!("nudge-{id}.local."),
        "",
        me.port,
        &properties[..],
    )
    .map_err(mdns_err)?
    .enable_addr_auto();
    let fullname = service.get_fullname().to_owned();
    daemon.register(service).map_err(mdns_err)?;

    let browser = daemon.browse(SERVICE_TYPE).map_err(mdns_err)?;
    let task = tokio::spawn(async move {
        while let Ok(event) = browser.recv_async().await {
            let sighting = match event {
                ServiceEvent::ServiceResolved(info) => {
                    let Some(fp) = info
                        .get_property_val_str("fp")
                        .and_then(|s| s.parse::<Fingerprint>().ok())
                    else {
                        continue;
                    };
                    let text = |key| info.get_property_val_str(key).unwrap_or("").to_owned();
                    Sighting::Mdns {
                        fullname: info.get_fullname().to_owned(),
                        peer: DiscoveredPeer {
                            fingerprint: fp,
                            device_name: crate::identity::clean_name(&text("n")),
                            platform: text("pl"),
                            addrs: info
                                .get_addresses()
                                .iter()
                                .map(|a| a.to_ip_addr())
                                .collect(),
                            port: info.get_port(),
                        },
                    }
                }
                ServiceEvent::ServiceRemoved(_, fullname) => Sighting::MdnsGone { fullname },
                _ => continue,
            };
            if sightings.send(sighting).is_err() {
                return;
            }
        }
    });
    Ok((daemon, fullname, task))
}

// --- UDP beacons ------------------------------------------------------------

struct Beacon {
    socket: UdpSocket,
    me: Announcement,
    udp_port: u16,
    extra_targets: Vec<SocketAddr>,
}

impl Beacon {
    fn bind(me: &Announcement, config: &DiscoveryConfig) -> Result<Self> {
        // Prefer the shared port so others' broadcasts reach us. If another
        // app (or a second Nudge on this machine) holds it, a random port
        // still lets us query and hear the unicast replies.
        let socket = bind_udp(config.udp_port).or_else(|_| bind_udp(0))?;
        Ok(Self {
            socket,
            me: me.clone(),
            udp_port: config.udp_port,
            extra_targets: config.extra_targets.clone(),
        })
    }

    async fn run(self: Arc<Self>, sightings: mpsc::UnboundedSender<Sighting>) {
        self.send_all(KIND_QUERY);
        let mut tick = tokio::time::interval(BEACON_INTERVAL);
        let mut buf = [0u8; 1024];
        loop {
            tokio::select! {
                _ = tick.tick() => self.send_all(KIND_ANNOUNCE),
                received = self.socket.recv_from(&mut buf) => {
                    let Ok((len, from)) = received else { continue };
                    let Some((kind, mut peer)) = decode_beacon(&buf[..len]) else { continue };
                    if peer.fingerprint == self.me.fingerprint {
                        continue;
                    }
                    peer.addrs = vec![from.ip()];
                    let sighting = match kind {
                        KIND_GOODBYE => Sighting::BeaconGone(peer.fingerprint),
                        KIND_QUERY => {
                            let reply = encode_beacon(KIND_ANNOUNCE, &self.me);
                            let _ = self.socket.try_send_to(&reply, from);
                            Sighting::Beacon(peer)
                        }
                        _ => Sighting::Beacon(peer),
                    };
                    if sightings.send(sighting).is_err() {
                        return;
                    }
                }
            }
        }
    }

    /// Send to every interface's broadcast address. Best effort: a beacon
    /// is tiny and repeated, so failures are ignored.
    fn send_all(&self, kind: u8) {
        let packet = encode_beacon(kind, &self.me);
        for target in self.targets() {
            let _ = self.socket.try_send_to(&packet, target);
        }
    }

    fn targets(&self) -> Vec<SocketAddr> {
        let mut targets: Vec<SocketAddr> = if_addrs::get_if_addrs()
            .unwrap_or_default()
            .into_iter()
            .filter(|i| !i.is_loopback() && i.is_oper_up())
            .filter_map(|i| match i.addr {
                if_addrs::IfAddr::V4(v4) => v4.broadcast,
                if_addrs::IfAddr::V6(_) => None,
            })
            .chain([Ipv4Addr::BROADCAST])
            .map(|ip| SocketAddr::new(IpAddr::V4(ip), self.udp_port))
            .collect();
        targets.extend(self.extra_targets.iter().copied());
        targets.sort();
        targets.dedup();
        targets
    }
}

fn bind_udp(port: u16) -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_broadcast(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)).into())?;
    UdpSocket::from_std(socket.into())
}

/// Beacon layout: magic, version, kind, TCP port (u16), fingerprint (32),
/// then name and platform, each as u8 length + UTF-8.
fn encode_beacon(kind: u8, me: &Announcement) -> Vec<u8> {
    let name = truncate(&me.device_name, MAX_ADVERTISED_NAME);
    let platform = truncate(&me.platform, 32);
    let mut b = Vec::with_capacity(4 + 1 + 1 + 2 + 32 + 2 + name.len() + platform.len());
    b.extend_from_slice(&BEACON_MAGIC);
    b.push(BEACON_VERSION);
    b.push(kind);
    b.extend_from_slice(&me.port.to_be_bytes());
    b.extend_from_slice(me.fingerprint.as_bytes());
    for text in [name, platform] {
        b.push(text.len() as u8);
        b.extend_from_slice(text.as_bytes());
    }
    b
}

fn decode_beacon(b: &[u8]) -> Option<(u8, DiscoveredPeer)> {
    if b.len() < 40 || b[..4] != BEACON_MAGIC || b[4] != BEACON_VERSION {
        return None;
    }
    let kind = b[5];
    let port = u16::from_be_bytes([b[6], b[7]]);
    let fingerprint = Fingerprint::from_bytes(b[8..40].try_into().ok()?);
    let mut rest = &b[40..];
    let mut text = || -> Option<String> {
        let (&len, tail) = rest.split_first()?;
        let s = std::str::from_utf8(tail.get(..len as usize)?).ok()?;
        rest = &tail[len as usize..];
        Some(s.to_owned())
    };
    let device_name = crate::identity::clean_name(&text()?);
    let platform = text()?;
    Some((
        kind,
        DiscoveredPeer {
            fingerprint,
            device_name,
            platform,
            addrs: Vec::new(),
            port,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Identity;

    fn me(name: &str) -> Announcement {
        Announcement {
            fingerprint: Identity::generate().unwrap().fingerprint(),
            device_name: name.into(),
            platform: "windows".into(),
            port: 47_800,
        }
    }

    #[test]
    fn beacon_round_trips() {
        let me = me("Galaxy S25 Ultra");
        let packet = encode_beacon(KIND_QUERY, &me);
        let (kind, peer) = decode_beacon(&packet).unwrap();
        assert_eq!(kind, KIND_QUERY);
        assert_eq!(peer.fingerprint, me.fingerprint);
        assert_eq!(peer.device_name, me.device_name);
        assert_eq!(peer.platform, "windows");
        assert_eq!(peer.port, 47_800);
        // Truncated or foreign packets are ignored.
        assert!(decode_beacon(&packet[..packet.len() - 1]).is_none());
        assert!(decode_beacon(b"M-SEARCH * HTTP/1.1").is_none());
    }

    #[test]
    fn addresses_ranked() {
        let addrs = [
            "fe80::1",
            "2001:db8::1",
            "8.8.8.8",
            "192.168.1.5",
            "192.168.1.5",
        ]
        .map(|a| a.parse().unwrap())
        .to_vec();
        let ranked: Vec<String> = rank_addrs(addrs).iter().map(ToString::to_string).collect();
        assert_eq!(ranked, ["192.168.1.5", "8.8.8.8", "2001:db8::1"]);
    }

    /// Two devices on this machine find each other by beacon, and one
    /// leaving is noticed.
    #[tokio::test]
    async fn beacons_find_each_other() {
        let (port_a, port_b) = (free_udp_port(), free_udp_port());
        let config = |own: u16, other: u16| DiscoveryConfig {
            mdns: false,
            broadcast: true,
            udp_port: own,
            extra_targets: vec![SocketAddr::from((Ipv4Addr::LOCALHOST, other))],
        };
        let (a, b) = (me("laptop"), me("phone"));
        let (disc_a, mut events_a) = Discovery::start(a.clone(), config(port_a, port_b)).unwrap();
        let (disc_b, mut events_b) = Discovery::start(b.clone(), config(port_b, port_a)).unwrap();

        let found = next_event(&mut events_a).await;
        let DiscoveryEvent::Found(peer) = found else {
            panic!("expected Found, got {found:?}");
        };
        assert_eq!(peer.fingerprint, b.fingerprint);
        assert_eq!(peer.device_name, "phone");
        assert!(peer.addrs.contains(&IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(
            matches!(next_event(&mut events_b).await, DiscoveryEvent::Found(p) if p.fingerprint == a.fingerprint)
        );

        drop(disc_b);
        loop {
            match next_event(&mut events_a).await {
                DiscoveryEvent::Lost(fp) => {
                    assert_eq!(fp, b.fingerprint);
                    break;
                }
                DiscoveryEvent::Found(_) => {} // duplicate addresses settling
            }
        }
        drop(disc_a);
    }

    /// Real mDNS on this machine's interfaces. Ignored by default: CI
    /// runners often block multicast. Run with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn mdns_finds_each_other() {
        let config = DiscoveryConfig {
            mdns: true,
            broadcast: false,
            ..DiscoveryConfig::default()
        };
        let (a, b) = (me("mdns laptop"), me("mdns phone"));
        let (_disc_a, mut events_a) = Discovery::start(a, config.clone()).unwrap();
        let (_disc_b, _events_b) = Discovery::start(b.clone(), config).unwrap();
        loop {
            if let DiscoveryEvent::Found(peer) = next_event(&mut events_a).await
                && peer.fingerprint == b.fingerprint
            {
                assert_eq!(peer.device_name, "mdns phone");
                assert!(!peer.addrs.is_empty());
                break;
            }
        }
    }

    fn free_udp_port() -> u16 {
        std::net::UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    async fn next_event(rx: &mut mpsc::UnboundedReceiver<DiscoveryEvent>) -> DiscoveryEvent {
        tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("no discovery event within 10 s")
            .expect("discovery stopped")
    }
}
