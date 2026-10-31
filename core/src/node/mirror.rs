//! Screen mirroring between paired devices: who may start a session, the
//! question on the device whose screen would be shared, and the sessions
//! running.
//!
//! The rules, enforced here so no feature built on top can skip them:
//! - only paired devices;
//! - the shared device's user always agrees: they're asked (`MirrorAsked`),
//!   or they started it themselves;
//! - a locked device never shares its screen: it's still asked (the
//!   question waits on the lock screen; answering needs an unlock), but a
//!   yes while it's locked is refused, and a sharing session pauses when it
//!   locks: nothing passes (no picture, no input) until it's unlocked, and
//!   after `LOCKED_LIMIT` locked the session ends;
//! - the app hears when a session starts and ends (`MirrorStarted`,
//!   `MirrorEnded`), so it can show a "being shared" indicator for as long
//!   as one runs.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use log::info;
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

use super::{Node, NodeEvent, Shared, connect_any};
use crate::identity::Fingerprint;
use crate::protocol::{Mirror, MirrorReply};
use crate::realtime::{Config, Control, Handle, Session};
use crate::{Error, Result};

/// How long the shared device's user has to answer. Covers Android's own
/// screen-capture prompt, which follows the question.
const ASK_TIMEOUT: Duration = Duration::from_secs(60);

/// A session the core started for the app (an answered request) is ended if
/// the app hasn't taken it by then: nothing is showing it.
const UNCLAIMED_TIMEOUT: Duration = Duration::from_secs(15);

/// A sharing session paused by the lock ends after this long locked.
const LOCKED_LIMIT: Duration = if cfg!(test) { Duration::from_secs(2) } else { Duration::from_secs(180) };

/// This device's part in a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirrorRole {
    /// Seeing the other device's screen.
    Viewing,
    /// Showing this device's screen.
    Sharing,
}

/// A session running, for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorInfo {
    pub id: u64,
    pub peer: Fingerprint,
    pub role: MirrorRole,
}

#[derive(Default)]
pub(super) struct Mirrors {
    /// Questions waiting for this device's user, by ID.
    asks: HashMap<u64, (Fingerprint, oneshot::Sender<bool>)>,
    running: HashMap<u64, Running>,
    /// Devices this one is asking to mirror with, until they answer.
    starting: std::collections::HashSet<Fingerprint>,
}

impl Mirrors {
    /// Sessions running with `peer`.
    pub(super) fn running_with(&self, peer: &Fingerprint) -> Vec<u64> {
        self.running.iter().filter(|(_, r)| r.peer == *peer).map(|(&id, _)| id).collect()
    }

    pub(super) fn close(&self, id: u64) {
        if let Some(r) = self.running.get(&id) {
            r.handle.close();
        }
    }
}

struct Running {
    peer: Fingerprint,
    role: MirrorRole,
    handle: Handle,
    /// Started by the core, waiting for the app to take it.
    unclaimed: Option<Session>,
}

impl Node {
    /// Starts mirroring with a paired device: `Viewing` asks to see its
    /// screen (its user is asked), `Sharing` shows this device's screen to
    /// it. Returns the session and its ID (as in `MirrorStarted`/`MirrorEnded`).
    pub async fn start_mirror(
        &self,
        peer: Fingerprint,
        role: MirrorRole,
        config: Config,
    ) -> Result<(u64, Session)> {
        let shared = &self.shared;
        if !shared.trusted.contains(&peer) {
            return Err(Error::UntrustedPeer(peer));
        }
        if role == MirrorRole::Sharing && shared.screen_locked.load(Ordering::Relaxed) {
            return Err(Error::ScreenLocked);
        }
        shared.mirrors().starting.insert(peer);
        let started = Self::ask_to_mirror(shared, peer, role, config).await;
        shared.mirrors().starting.remove(&peer);
        started
    }

    async fn ask_to_mirror(
        shared: &Arc<Shared>,
        peer: Fingerprint,
        role: MirrorRole,
        config: Config,
    ) -> Result<(u64, Session)> {
        // No network change for mirroring: the PC stays on its own Wi-Fi
        // (moving it onto the phone's Wi-Fi Direct group took it off the
        // internet, and a refused request left it there for nothing). The
        // session takes the best path there is: a cable, else the router.
        let mut stream = connect_any(&shared.tls, &shared.addrs_of(&peer), peer).await?;
        match role {
            MirrorRole::Viewing => Mirror::ShowMe,
            MirrorRole::Sharing => Mirror::ShowYou,
        }
        .write_to(&mut stream)
        .await?;
        // The other side may be asking its user: give it time to.
        let reply = tokio::time::timeout(
            ASK_TIMEOUT + Duration::from_secs(5),
            MirrorReply::read_from(&mut stream),
        )
        .await
        .map_err(|_| Error::NoAnswer)?;
        let reply = match reply {
            Ok(r) => r,
            // Builds from before mirroring close the connection on it.
            Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(Error::PeerOutdated);
            }
            Err(e) => return Err(e),
        };
        let failed = match reply {
            MirrorReply::Accepted => None,
            MirrorReply::Declined => Some(Error::Declined),
            MirrorReply::NoAnswer => Some(Error::NoAnswer),
            MirrorReply::Locked => Some(Error::PeerLocked),
            MirrorReply::Busy => Some(Error::MirrorBusy),
        };
        if let Some(e) = failed {
            let _ = stream.shutdown().await;
            return Err(e);
        }
        // Locked while the other side was answering.
        if role == MirrorRole::Sharing && shared.screen_locked.load(Ordering::Relaxed) {
            let _ = stream.shutdown().await;
            return Err(Error::ScreenLocked);
        }
        let session = Session::open(stream, config).await?;
        let id = shared.next_id.fetch_add(1, Ordering::Relaxed);
        shared.add_mirror(id, peer, role, session.handle(), None);
        Ok((id, session))
    }

    /// Answers `NodeEvent::MirrorAsked`. On yes the session starts and
    /// `MirrorStarted` follows: take it with [`Node::take_mirror`].
    pub fn answer_mirror(&self, id: u64, accept: bool) {
        let ask = self.shared.mirrors().asks.remove(&id);
        if let Some((_, answer)) = ask {
            let _ = answer.send(accept);
        }
    }

    /// The session behind a `MirrorStarted` the core started (an answered
    /// request). Ended if not taken soon. `None` if already taken, ended, or
    /// returned by `start_mirror` instead.
    pub fn take_mirror(&self, id: u64) -> Option<Session> {
        self.shared.mirrors().running.get_mut(&id)?.unclaimed.take()
    }

    /// Ends a session, from either side.
    pub fn stop_mirror(&self, id: u64) {
        if let Some(r) = self.shared.mirrors().running.get(&id) {
            r.handle.close();
        }
    }

    /// Sessions running now.
    pub fn mirrors(&self) -> Vec<MirrorInfo> {
        let mirrors = self.shared.mirrors();
        let mut all: Vec<MirrorInfo> = mirrors
            .running
            .iter()
            .filter(|(_, r)| !r.handle.is_closed())
            .map(|(&id, r)| MirrorInfo {
                id,
                peer: r.peer,
                role: r.role,
            })
            .collect();
        all.sort_by_key(|m| m.id);
        all
    }

    /// Whether this device is locked (or its screen is off). While it is,
    /// its screen is never shared: requests are refused, and sharing
    /// sessions pause (the other side is told, `Input::Locked`) until it's
    /// unlocked, ending after `LOCKED_LIMIT`. Not saved: the app sets it as
    /// things change.
    pub fn set_screen_locked(&self, locked: bool) {
        let was = self.shared.screen_locked.swap(locked, Ordering::Relaxed);
        if was == locked {
            return;
        }
        let mirrors = self.shared.mirrors();
        for r in mirrors.running.values().filter(|r| r.role == MirrorRole::Sharing) {
            if locked {
                // Paused first: nothing more gets out from here on.
                r.handle.pause(Some(LOCKED_LIMIT));
            }
            let _ = r.handle.send_message(crate::input::STREAM, crate::input::Input::Locked(locked).encode());
            if !locked {
                r.handle.pause(None);
            }
        }
        // Open questions stay: answering one needs an unlock first, and a
        // yes that comes while still locked is refused (`ask_to_share`).
    }
}

impl Shared {
    pub(super) fn mirrors(&self) -> std::sync::MutexGuard<'_, Mirrors> {
        self.mirrors.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A session with `peer` is running, or being asked for.
    pub(super) fn mirroring_with(&self, peer: &Fingerprint) -> bool {
        let m = self.mirrors();
        m.starting.contains(peer)
            || m.asks.values().any(|(p, _)| p == peer)
            || m.running.values().any(|r| r.peer == *peer && !r.handle.is_closed())
    }

    /// A `Mirror` request from a paired device.
    pub(super) async fn answer_mirror_request<S: Control>(
        self: &Arc<Self>,
        mut stream: S,
        peer: Fingerprint,
        request: Mirror,
    ) -> Result<()> {
        let role = match request {
            Mirror::ShowMe => MirrorRole::Sharing,
            Mirror::ShowYou => MirrorRole::Viewing,
        };
        if role == MirrorRole::Sharing {
            if let Some(no) = self.ask_to_share(peer).await {
                no.write_to(&mut stream).await?;
                let _ = stream.shutdown().await;
                return Ok(());
            }
        }
        // Viewing a paired device's screen needs no question: its user
        // chose to show it, and the window that opens can be closed.
        MirrorReply::Accepted.write_to(&mut stream).await?;
        let session = Session::accept(stream, Config::default()).await?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        info!("mirroring with {peer} ({role:?}), session {id}");
        self.add_mirror(id, peer, role, session.handle(), Some(session));
        Ok(())
    }

    /// Asks this device's user whether `peer` may see the screen. `None`:
    /// yes; otherwise the reply saying why not.
    /// Locked (or the screen off: the phone is on the desk) is fine for
    /// asking: the user picks it up, unlocks, and answers.
    async fn ask_to_share(self: &Arc<Self>, peer: Fingerprint) -> Option<MirrorReply> {
        let (tx, rx) = oneshot::channel();
        let id = {
            let mut mirrors = self.mirrors();
            // One question per device at a time.
            if mirrors.asks.values().any(|(p, _)| *p == peer) {
                return Some(MirrorReply::Busy);
            }
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            mirrors.asks.insert(id, (peer, tx));
            id
        };
        self.emit(NodeEvent::MirrorAsked { id, peer });
        let answer = tokio::time::timeout(ASK_TIMEOUT, rx).await;
        self.mirrors().asks.remove(&id);
        self.emit(NodeEvent::OfferClosed { id });
        match answer {
            _ if self.screen_locked.load(Ordering::Relaxed) => Some(MirrorReply::Locked),
            Ok(Ok(true)) => None,
            Ok(_) => Some(MirrorReply::Declined),
            Err(_) => Some(MirrorReply::NoAnswer),
        }
    }

    /// Lists a session that just started, and tells the app; `MirrorEnded`
    /// follows when it ends.
    fn add_mirror(
        self: &Arc<Self>,
        id: u64,
        peer: Fingerprint,
        role: MirrorRole,
        handle: Handle,
        unclaimed: Option<Session>,
    ) {
        let waits = unclaimed.is_some();
        self.mirrors().running.insert(
            id,
            Running {
                peer,
                role,
                handle: handle.clone(),
                unclaimed,
            },
        );
        self.emit(NodeEvent::MirrorStarted { id, peer, role });
        let shared = self.clone();
        tokio::spawn(async move {
            if waits {
                tokio::select! {
                    () = handle.closed() => {}
                    () = tokio::time::sleep(UNCLAIMED_TIMEOUT) => {
                        // Dropping it ends it.
                        let left = shared.mirrors().running.get_mut(&id).and_then(|r| r.unclaimed.take());
                        drop(left);
                    }
                }
            }
            handle.closed().await;
            shared.mirrors().running.remove(&id);
            info!("mirroring session {id} ended");
            shared.emit(NodeEvent::MirrorEnded { id });
        });
    }
}
