//! Choosing and changing the network path to a device, for speed.
//!
//! - **Pick the fastest path up front.** Every address a device is known
//!   at (from discovery, from its own connections, from its hotspot) is
//!   ranked by the speed measured over that kind of path before, or a
//!   modest guess. A USB cable or Ethernet usually wins over Wi-Fi.
//! - **Move mid-transfer when it pays.** While a file is sending, its speed
//!   is measured. If a faster path is there (a cable was plugged in), or the
//!   phone's hotspot would be worth setting up, the rest moves over: the
//!   connection is dropped on purpose and the transfer resumes from where it
//!   was. It must be at least 20% faster and save at least 10 s.
//! - **The hotspot** (the phone hosts, the PC joins) is set up through the
//!   apps, which do the platform work, and taken down once both sides have
//!   been idle for a while. The user can turn it off (`set_direct_link`).

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use log::{info, warn};
use tokio::io::AsyncWriteExt;

use super::{Conn, NodeEvent, Outgoing, Shared, connect_any, send_over};
use crate::identity::Fingerprint;
use crate::link::{self, LinkKind, LocalNet};
use crate::protocol::{Batch, Hotspot, Link, LinkReply};
use crate::transfer::{Progress, TransferStats};
use crate::{DEFAULT_PORT, Error, Result};

/// Time to bring up the hotspot and move the PC onto it, counted against
/// the time it would save.
const LINK_SETUP: Duration = Duration::from_secs(10);

/// Time to reconnect over a path that's already there.
const SWITCH_SETUP: Duration = Duration::from_secs(1);

/// How often a running send checks for a faster path.
const CHECK_EVERY: Duration = Duration::from_secs(2);

/// A hotspot is taken down after this long with nothing moving.
const LINK_IDLE: Duration = Duration::from_secs(30);

/// After a hotspot attempt (worked or not), don't try again for this long.
const LINK_RETRY: Duration = Duration::from_secs(10 * 60);

/// A send that hasn't moved for this long is stuck on a path that's gone
/// (the other side moved to the hotspot, or left the network): it moves to
/// another path if there is one...
const STALL: Duration = Duration::from_secs(3);

/// ...or, with none, reconnects on the best one after this long.
const STALL_RECONNECT: Duration = Duration::from_secs(10);

/// A path that didn't connect when switched to isn't tried again for this long.
const DEAD_PATH: Duration = Duration::from_secs(5 * 60);

/// How long the app gets to start a hotspot, and to join one.
const HOST_WAIT: Duration = Duration::from_secs(20);
const JOIN_WAIT: Duration = Duration::from_secs(45);

/// Retries after a dropped connection: a few normally; many while the
/// network is moving to the hotspot, which takes the old one down.
const RETRIES: u32 = 3;
const RETRIES_LINKING: u32 = 20;

/// This side's part in a direct link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    Host,
    Join,
}

pub(super) struct DirectLink {
    pub(super) peer: Fingerprint,
    pub(super) role: Role,
    since: Instant,
}

/// Network details the app reports once its hotspot is up.
#[derive(Debug, Clone)]
pub struct HotspotInfo {
    pub ssid: String,
    pub passphrase: String,
    /// WPA3 only, no WPA2 fallback.
    pub wpa3: bool,
    /// Its frequency in MHz, 0 if the phone didn't say. For the log: 2.4 GHz
    /// (below 3000) is rarely faster than the router.
    pub mhz: u32,
}

impl Shared {
    /// Where to reach `fp`, fastest path first: discovery's addresses, the
    /// one it last connected from, and its hotspot's.
    pub(super) fn addrs_of(&self, fp: &Fingerprint) -> Vec<SocketAddr> {
        let (mut ips, port, extra) = {
            let state = self.state();
            let seen = state.nearby.get(fp).or_else(|| state.last_seen.get(fp));
            let port = seen.map_or(DEFAULT_PORT, |p| p.port);
            let mut ips: Vec<IpAddr> = seen.map(|p| p.addrs.clone()).unwrap_or_default();
            let extra = state.extra.get(fp).cloned().unwrap_or_default();
            ips.extend(extra.iter().copied());
            ips.extend(state.learned.get(fp).copied());
            (ips, port, extra)
        };
        let mut seen = std::collections::HashSet::new();
        ips.retain(|ip| seen.insert(*ip));
        let nets = link::local_nets();
        // Fastest expected first; addresses on no network of ours last (they
        // may still route), in the order they came.
        let mut ranked: Vec<(f64, IpAddr)> = ips
            .into_iter()
            .map(|ip| {
                let speed = self
                    .kind_toward(fp, &nets, ip, &extra)
                    .map_or(0.0, |kind| self.speeds.expected(fp, kind));
                (speed, ip)
            })
            .collect();
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
        ranked
            .into_iter()
            .map(|(_, ip)| SocketAddr::new(ip, port))
            .collect()
    }

    /// The kind of path to `fp` at `ip`. On the PC side a hotspot looks like
    /// any Wi-Fi; it's the hotspot when it's the one joined for this device.
    fn kind_toward(
        &self,
        fp: &Fingerprint,
        nets: &[LocalNet],
        ip: IpAddr,
        hotspot_addrs: &[IpAddr],
    ) -> Option<LinkKind> {
        let kind = link::kind_toward(nets, ip)?;
        Some(
            if kind == LinkKind::Wifi && hotspot_addrs.contains(&ip) && self.joined_with(fp) {
                LinkKind::Hotspot
            } else {
                kind
            },
        )
    }

    fn joined_with(&self, fp: &Fingerprint) -> bool {
        self.state()
            .link
            .as_ref()
            .is_some_and(|l| l.peer == *fp && l.role == Role::Join)
    }

    /// The kind of path an open connection takes.
    fn conn_kind(&self, fp: &Fingerprint, conn: &Conn) -> LinkKind {
        let (tcp, _) = conn.get_ref();
        let nets = link::local_nets();
        let local = tcp.local_addr().map(|a| a.ip());
        let peer = tcp.peer_addr().map(|a| a.ip());
        let kind = local.map_or(LinkKind::Other, |ip| link::kind_of_local(&nets, ip));
        let extra = self.state().extra.get(fp).cloned().unwrap_or_default();
        if kind == LinkKind::Wifi
            && self.joined_with(fp)
            && peer.is_ok_and(|ip| extra.contains(&ip))
        {
            LinkKind::Hotspot
        } else {
            kind
        }
    }

    /// Send one file, on the fastest path, moving it mid-file when a faster
    /// one turns up, and picking up after dropped connections. `more`: another
    /// file to the same device follows. `after`: bytes queued behind this file.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn send_one(
        self: &Arc<Self>,
        peer: Fingerprint,
        id: u64,
        item: Outgoing,
        batch: Batch,
        progress: &Progress,
        more: bool,
        after: u64,
    ) -> Result<TransferStats> {
        if !self.trusted.contains(&peer) {
            return Err(Error::UntrustedPeer(peer));
        }
        let started = Instant::now();
        let size = item.size();
        let mut item = item;
        let mut warmed = self.take_warm(peer).await;
        let mut warm_next = more;
        let mut failures = 0;
        let mut moved = 0u64;
        let mut first_offset: Option<u64> = None;
        // The path a deliberate move was for, to check it was reached.
        let mut wanted: Option<LinkKind> = None;
        loop {
            // A second handle to the file, so the send can go again on a new
            // connection; resume makes that safe partway through.
            let spare = item.try_clone().ok();
            let conn = match warmed.take() {
                Some(conn) => Ok(conn),
                None => connect_any(&self.tls, &self.addrs_of(&peer), peer).await,
            };
            let mut stream = match conn {
                Ok(stream) => stream,
                Err(e) => {
                    warn!("send {id}: couldn't connect ({e})");
                    if self.retry(peer, &mut failures, &e).await {
                        continue;
                    }
                    return Err(e);
                }
            };
            if std::mem::take(&mut warm_next) {
                self.warm(peer);
            }
            let kind = self.conn_kind(&peer, &stream);
            if let Some(want) = wanted.take()
                && want != kind
            {
                // Couldn't get onto it after all: don't keep trying.
                self.state().dead_paths.insert((peer, want), Instant::now());
            }
            self.emit(NodeEvent::TransferRoute { id, route: kind });
            let before = progress.position();
            info!(
                "send {id}: over {} to {}, from byte {before} of {size}",
                kind.name(),
                stream
                    .get_ref()
                    .0
                    .peer_addr()
                    .map_or("?".into(), |a| a.to_string())
            );
            let outcome = tokio::select! {
                result = send_over(&mut stream, item, batch, progress) => Some(result),
                target = self.watch_path(peer, kind, progress, size, after) => {
                    wanted = Some(target);
                    None
                }
            };
            match outcome {
                Some(Ok(stats)) => {
                    info!(
                        "send {id}: done over {}, {} MB in {:.1} s ({:.1} MB/s)",
                        kind.name(),
                        stats.bytes / 1_000_000,
                        stats.elapsed.as_secs_f64(),
                        stats.bytes as f64 / 1e6 / stats.elapsed.as_secs_f64().max(1e-3)
                    );
                    self.speeds.record(peer, kind, stats.bytes, stats.elapsed);
                    moved += stats.bytes;
                    return Ok(TransferStats {
                        bytes: moved,
                        resumed_from: first_offset.unwrap_or(stats.resumed_from),
                        elapsed: started.elapsed(),
                    });
                }
                Some(Err(e @ (Error::Io(_) | Error::Truncated { .. } | Error::Unreachable))) => {
                    warn!(
                        "send {id}: connection lost at byte {} ({e})",
                        progress.position()
                    );
                    first_offset.get_or_insert(before.min(progress.position()));
                    moved += progress.position().saturating_sub(before);
                    let Some(spare) = spare else {
                        return Err(e);
                    };
                    item = spare;
                    if !self.retry(peer, &mut failures, &e).await {
                        return Err(e);
                    }
                }
                Some(Err(e)) => {
                    warn!("send {id}: failed ({e})");
                    return Err(e);
                }
                None => {
                    info!("send {id}: moving at byte {}", progress.position());
                    // Moving to a faster path: drop this connection; the
                    // receiver keeps what arrived and the send resumes. The
                    // old path may be dead with a full send buffer, so don't
                    // wait long to say goodbye on it.
                    let _ = tokio::time::timeout(Duration::from_secs(1), stream.shutdown()).await;
                    first_offset.get_or_insert(before.min(progress.position()));
                    moved += progress.position().saturating_sub(before);
                    item = spare.ok_or(Error::Protocol("couldn't reopen the file to move it"))?;
                }
            }
        }
    }

    /// Wait a moment before trying again, or say it's time to give up.
    async fn retry(&self, peer: Fingerprint, failures: &mut u32, _error: &Error) -> bool {
        *failures += 1;
        let limit = if self.linking(&peer) {
            RETRIES_LINKING
        } else {
            RETRIES
        };
        if *failures > limit {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
        true
    }

    /// A direct link with `peer` was set up lately: the network may still be
    /// moving.
    fn linking(&self, peer: &Fingerprint) -> bool {
        self.state()
            .link
            .as_ref()
            .is_some_and(|l| l.peer == *peer && l.since.elapsed() < Duration::from_secs(60))
            || self
                .state()
                .link_tried
                .get(peer)
                .is_some_and(|t| t.elapsed() < Duration::from_secs(60))
    }

    /// Watch a running send. Finishes (with the path to move to) when a
    /// faster path to `peer` is there and worth moving to. Also starts the
    /// hotspot when that would pay off; the move then happens by itself, as
    /// the PC leaves the old network.
    async fn watch_path(
        self: &Arc<Self>,
        peer: Fingerprint,
        kind: LinkKind,
        progress: &Progress,
        size: u64,
        after: u64,
    ) -> LinkKind {
        // Both speeds known from before, and the direct link wins: start it
        // now rather than after a few seconds of measuring.
        if kind != LinkKind::Hotspot
            && let (Some(now), Some(direct)) = (
                self.speeds.measured(&peer, kind),
                self.speeds.measured(&peer, LinkKind::Hotspot),
            )
        {
            let remaining = size
                .saturating_sub(progress.position())
                .saturating_add(after);
            if self.may_link(&peer) && link::worth_switching(remaining, now, direct, LINK_SETUP) {
                info!(
                    "path: from history, {} {:.1} MB/s vs direct {:.1} MB/s: starting the direct link",
                    kind.name(),
                    now / 1e6,
                    direct / 1e6
                );
                self.start_link(peer);
            }
        }
        let mut tick = tokio::time::interval(CHECK_EVERY);
        tick.tick().await;
        let mut last = (progress.position(), Instant::now());
        let mut speed = 0.0;
        let mut samples = 0;
        let mut stalled_since: Option<Instant> = None;
        loop {
            tick.tick().await;
            let (pos, now) = (progress.position(), Instant::now());
            let secs = now.duration_since(last.1).as_secs_f64().max(1e-3);
            let sample = pos.saturating_sub(last.0) as f64 / secs;
            last = (pos, now);
            if sample <= 0.0 {
                // Nothing moved. All sent and waiting for the reply is fine;
                // otherwise the connection may be dead without an error yet
                // (TCP takes minutes to notice the other side left).
                if pos >= size {
                    continue;
                }
                let stalled = now.duration_since(*stalled_since.get_or_insert(now));
                if stalled >= STALL
                    && let Some(other) = self.other_path(peer, kind)
                {
                    info!(
                        "path: nothing moving over {} for {} s: moving to {}",
                        kind.name(),
                        stalled.as_secs(),
                        other.name()
                    );
                    return other;
                }
                if stalled >= STALL_RECONNECT {
                    info!(
                        "path: nothing moving over {} for {} s: reconnecting",
                        kind.name(),
                        stalled.as_secs()
                    );
                    return kind;
                }
                continue;
            }
            stalled_since = None;
            speed = if samples == 0 {
                sample
            } else {
                0.7 * speed + 0.3 * sample
            };
            samples += 1;
            // A few seconds of real numbers first.
            if samples < 2 {
                continue;
            }
            let remaining = size.saturating_sub(pos).saturating_add(after);
            if let Some(better) = self.better_path(peer, kind, speed, remaining) {
                info!(
                    "path: {} at {:.1} MB/s, {} MB left: moving to {}",
                    kind.name(),
                    speed / 1e6,
                    remaining / 1_000_000,
                    better.name()
                );
                return better;
            }
            let hotspot = self.hotspot_speed(&peer, speed);
            let may = kind != LinkKind::Hotspot && self.may_link(&peer);
            let pays = link::worth_switching(remaining, speed, hotspot, LINK_SETUP);
            info!(
                "path: {} at {:.1} MB/s, {} MB left; direct link would do {:.1} MB/s, allowed now: {may}, pays: {pays}",
                kind.name(),
                speed / 1e6,
                remaining / 1_000_000,
                hotspot / 1e6
            );
            if may && pays {
                self.start_link(peer);
            }
        }
    }

    /// A path to `peer`, other than the one in use, that's there now and
    /// worth moving the rest to.
    fn better_path(
        &self,
        peer: Fingerprint,
        current: LinkKind,
        speed: f64,
        remaining: u64,
    ) -> Option<LinkKind> {
        let nets = link::local_nets();
        let extra = self.state().extra.get(&peer).cloned().unwrap_or_default();
        let ips: Vec<IpAddr> = self.addrs_of(&peer).iter().map(SocketAddr::ip).collect();
        let dead = |kind: LinkKind| {
            self.state()
                .dead_paths
                .get(&(peer, kind))
                .is_some_and(|t| t.elapsed() < DEAD_PATH)
        };
        ips.into_iter()
            .filter_map(|ip| self.kind_toward(&peer, &nets, ip, &extra))
            .filter(|kind| *kind != current && !dead(*kind))
            .find(|kind| {
                let expected = self.speeds.expected(&peer, *kind);
                link::worth_switching(remaining, speed, expected, SWITCH_SETUP)
            })
    }

    /// Any path to `peer` other than `current` (and not known dead), fastest
    /// expected first: for when `current` has stopped moving.
    fn other_path(&self, peer: Fingerprint, current: LinkKind) -> Option<LinkKind> {
        let nets = link::local_nets();
        let extra = self.state().extra.get(&peer).cloned().unwrap_or_default();
        let dead = |kind: LinkKind| {
            self.state()
                .dead_paths
                .get(&(peer, kind))
                .is_some_and(|t| t.elapsed() < DEAD_PATH)
        };
        self.addrs_of(&peer)
            .iter()
            .filter_map(|a| self.kind_toward(&peer, &nets, a.ip(), &extra))
            .find(|kind| *kind != current && !dead(*kind))
    }

    /// What the hotspot would likely do for `peer`: as measured before, or
    /// twice the router path (one hop over the air instead of two), but no
    /// more than the modest guess for a hotspot. Phones often put their
    /// hotspot on 2.4 GHz (about 10 MB/s) while on Wi-Fi themselves, so a
    /// fast router path is never given up on a guess; a measurement decides.
    fn hotspot_speed(&self, peer: &Fingerprint, now: f64) -> f64 {
        self.speeds
            .measured(peer, LinkKind::Hotspot)
            .unwrap_or_else(|| (2.0 * now).min(self.speeds.expected(peer, LinkKind::Hotspot)))
    }

    /// Whether this side should start setting up a hotspot with `peer` now:
    /// allowed, a phone and a PC, none running, none tried lately.
    fn may_link(&self, peer: &Fingerprint) -> bool {
        if !self.direct_link.load(Ordering::Relaxed) {
            return false;
        }
        let state = self.state();
        let theirs = state
            .last_seen
            .get(peer)
            .map(|p| p.platform.as_str())
            .unwrap_or_default();
        let pair = can_host(&self.platform) && can_join(theirs)
            || can_join(&self.platform) && can_host(theirs);
        pair && state.link.is_none()
            && !state
                .link_tried
                .get(peer)
                .is_some_and(|t| t.elapsed() < LINK_RETRY)
    }

    /// Set up the hotspot with `peer`, in the background.
    fn start_link(self: &Arc<Self>, peer: Fingerprint) {
        self.state().link_tried.insert(peer, Instant::now());
        let shared = self.clone();
        tokio::spawn(async move {
            let hosting = can_host(&shared.platform);
            info!(
                "link: setting up the hotspot ({})",
                if hosting { "offering" } else { "asking" }
            );
            let result = if hosting {
                shared.offer_link(peer).await
            } else {
                shared.ask_link(peer).await
            };
            if let Err(e) = result {
                warn!("link: setup failed ({e})");
            }
        });
    }

    /// The PC side: ask the phone to start its hotspot, then join it.
    async fn ask_link(self: &Arc<Self>, peer: Fingerprint) -> Result<()> {
        let mut stream = connect_any(&self.tls, &self.addrs_of(&peer), peer).await?;
        Link::Ask.write_to(&mut stream).await?;
        let reply = tokio::time::timeout(
            HOST_WAIT + Duration::from_secs(10),
            LinkReply::read_from(&mut stream),
        )
        .await
        .map_err(|_| Error::NoAnswer)??;
        let _ = stream.shutdown().await;
        match reply {
            LinkReply::Join(hotspot) => {
                self.join_link(peer, hotspot).await;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// The phone side: offer the PC a hotspot; if it's willing, start one
    /// and tell it how to join.
    async fn offer_link(self: &Arc<Self>, peer: Fingerprint) -> Result<()> {
        let mut stream = connect_any(&self.tls, &self.addrs_of(&peer), peer).await?;
        Link::Offer.write_to(&mut stream).await?;
        let reply =
            tokio::time::timeout(Duration::from_secs(10), LinkReply::read_from(&mut stream))
                .await
                .map_err(|_| Error::NoAnswer)??;
        if reply != LinkReply::Ready {
            info!("link: the other side said no ({reply:?})");
            return Ok(());
        }
        let answer = match self.host_link(peer).await {
            Some(hotspot) => LinkReply::Join(hotspot),
            None => LinkReply::Refused,
        };
        // If starting the hotspot took this phone off the Wi-Fi, this can't
        // get through: the hotspot then closes again for lack of use.
        answer.write_to(&mut stream).await?;
        let _ = stream.shutdown().await;
        Ok(())
    }

    /// Answer a `Link` request from a paired device.
    pub(super) async fn answer_link<S>(
        self: &Arc<Self>,
        mut stream: S,
        peer: Fingerprint,
        request: Link,
    ) -> Result<()>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let allowed = self.direct_link.load(Ordering::Relaxed) && self.state().link.is_none();
        match request {
            Link::Ask if allowed && can_host(&self.platform) => {
                self.state().link_tried.insert(peer, Instant::now());
                let reply = match self.host_link(peer).await {
                    Some(hotspot) => LinkReply::Join(hotspot),
                    None => LinkReply::Refused,
                };
                reply.write_to(&mut stream).await?;
            }
            Link::Offer if allowed && can_join(&self.platform) => {
                self.state().link_tried.insert(peer, Instant::now());
                LinkReply::Ready.write_to(&mut stream).await?;
                let reply = tokio::time::timeout(
                    HOST_WAIT + Duration::from_secs(5),
                    LinkReply::read_from(&mut stream),
                )
                .await
                .map_err(|_| Error::NoAnswer)??;
                if let LinkReply::Join(hotspot) = reply {
                    let shared = self.clone();
                    tokio::spawn(async move { shared.join_link(peer, hotspot).await });
                }
            }
            _ => LinkReply::Refused.write_to(&mut stream).await?,
        }
        let _ = stream.shutdown().await;
        Ok(())
    }

    /// Have the app start a hotspot; returns how to join it.
    async fn host_link(self: &Arc<Self>, peer: Fingerprint) -> Option<Hotspot> {
        let before: Vec<_> = link::local_nets().iter().map(|n| n.ip).collect();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.state().hotspot_waits.insert(id, tx);
        self.emit(NodeEvent::HotspotWanted { id });
        let Some(info) = tokio::time::timeout(HOST_WAIT, rx).await.ok()?.ok()? else {
            warn!("link: the app couldn't start a hotspot");
            return None;
        };
        info!("link: {} is up at {} MHz", info.ssid, info.mhz);
        // The hotspot's own address appears shortly after it starts.
        let mut addrs = Vec::new();
        for _ in 0..15 {
            let nets = link::local_nets();
            addrs = nets
                .iter()
                .filter(|n| n.kind == LinkKind::Hotspot || !before.contains(&n.ip))
                .map(|n| n.ip)
                .collect();
            if !addrs.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        self.state().link = Some(DirectLink {
            peer,
            role: Role::Host,
            since: Instant::now(),
        });
        self.watch_link(peer);
        Some(Hotspot {
            ssid: info.ssid,
            passphrase: info.passphrase,
            wpa3: info.wpa3,
            addrs,
        })
    }

    /// Have the app join `peer`'s hotspot, then let it know where we are.
    async fn join_link(self: &Arc<Self>, peer: Fingerprint, hotspot: Hotspot) {
        let addrs: Vec<IpAddr> = hotspot.addrs.iter().map(|a| IpAddr::V4(*a)).collect();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut state = self.state();
            state.extra.insert(peer, addrs);
            state.join_waits.insert(id, tx);
            state.link = Some(DirectLink {
                peer,
                role: Role::Join,
                since: Instant::now(),
            });
        }
        self.emit(NodeEvent::JoinWanted {
            id,
            ssid: hotspot.ssid,
            passphrase: hotspot.passphrase,
            wpa3: hotspot.wpa3,
        });
        let joined = matches!(tokio::time::timeout(JOIN_WAIT, rx).await, Ok(Ok(true)));
        info!(
            "link: joining the hotspot {}",
            if joined { "worked" } else { "failed" }
        );
        if !joined {
            self.end_link(peer);
            return;
        }
        self.watch_link(peer);
        // Connecting tells the phone our address on its hotspot, so its
        // sends can reach us there. The new network can take a moment.
        for _ in 0..10 {
            if self.swap_status(peer).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// Take the link down once nothing has moved with `peer` for a while.
    fn watch_link(self: &Arc<Self>, peer: Fingerprint) {
        let shared = self.clone();
        tokio::spawn(async move {
            let mut idle_since = Instant::now();
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let (current, busy) = {
                    let state = shared.state();
                    let current = state.link.as_ref().is_some_and(|l| l.peer == peer);
                    let busy = state.active.values().any(|a| a.peer == peer);
                    (current, busy)
                };
                if !current {
                    return;
                }
                if busy {
                    idle_since = Instant::now();
                } else if idle_since.elapsed() >= LINK_IDLE {
                    shared.end_link(peer);
                    return;
                }
            }
        });
    }

    /// Back to normal: the phone stops its hotspot, the PC goes back to its
    /// usual network.
    fn end_link(&self, peer: Fingerprint) {
        let role = {
            let mut state = self.state();
            if !state.link.as_ref().is_some_and(|l| l.peer == peer) {
                return;
            }
            state.extra.remove(&peer);
            state.link.take().map(|l| l.role)
        };
        info!("link: ending the direct link");
        self.emit(match role {
            Some(Role::Host) => NodeEvent::HotspotDone,
            _ => NodeEvent::LeaveWanted,
        });
    }

    /// The app's answer to `HotspotWanted`.
    pub(super) fn hotspot_ready(&self, id: u64, info: Option<HotspotInfo>) {
        if let Some(tx) = self.state().hotspot_waits.remove(&id) {
            let _ = tx.send(info);
        }
    }

    /// The app's answer to `JoinWanted`.
    pub(super) fn joined(&self, id: u64, ok: bool) {
        if let Some(tx) = self.state().join_waits.remove(&id) {
            let _ = tx.send(ok);
        }
    }
}

/// Phones host the hotspot (Android can start one from an app).
fn can_host(platform: &str) -> bool {
    platform == "android"
}

/// PCs join it.
fn can_join(platform: &str) -> bool {
    platform == "windows"
}
