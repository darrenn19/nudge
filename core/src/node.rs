//! `Node`: everything a Nudge app needs, behind one object.
//!
//! A node listens for connections, finds nearby devices, pairs by QR code,
//! sends files and accepts files from paired devices without asking. Apps
//! call its methods and react to the `NodeEvent`s it emits; they never touch
//! sockets or TLS themselves.
//!
//! Must be started inside a tokio runtime.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::discovery::{Announcement, DiscoveredPeer, Discovery, DiscoveryConfig, DiscoveryEvent};
use crate::identity::{Fingerprint, Identity, TrustedPeers, clean_name};
use crate::pairing::{self, CODE_LIFETIME, PairingCode};
use crate::protocol::{Batch, PairAsk, PairReply, PairRequest, Request, Response, Status};
use crate::transfer::{
    Progress, TransferStats, receive_offered, send_file_tracked, send_open_file_tracked,
};
use crate::{DEFAULT_PORT, Error, Result, Tls};

mod web;
pub use web::WEB_PORT;

/// How long to wait for each address when connecting.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// How long a new connection may take to say what it wants.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// The same, for a paired device: long enough for a connection it opened
/// ahead of time (`Node::warm`) to wait while the user picks a device.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a connection opened by `Node::warm` is kept for a send. Shorter
/// than the other side's `IDLE_TIMEOUT`, so it's still open when used.
const WARM_LIFETIME: Duration = Duration::from_secs(45);

/// How long a "do you want these files?" question waits for an answer.
/// The sender shows "pending" meanwhile, so this can be generous.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How long a "can we pair?" question waits.
const PAIR_ASK_TIMEOUT: Duration = Duration::from_secs(90);

/// How long an answer covers the rest of its batch.
const ANSWER_LIFETIME: Duration = Duration::from_secs(30 * 60);

/// How often progress events are sent for running transfers.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// Settings for starting a node.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// Folder for this device's identity and paired-device list.
    pub data_dir: PathBuf,
    /// Folder received files are saved into (created if missing).
    pub download_dir: PathBuf,
    /// Name other devices see, e.g. "Darren's Galaxy S25".
    pub device_name: String,
    /// Short platform tag: "windows", "android", "macos", "linux", "ios".
    pub platform: String,
    /// TCP port to listen on. `None` tries `DEFAULT_PORT`, then any free port.
    pub port: Option<u16>,
    /// Address to listen on; all interfaces by default.
    pub bind: IpAddr,
    pub discovery: DiscoveryConfig,
    /// How long a paired device may hold a connection open before saying
    /// what it wants (see `Node::warm`).
    pub idle_timeout: Duration,
    /// Ask before receiving from a paired device (`NodeEvent::OfferReceived`)
    /// unless it's set to auto-accept. Off: everything from a paired
    /// device is accepted (the command-line tool).
    pub ask_before_receiving: bool,
}

impl NodeConfig {
    pub fn new(
        data_dir: PathBuf,
        download_dir: PathBuf,
        device_name: &str,
        platform: &str,
    ) -> Self {
        Self {
            data_dir,
            download_dir,
            device_name: device_name.to_owned(),
            platform: platform.to_owned(),
            port: None,
            bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            discovery: DiscoveryConfig::default(),
            idle_timeout: IDLE_TIMEOUT,
            ask_before_receiving: true,
        }
    }
}

/// A device the app can show: paired, nearby, or both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub fingerprint: Fingerprint,
    pub device_name: String,
    /// Empty if unknown (paired but not seen this session).
    pub platform: String,
    /// Paired devices exchange files without prompts.
    pub paired: bool,
    /// Seen on the network right now.
    pub online: bool,
    /// Worth trying to send to: nearby, or seen earlier with an address to
    /// try. A phone with its screen off stops announcing itself but can
    /// still receive, so a paired device stays reachable after it goes quiet.
    pub reachable: bool,
    /// Files from it are accepted without asking.
    pub auto_accept: bool,
    /// It accepts files from this device without asking. `None` until
    /// it's said.
    pub accepts_from_us: Option<bool>,
}

/// A file to send.
#[derive(Debug)]
pub enum Outgoing {
    Path(PathBuf),
    /// Already open, sent under `name`. Android shares files this way: the
    /// app gets an open descriptor, not a path, and reading it directly
    /// avoids copying the file first.
    Open {
        file: std::fs::File,
        name: String,
    },
}

impl Outgoing {
    fn name(&self) -> String {
        match self {
            Self::Path(path) => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Self::Open { name, .. } => name.clone(),
        }
    }

    fn size(&self) -> u64 {
        match self {
            Self::Path(path) => std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
            Self::Open { file, .. } => file.metadata().map(|m| m.len()).unwrap_or(0),
        }
    }

    /// A second handle to the same file, for a retry.
    fn try_clone(&self) -> std::io::Result<Self> {
        Ok(match self {
            Self::Path(path) => Self::Path(path.clone()),
            Self::Open { file, name } => Self::Open {
                file: file.try_clone()?,
                name: name.clone(),
            },
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Outgoing,
    Incoming,
}

/// Things the app should react to.
#[derive(Debug, Clone)]
pub enum NodeEvent {
    /// The device list changed (someone appeared, left, paired or unpaired).
    /// Re-read it with `Node::peers`.
    PeersChanged,
    /// A pairing finished, from either side.
    Paired {
        peer: Peer,
    },
    /// The pairing code shown on this device expired or was used.
    PairingClosed,
    TransferStarted {
        id: u64,
        peer: Fingerprint,
        direction: Direction,
        file_name: String,
        size: u64,
    },
    TransferProgress {
        id: u64,
        /// Bytes done, from the start of the file.
        bytes: u64,
        size: u64,
        /// Recent speed in bytes per second.
        bytes_per_sec: f64,
    },
    TransferDone {
        id: u64,
        /// Where an incoming file was saved; the source for outgoing ones.
        path: PathBuf,
        stats: TransferStats,
    },
    TransferFailed {
        id: u64,
        error: String,
    },
    /// Browser sharing stopped (by `stop_web_share`, or after a long spell
    /// with no browser connected).
    WebShareStopped,
    /// A paired device wants to send files: answer with `Node::answer_offer`
    /// within a minute, or it's turned down. One question per send, however
    /// many files.
    OfferReceived {
        id: u64,
        peer: Fingerprint,
        /// The first file's name.
        file_name: String,
        count: u32,
        size: u64,
    },
    /// That question was answered or timed out: stop showing it.
    OfferClosed {
        id: u64,
    },
    /// The receiver said yes to an outgoing transfer.
    TransferAccepted {
        id: u64,
    },
    /// The receiver has the offer and is asking its user: the send is
    /// delivered, the answer to come.
    TransferPending {
        id: u64,
    },
    /// A nearby device wants to pair without a QR code. Check both screens
    /// show `code`, then answer with `Node::answer_pair` (closed with
    /// `OfferClosed` if it times out).
    PairAsked {
        id: u64,
        peer: Fingerprint,
        device_name: String,
        code: String,
    },
}

/// A running Nudge device. Dropping it stops everything.
pub struct Node {
    shared: Arc<Shared>,
    discovery: Option<Discovery>,
    tasks: Vec<JoinHandle<()>>,
}

type Conn = tokio_rustls::client::TlsStream<TcpStream>;

/// A connection opened ahead of a send, and when it opened. Locked while
/// it's being opened, so a send that arrives meanwhile waits for it rather
/// than opening a second one.
type WarmSlot = Arc<tokio::sync::Mutex<Option<(Conn, Instant)>>>;

struct Shared {
    identity: Identity,
    fingerprint: Fingerprint,
    device_name: String,
    data_dir: PathBuf,
    download_dir: Mutex<PathBuf>,
    port: u16,
    trusted: TrustedPeers,
    tls: Tls,
    events: mpsc::UnboundedSender<NodeEvent>,
    state: Mutex<State>,
    next_id: AtomicU64,
    idle_timeout: Duration,
    /// Address the node listens on; browser sharing listens there too.
    bind: IpAddr,
    /// "windows", "android", ...: tells the web page which icon to show.
    platform: String,
    ask: bool,
    /// Paired devices whose files are accepted without asking.
    auto_accept: Mutex<HashSet<Fingerprint>>,
}

#[derive(Default)]
struct State {
    nearby: HashMap<Fingerprint, DiscoveredPeer>,
    /// The last sighting of every device seen this session, kept after it
    /// goes quiet: where to try a sleeping phone.
    last_seen: HashMap<Fingerprint, DiscoveredPeer>,
    /// Sharing with browsers, while it's on.
    web: Option<web::WebShare>,
    /// The code this device is showing, and when it stops working.
    pairing: Option<(PairingCode, Instant)>,
    active: HashMap<u64, Active>,
    /// Questions waiting for the user, by ID, with the batch they're about.
    questions: HashMap<u64, (BatchKey, tokio::sync::watch::Sender<Option<bool>>)>,
    /// The answer to each batch asked about, so the rest of its files
    /// follow it; `None` while still waiting.
    answers: HashMap<BatchKey, (tokio::sync::watch::Receiver<Option<bool>>, Instant)>,
    /// "Can we pair?" questions waiting for the user, by ID.
    pair_asks: HashMap<u64, (Fingerprint, tokio::sync::oneshot::Sender<bool>)>,
    /// Whether each paired device accepts our files without asking, as it
    /// last said; and the ones we've asked this session.
    remote_auto: HashMap<Fingerprint, bool>,
    status_asked: HashSet<Fingerprint>,
    /// Connections opened ahead of a send, one per device at most.
    warm: HashMap<Fingerprint, WarmSlot>,
}

/// A running transfer, for progress reporting.
struct Active {
    progress: Progress,
    size: u64,
    last: (u64, Instant),
    /// `TransferAccepted` / `TransferPending` sent.
    told_accepted: bool,
    told_pending: bool,
}

/// A batch from a peer. Single files (batch ID 0) get one of their own.
type BatchKey = (Fingerprint, u64);

/// Where a decision to take files stands.
enum Asking {
    /// Decided (`None`: nobody answered in time).
    Now(Option<bool>),
    /// The user is being asked.
    Wait(BatchKey, tokio::sync::watch::Receiver<Option<bool>>),
}

impl Node {
    /// Load (or create) this device's identity, start listening and start
    /// discovery. Events arrive on the returned channel.
    pub async fn start(config: NodeConfig) -> Result<(Self, mpsc::UnboundedReceiver<NodeEvent>)> {
        let identity = Identity::load_or_create(&config.data_dir)?;
        let trusted = TrustedPeers::load(&config.data_dir)?;
        let tls = Tls::new(&identity, trusted.clone())?;
        // Any device may connect, but an unpaired one can only ask to pair
        // (`serve` checks every request), so nearby devices can pair
        // without a QR code.
        tls.set_pairing_open(true);
        let fingerprint = identity.fingerprint();

        let listener = match config.port {
            Some(port) => TcpListener::bind((config.bind, port)).await?,
            None => match TcpListener::bind((config.bind, DEFAULT_PORT)).await {
                Ok(l) => l,
                Err(_) => TcpListener::bind((config.bind, 0)).await?,
            },
        };
        let port = listener.local_addr()?.port();

        let auto_accept = load_auto_accept(&config.data_dir);
        let device_name = clean_name(&config.device_name);
        let (events, events_rx) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared {
            identity,
            fingerprint,
            device_name: device_name.clone(),
            data_dir: config.data_dir,
            download_dir: Mutex::new(config.download_dir),
            port,
            trusted,
            tls,
            events,
            state: Mutex::default(),
            next_id: AtomicU64::new(1),
            idle_timeout: config.idle_timeout,
            bind: config.bind,
            platform: config.platform.clone(),
            ask: config.ask_before_receiving,
            auto_accept: Mutex::new(auto_accept),
        });

        let mut tasks = vec![
            tokio::spawn(accept_loop(shared.clone(), listener)),
            tokio::spawn(progress_loop(shared.clone())),
        ];

        // Discovery is best effort: without it the app still works for
        // pairing (the QR code carries addresses) and for incoming files.
        let announcement = Announcement {
            fingerprint,
            device_name,
            platform: config.platform,
            port,
        };
        let discovery = match Discovery::start(announcement, config.discovery) {
            Ok((discovery, rx)) => {
                tasks.push(tokio::spawn(discovery_loop(shared.clone(), rx)));
                Some(discovery)
            }
            Err(_) => None,
        };

        Ok((
            Self {
                shared,
                discovery,
                tasks,
            },
            events_rx,
        ))
    }

    pub fn fingerprint(&self) -> Fingerprint {
        self.shared.fingerprint
    }

    pub fn device_name(&self) -> &str {
        &self.shared.device_name
    }

    /// TCP port this node listens on.
    pub fn port(&self) -> u16 {
        self.shared.port
    }

    pub fn download_dir(&self) -> PathBuf {
        self.shared
            .download_dir
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn set_download_dir(&self, dir: PathBuf) {
        *self
            .shared
            .download_dir
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = dir;
    }

    /// Whether discovery is running (it can fail on locked-down networks).
    pub fn discovery_running(&self) -> bool {
        self.discovery.is_some()
    }

    /// Paired devices first (online ones on top), then unpaired nearby ones,
    /// each group sorted by name.
    pub fn peers(&self) -> Vec<Peer> {
        let state = self.shared.state();
        let mut peers: Vec<Peer> = self
            .shared
            .trusted
            .list()
            .into_iter()
            .map(|(fp, name)| {
                let seen = state.last_seen.get(&fp);
                Peer {
                    fingerprint: fp,
                    device_name: if name.is_empty() {
                        seen.map_or_else(|| fp.short(), |n| n.device_name.clone())
                    } else {
                        name
                    },
                    platform: seen.map(|n| n.platform.clone()).unwrap_or_default(),
                    paired: true,
                    online: state.nearby.contains_key(&fp),
                    reachable: seen.is_some(),
                    auto_accept: self.shared.auto_accepts(&fp),
                    accepts_from_us: state.remote_auto.get(&fp).copied(),
                }
            })
            .collect();
        peers.extend(
            state
                .nearby
                .values()
                .filter(|n| !self.shared.trusted.contains(&n.fingerprint))
                .map(|n| Peer {
                    fingerprint: n.fingerprint,
                    device_name: n.device_name.clone(),
                    platform: n.platform.clone(),
                    paired: false,
                    online: true,
                    reachable: true,
                    auto_accept: false,
                    accepts_from_us: None,
                }),
        );
        if let Some(web) = &state.web {
            peers.extend(web.browsers.iter().map(|(id, tab)| Peer {
                fingerprint: *id,
                device_name: tab.name.clone(),
                platform: "browser".into(),
                paired: false,
                online: tab.connected(),
                reachable: true,
                auto_accept: false,
                accepts_from_us: None,
            }));
        }
        peers.sort_by(|a, b| {
            (!a.paired, !a.online, a.device_name.to_lowercase()).cmp(&(
                !b.paired,
                !b.online,
                b.device_name.to_lowercase(),
            ))
        });
        peers
    }

    /// Ask nearby devices to announce themselves now.
    pub fn refresh(&self) {
        if let Some(d) = &self.discovery {
            d.refresh();
        }
    }

    /// Start showing a pairing code. Returns the text to put in the QR code.
    /// Any earlier code stops working. The code works once and expires after
    /// `CODE_LIFETIME`; `NodeEvent::PairingClosed` is sent when it does.
    pub fn start_pairing(&self) -> String {
        let code = PairingCode::new(
            self.shared.fingerprint,
            &self.shared.device_name,
            local_addrs(),
            self.shared.port,
        );
        let uri = code.to_uri();
        let deadline = Instant::now() + CODE_LIFETIME;
        self.shared.state().pairing = Some((code, deadline));
        self.shared.tls.set_pairing_open(true);

        let shared = self.shared.clone();
        tokio::spawn(async move {
            tokio::time::sleep_until(deadline.into()).await;
            let mut state = shared.state();
            if state.pairing.as_ref().is_some_and(|(_, d)| *d == deadline) {
                state.pairing = None;
                drop(state);
                shared.close_pairing();
            }
        });
        uri
    }

    /// Stop showing the pairing code early.
    pub fn cancel_pairing(&self) {
        if self.shared.state().pairing.take().is_some() {
            self.shared.close_pairing();
        }
    }

    /// Pair with the device whose code was scanned. On success both devices
    /// trust each other from now on.
    pub async fn pair(&self, scanned: &str) -> Result<Peer> {
        let code = PairingCode::parse(scanned)?;
        if code.fingerprint == self.shared.fingerprint {
            return Err(Error::BadPairingCode(
                "that's this device's own code".into(),
            ));
        }
        // Trust exactly the device in the code, for this one connection.
        let tls = Tls::new(&self.shared.identity, TrustedPeers::new([code.fingerprint]))?;
        let mut addrs: Vec<SocketAddr> = code
            .addrs
            .iter()
            .map(|ip| SocketAddr::new(*ip, code.port))
            .collect();
        addrs.extend(self.shared.addrs_of(&code.fingerprint));
        let mut stream = connect_any(&tls, &addrs, code.fingerprint).await?;

        PairRequest {
            proof: pairing::proof(&code.secret, &self.shared.fingerprint, &code.fingerprint),
            device_name: self.shared.device_name.clone(),
        }
        .write_to(&mut stream)
        .await?;
        let name = match PairReply::read_from(&mut stream).await? {
            PairReply::Accept { device_name } => clean_name(&device_name),
            PairReply::Reject => return Err(Error::PairingRejected),
        };
        let _ = stream.shutdown().await;

        let name = if name.is_empty() {
            code.device_name
        } else {
            name
        };
        self.shared.trust(code.fingerprint, &name)?;
        let peer = self.shared.peer(code.fingerprint);
        Ok(peer)
    }

    /// Forget a paired device. It will need a new QR pairing to send again.
    pub fn unpair(&self, peer: &Fingerprint) -> Result<()> {
        if self.shared.trusted.remove(peer) {
            self.shared.trusted.save(&self.shared.data_dir)?;
            self.shared.set_auto_accept(peer, false)?;
            self.shared.emit(NodeEvent::PeersChanged);
        }
        Ok(())
    }

    /// Accept files from a paired device without asking (or ask again).
    /// Tells that device, so it can show it.
    pub fn set_auto_accept(&self, peer: &Fingerprint, on: bool) -> Result<()> {
        self.shared.set_auto_accept(peer, on)?;
        self.shared.exchange_status(*peer);
        self.shared.emit(NodeEvent::PeersChanged);
        Ok(())
    }

    /// The code both screens show while pairing with `peer` without a QR
    /// code.
    pub fn pair_code(&self, peer: &Fingerprint) -> String {
        pairing::compare_code(&self.shared.fingerprint, peer)
    }

    /// Pair with a device that's nearby, without a QR code: it asks its
    /// user, who checks both screens show `pair_code`. Waits for the answer.
    pub async fn pair_nearby(&self, peer: Fingerprint) -> Result<Peer> {
        let shared = &self.shared;
        // Trust exactly that device, for this one connection.
        let tls = Tls::new(&shared.identity, TrustedPeers::new([peer]))?;
        let mut stream = connect_any(&tls, &shared.addrs_of(&peer), peer).await?;
        PairAsk {
            device_name: shared.device_name.clone(),
        }
        .write_to(&mut stream)
        .await?;
        let reply = tokio::time::timeout(
            PAIR_ASK_TIMEOUT + Duration::from_secs(5),
            PairReply::read_from(&mut stream),
        )
        .await
        .map_err(|_| Error::PairingRejected)??;
        let name = match reply {
            PairReply::Accept { device_name } => clean_name(&device_name),
            PairReply::Reject => return Err(Error::PairingRejected),
        };
        let _ = stream.shutdown().await;
        shared.trust(peer, &name)?;
        Ok(shared.peer(peer))
    }

    /// Answer `NodeEvent::PairAsked`.
    pub fn answer_pair(&self, id: u64, accept: bool) {
        if let Some((_, answer)) = self.shared.state().pair_asks.remove(&id) {
            let _ = answer.send(accept);
        }
        self.shared.emit(NodeEvent::OfferClosed { id });
    }

    /// Answer `NodeEvent::OfferReceived`. With `always`, files from that
    /// device are accepted without asking from now on.
    pub fn answer_offer(&self, id: u64, accept: bool, always: bool) -> Result<()> {
        let question = self.shared.state().questions.remove(&id);
        let Some(((peer, _), answer)) = question else {
            return Ok(()); // already answered, or timed out
        };
        let _ = answer.send(Some(accept));
        self.shared.emit(NodeEvent::OfferClosed { id });
        if accept && always {
            if web::is_browser(&self.shared, &peer) {
                // A browser tab: for as long as it's open.
                web::always_accept(&self.shared, &peer);
            } else {
                self.set_auto_accept(&peer, true)?;
            }
        }
        Ok(())
    }

    /// Send files to a paired, nearby device, one after another, in the
    /// background. Returns the transfer IDs (one per file, in order) that
    /// progress events will use.
    pub fn send_files(&self, peer: Fingerprint, paths: Vec<PathBuf>) -> Vec<u64> {
        self.send(peer, paths.into_iter().map(Outgoing::Path).collect())
    }

    /// Open a connection to `peer` now, so a send that follows soon starts
    /// without waiting for the handshake. Call it when a send is likely
    /// (the share sheet opened, files were dragged in, a device was tapped).
    ///
    /// Does nothing unless the device is paired and nearby, or if a
    /// connection to it is already open or opening. An unused connection
    /// closes after `WARM_LIFETIME`.
    pub fn warm(&self, peer: Fingerprint) {
        self.shared.warm(peer);
    }

    /// Start sharing with browsers, for people without Nudge: returns the
    /// link to show as a QR code. Anyone on the network who opens it appears
    /// as a device (platform "browser") while their tab stays open. Files
    /// sent to them wait for them to accept; files they send are saved
    /// without asking. Stops after ten minutes with nobody connected
    /// (`NodeEvent::WebShareStopped`). Already sharing: the same link.
    pub async fn start_web_share(&self) -> Result<String> {
        web::start(&self.shared).await
    }

    pub fn stop_web_share(&self) {
        web::stop(&self.shared);
    }

    /// The link, while sharing with browsers.
    pub fn web_share_url(&self) -> Option<String> {
        self.shared.state().web.as_ref().map(|w| w.url.clone())
    }

    /// `send_files` for any mix of paths and already-open files.
    pub fn send(&self, peer: Fingerprint, items: Vec<Outgoing>) -> Vec<u64> {
        let jobs: Vec<(u64, Outgoing)> = items
            .into_iter()
            .map(|item| (self.shared.next_id.fetch_add(1, Ordering::Relaxed), item))
            .collect();
        let ids = jobs.iter().map(|(id, _)| *id).collect();
        if web::is_browser(&self.shared, &peer) {
            web::send(self.shared.clone(), peer, jobs);
            return ids;
        }
        let shared = self.shared.clone();
        tokio::spawn(async move {
            let count = jobs.len();
            let batch = Batch {
                id: u64::from_be_bytes(pairing::random_bytes()).max(1),
                count: count as u32,
                size: jobs.iter().map(|(_, item)| item.size()).sum(),
            };
            // Once they decline (or don't answer), the rest isn't offered.
            let mut refused: Option<fn() -> Error> = None;
            for (index, (id, item)) in jobs.into_iter().enumerate() {
                let more = index + 1 < count;
                let name = item.name();
                let size = item.size();
                shared.emit(NodeEvent::TransferStarted {
                    id,
                    peer,
                    direction: Direction::Outgoing,
                    file_name: name.clone(),
                    size,
                });
                let progress = shared.track(id, size);
                let source = match &item {
                    Outgoing::Path(path) => path.clone(),
                    Outgoing::Open { .. } => PathBuf::from(&name),
                };
                if let Some(error) = refused {
                    shared.finish(id, Err(error()));
                    continue;
                }
                let result = shared.send_one(peer, item, batch, &progress, more).await;
                match &result {
                    Err(Error::Declined) => refused = Some(|| Error::Declined),
                    Err(Error::NoAnswer) => refused = Some(|| Error::NoAnswer),
                    _ => {}
                }
                // Whether they asked tells us their setting for this device.
                if progress.accepted() || progress.pending() {
                    shared.record_remote_auto(peer, !progress.pending());
                }
                shared.finish(id, result.map(|stats| (source, stats)));
            }
        });
        ids
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.tasks.iter().for_each(JoinHandle::abort);
        // Its tasks hold the node's state; stopping them lets it go.
        self.shared.state().web = None;
    }
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn auto_accepts(&self, peer: &Fingerprint) -> bool {
        self.auto_accept
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(peer)
    }

    fn set_auto_accept(&self, peer: &Fingerprint, on: bool) -> Result<()> {
        let list = {
            let mut set = self.auto_accept.lock().unwrap_or_else(|e| e.into_inner());
            let changed = if on {
                set.insert(*peer)
            } else {
                set.remove(peer)
            };
            if !changed {
                return Ok(());
            }
            set.iter().map(ToString::to_string).collect::<Vec<_>>()
        };
        let mut text = String::from("# Paired devices whose files are accepted without asking.\n");
        for fp in list {
            text.push_str(&fp);
            text.push('\n');
        }
        std::fs::write(self.data_dir.join(AUTO_ACCEPT_FILE), text)?;
        Ok(())
    }

    /// Start deciding whether to take files: at once for auto-accepted
    /// devices or a batch already answered; otherwise the user is asked
    /// (once per batch) and the answer is waited for with `wait_answer`.
    fn start_asking(
        &self,
        peer: Fingerprint,
        batch: u64,
        first_name: &str,
        count: u32,
        size: u64,
    ) -> Asking {
        if !self.ask || self.auto_accepts(&peer) || web::always_accepts(self, &peer) {
            return Asking::Now(Some(true));
        }
        let key = match batch {
            0 => (peer, u64::from_be_bytes(pairing::random_bytes()).max(1)),
            id => (peer, id),
        };
        let answer = {
            let mut state = self.state();
            state
                .answers
                .retain(|_, (_, at)| at.elapsed() < ANSWER_LIFETIME);
            match state.answers.get(&key) {
                Some((answer, _)) => answer.clone(),
                None => {
                    let (tx, rx) = tokio::sync::watch::channel(None);
                    let id = self.next_id.fetch_add(1, Ordering::Relaxed);
                    state.questions.insert(id, (key, tx));
                    state.answers.insert(key, (rx.clone(), Instant::now()));
                    drop(state);
                    self.emit(NodeEvent::OfferReceived {
                        id,
                        peer,
                        file_name: first_name.to_owned(),
                        count: count.max(1),
                        size,
                    });
                    rx
                }
            }
        };
        let known = *answer.borrow();
        match known {
            Some(value) => Asking::Now(Some(value)),
            None => Asking::Wait(key, answer),
        }
    }

    /// Wait for the user's answer. `None`: nobody answered in time.
    async fn wait_answer(
        &self,
        key: BatchKey,
        mut answer: tokio::sync::watch::Receiver<Option<bool>>,
    ) -> Option<bool> {
        let waited = tokio::time::timeout(ANSWER_TIMEOUT, answer.wait_for(Option::is_some)).await;
        match waited {
            Ok(Ok(value)) => *value,
            _ => {
                // Nobody answered: close the question and forget the batch,
                // so a later send asks again.
                let mut state = self.state();
                state.answers.remove(&key);
                let id = state
                    .questions
                    .iter()
                    .find(|(_, (k, _))| *k == key)
                    .map(|(id, _)| *id);
                if let Some(id) = id {
                    state.questions.remove(&id);
                    drop(state);
                    self.emit(NodeEvent::OfferClosed { id });
                }
                None
            }
        }
    }

    fn download_dir(&self) -> PathBuf {
        self.download_dir
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn emit(&self, event: NodeEvent) {
        let _ = self.events.send(event);
    }

    fn close_pairing(&self) {
        self.emit(NodeEvent::PairingClosed);
    }

    fn trust(&self, fp: Fingerprint, name: &str) -> Result<()> {
        // The same device paired again after a reinstall has a new identity
        // but the same name: drop the old entry rather than keep a stale twin.
        // Only ones not seen right now, so two live devices that share a
        // name both stay.
        let stale: Vec<Fingerprint> = {
            let state = self.state();
            self.trusted
                .list()
                .into_iter()
                .filter(|(old, old_name)| {
                    *old != fp
                        && !name.is_empty()
                        && old_name.eq_ignore_ascii_case(name)
                        && !state.nearby.contains_key(old)
                })
                .map(|(old, _)| old)
                .collect()
        };
        for old in &stale {
            self.trusted.remove(old);
            let _ = self.set_auto_accept(old, false);
        }
        self.trusted.add_named(fp, name);
        self.trusted.save(&self.data_dir)?;
        self.state().remote_auto.remove(&fp);
        self.emit(NodeEvent::Paired {
            peer: self.peer(fp),
        });
        self.emit(NodeEvent::PeersChanged);
        Ok(())
    }

    fn peer(&self, fp: Fingerprint) -> Peer {
        let state = self.state();
        let nearby = state.last_seen.get(&fp);
        Peer {
            fingerprint: fp,
            device_name: self
                .trusted
                .name_of(&fp)
                .filter(|n| !n.is_empty())
                .or_else(|| nearby.map(|n| n.device_name.clone()))
                .unwrap_or_else(|| fp.short()),
            platform: nearby.map(|n| n.platform.clone()).unwrap_or_default(),
            paired: self.trusted.contains(&fp),
            online: state.nearby.contains_key(&fp),
            reachable: nearby.is_some(),
            auto_accept: self.auto_accepts(&fp),
            accepts_from_us: state.remote_auto.get(&fp).copied(),
        }
    }

    /// Where to reach `fp`: where it is now if it's nearby, else where it
    /// was last seen.
    fn addrs_of(&self, fp: &Fingerprint) -> Vec<SocketAddr> {
        let state = self.state();
        state
            .nearby
            .get(fp)
            .or_else(|| state.last_seen.get(fp))
            .map(|p| {
                p.addrs
                    .iter()
                    .map(|ip| SocketAddr::new(*ip, p.port))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn track(&self, id: u64, size: u64) -> Progress {
        let progress = Progress::new();
        self.state().active.insert(
            id,
            Active {
                progress: progress.clone(),
                size,
                last: (0, Instant::now()),
                told_accepted: false,
                told_pending: false,
            },
        );
        progress
    }

    fn finish(&self, id: u64, result: Result<(PathBuf, TransferStats)>) {
        let active = self.state().active.remove(&id);
        // Finished before the progress tick noticed.
        if let Some(a) = active {
            if a.progress.pending() && !a.told_pending {
                self.emit(NodeEvent::TransferPending { id });
            }
            if a.progress.accepted() && !a.told_accepted {
                self.emit(NodeEvent::TransferAccepted { id });
            }
        }
        self.emit(match result {
            Ok((path, stats)) => NodeEvent::TransferDone { id, path, stats },
            Err(e) => NodeEvent::TransferFailed {
                id,
                error: e.to_string(),
            },
        });
    }

    fn warm(self: &Arc<Self>, peer: Fingerprint) {
        if !self.trusted.contains(&peer) {
            return;
        }
        let addrs = self.addrs_of(&peer);
        if addrs.is_empty() {
            return;
        }
        let slot = WarmSlot::default();
        let mut opening = slot
            .clone()
            .try_lock_owned()
            .expect("a new slot is unlocked");
        {
            let mut state = self.state();
            if state.warm.contains_key(&peer) {
                return;
            }
            state.warm.insert(peer, slot.clone());
        }
        let shared = self.clone();
        tokio::spawn(async move {
            match connect_any(&shared.tls, &addrs, peer).await {
                Ok(conn) => *opening = Some((conn, Instant::now())),
                Err(_) => {
                    drop(opening);
                    shared.drop_warm(peer, &slot);
                    return;
                }
            }
            drop(opening);
            tokio::time::sleep(WARM_LIFETIME).await;
            shared.drop_warm(peer, &slot);
        });
    }

    /// Forget `slot` if it's still the one held for `peer`, closing its
    /// connection unless a send already took it.
    fn drop_warm(&self, peer: Fingerprint, slot: &WarmSlot) {
        let mut state = self.state();
        if state.warm.get(&peer).is_some_and(|s| Arc::ptr_eq(s, slot)) {
            state.warm.remove(&peer);
        }
    }

    /// The connection `warm` opened to `peer`, if it's still fresh. Waits
    /// for one that's still opening.
    async fn take_warm(&self, peer: Fingerprint) -> Option<Conn> {
        let slot = self.state().warm.remove(&peer)?;
        let (conn, opened) = slot.lock().await.take()?;
        (opened.elapsed() < WARM_LIFETIME).then_some(conn)
    }

    /// Send one file. `more` says another file to the same device follows,
    /// so its connection can open while this one sends.
    async fn send_one(
        self: &Arc<Self>,
        peer: Fingerprint,
        item: Outgoing,
        batch: Batch,
        progress: &Progress,
        more: bool,
    ) -> Result<TransferStats> {
        if !self.trusted.contains(&peer) {
            return Err(Error::UntrustedPeer(peer));
        }
        let warmed = self.take_warm(peer).await;
        // A connection opened ahead of time may have been closed by the other
        // side while it waited (it restarted, or waited too long). Keep a
        // second handle to the file so the send can go again on a fresh
        // connection; resume makes that safe even partway through.
        let spare = match warmed {
            Some(_) => item.try_clone().ok(),
            None => None,
        };
        let mut stream = match warmed {
            Some(conn) => conn,
            None => connect_any(&self.tls, &self.addrs_of(&peer), peer).await?,
        };
        if more {
            self.warm(peer);
        }
        match (send_over(&mut stream, item, batch, progress).await, spare) {
            (Err(Error::Io(_)), Some(spare)) => {
                let mut stream = connect_any(&self.tls, &self.addrs_of(&peer), peer).await?;
                send_over(&mut stream, spare, batch, progress).await
            }
            (result, _) => result,
        }
    }

    /// Handle one incoming connection: a file from a paired device, or a
    /// pairing request from a device that scanned our code.
    async fn serve<S>(self: &Arc<Self>, mut stream: S, peer: Fingerprint) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let trusted = self.tls.is_trusted(&peer);
        // A paired device may connect before it knows what to send (see
        // `Node::warm`); anyone else must say at once.
        let wait = if trusted {
            self.idle_timeout
        } else {
            REQUEST_TIMEOUT
        };
        let request = tokio::time::timeout(wait, Request::read_from(&mut stream))
            .await
            .map_err(|_| Error::Protocol("no request in time"))??;
        match request {
            Request::File(offer) if trusted => {
                let decision = match self.start_asking(
                    peer,
                    offer.batch.id,
                    &offer.name,
                    offer.batch.count,
                    offer.batch.size.max(offer.size),
                ) {
                    Asking::Now(answer) => answer,
                    Asking::Wait(key, answer) => {
                        // Let the sender know it's delivered and waiting.
                        let _ = Response::Asking.write_to(&mut stream).await;
                        self.wait_answer(key, answer).await
                    }
                };
                match decision {
                    Some(true) => {}
                    Some(false) => {
                        let _ = Response::Declined.write_to(&mut stream).await;
                        return Ok(());
                    }
                    None => {
                        let _ = Response::NoAnswer.write_to(&mut stream).await;
                        return Ok(());
                    }
                }
                let id = self.next_id.fetch_add(1, Ordering::Relaxed);
                self.emit(NodeEvent::TransferStarted {
                    id,
                    peer,
                    direction: Direction::Incoming,
                    file_name: offer.name.clone(),
                    size: offer.size,
                });
                let progress = self.track(id, offer.size);
                let dir = self
                    .download_dir
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                let result = match std::fs::create_dir_all(&dir) {
                    Ok(()) => receive_offered(&mut stream, offer, &dir, Some(peer), &progress)
                        .await
                        .map(|f| (f.path, f.stats)),
                    Err(e) => {
                        let _ = Response::Reject.write_to(&mut stream).await;
                        Err(e.into())
                    }
                };
                let failed = result.is_err();
                self.finish(id, result);
                if !failed {
                    let _ = stream.shutdown().await;
                }
                Ok(())
            }
            Request::File(_) => {
                let _ = Response::NotPaired.write_to(&mut stream).await;
                Err(Error::UntrustedPeer(peer))
            }
            Request::Pair(request) => self.answer_pairing(stream, peer, request).await,
            Request::PairAsk(ask) => self.answer_pair_ask(stream, peer, ask).await,
            Request::Status(theirs) if trusted => {
                self.record_remote_auto(peer, theirs.auto_accepts_you);
                Status {
                    auto_accepts_you: self.auto_accepts(&peer),
                }
                .write_reply(&mut stream)
                .await?;
                let _ = stream.shutdown().await;
                Ok(())
            }
            Request::Status(_) => Err(Error::UntrustedPeer(peer)),
        }
    }

    /// "Can we pair?" from a nearby device: ask the user, who compares the
    /// codes on both screens.
    async fn answer_pair_ask<S>(&self, mut stream: S, peer: Fingerprint, ask: PairAsk) -> Result<()>
    where
        S: AsyncWrite + Unpin,
    {
        let reject = |mut s: S| async move {
            let _ = PairReply::Reject.write_to(&mut s).await;
            Err(Error::PairingRejected)
        };
        if peer == self.fingerprint {
            return reject(stream).await;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        // One question per device at a time.
        let id = {
            let mut state = self.state();
            if state.pair_asks.values().any(|(p, _)| *p == peer) {
                None
            } else {
                let id = self.next_id.fetch_add(1, Ordering::Relaxed);
                state.pair_asks.insert(id, (peer, tx));
                Some(id)
            }
        };
        let Some(id) = id else {
            return reject(stream).await;
        };
        let name = clean_name(&ask.device_name);
        self.emit(NodeEvent::PairAsked {
            id,
            peer,
            device_name: name.clone(),
            code: pairing::compare_code(&self.fingerprint, &peer),
        });
        let yes = matches!(
            tokio::time::timeout(PAIR_ASK_TIMEOUT, rx).await,
            Ok(Ok(true))
        );
        if self.state().pair_asks.remove(&id).is_some() {
            self.emit(NodeEvent::OfferClosed { id });
        }
        if !yes {
            return reject(stream).await;
        }
        PairReply::Accept {
            device_name: self.device_name.clone(),
        }
        .write_to(&mut stream)
        .await?;
        let _ = stream.shutdown().await;
        self.trust(peer, &name)
    }

    /// What a paired device said about accepting our files.
    fn record_remote_auto(&self, peer: Fingerprint, on: bool) {
        let changed = self.state().remote_auto.insert(peer, on) != Some(on);
        if changed {
            self.emit(NodeEvent::PeersChanged);
        }
    }

    /// Swap settings with a paired device in the background: tells it
    /// whether we auto-accept it, and learns the same about it.
    fn exchange_status(self: &Arc<Self>, peer: Fingerprint) {
        // Called from outside the runtime, it can't run; the next time the
        // device shows up, the settings are swapped anyway.
        if !self.trusted.contains(&peer) || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let shared = self.clone();
        tokio::spawn(async move {
            let addrs = shared.addrs_of(&peer);
            let Ok(mut stream) = connect_any(&shared.tls, &addrs, peer).await else {
                return;
            };
            let mine = Status {
                auto_accepts_you: shared.auto_accepts(&peer),
            };
            if mine.write_to(&mut stream).await.is_err() {
                return;
            }
            if let Ok(Ok(theirs)) =
                tokio::time::timeout(REQUEST_TIMEOUT, Status::read_reply(&mut stream)).await
            {
                shared.record_remote_auto(peer, theirs.auto_accepts_you);
            }
            let _ = stream.shutdown().await;
        });
    }

    async fn answer_pairing<S>(
        &self,
        mut stream: S,
        peer: Fingerprint,
        request: PairRequest,
    ) -> Result<()>
    where
        S: AsyncWrite + Unpin,
    {
        // Check and use up the code in one step, so it works exactly once.
        let accepted = {
            let mut state = self.state();
            let valid = state.pairing.as_ref().is_some_and(|(code, deadline)| {
                Instant::now() < *deadline
                    && pairing::verify(&code.secret, &peer, &self.fingerprint, &request.proof)
            });
            if valid {
                state.pairing = None;
            }
            valid
        };
        if !accepted {
            PairReply::Reject.write_to(&mut stream).await?;
            return Err(Error::PairingRejected);
        }
        self.close_pairing();
        PairReply::Accept {
            device_name: self.device_name.clone(),
        }
        .write_to(&mut stream)
        .await?;
        let _ = stream.shutdown().await;
        self.trust(peer, &request.device_name)
    }
}

async fn accept_loop(shared: Arc<Shared>, listener: TcpListener) {
    loop {
        let Ok((tcp, _addr)) = listener.accept().await else {
            // Out of file handles or similar: back off instead of spinning.
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let _ = tcp.set_nodelay(true);
        let shared = shared.clone();
        tokio::spawn(async move {
            let Ok(Ok((stream, peer))) =
                tokio::time::timeout(REQUEST_TIMEOUT, shared.tls.accept(tcp)).await
            else {
                return; // untrusted device outside a pairing window, or junk
            };
            let _ = shared.serve(stream, peer).await;
        });
    }
}

async fn discovery_loop(shared: Arc<Shared>, mut rx: mpsc::UnboundedReceiver<DiscoveryEvent>) {
    while let Some(event) = rx.recv().await {
        {
            let mut state = shared.state();
            match event {
                DiscoveryEvent::Found(peer) => {
                    // A paired device's name is the one it gave when pairing,
                    // unless we never learned one.
                    if shared
                        .trusted
                        .name_of(&peer.fingerprint)
                        .is_some_and(|n| n.is_empty())
                        && !peer.device_name.is_empty()
                    {
                        shared
                            .trusted
                            .add_named(peer.fingerprint, &peer.device_name);
                        let _ = shared.trusted.save(&shared.data_dir);
                    }
                    if shared.trusted.contains(&peer.fingerprint)
                        && state.status_asked.insert(peer.fingerprint)
                    {
                        shared.exchange_status(peer.fingerprint);
                    }
                    state.last_seen.insert(peer.fingerprint, peer.clone());
                    state.nearby.insert(peer.fingerprint, peer);
                }
                DiscoveryEvent::Lost(fp) => {
                    state.nearby.remove(&fp);
                }
            }
        }
        shared.emit(NodeEvent::PeersChanged);
    }
}

async fn progress_loop(shared: Arc<Shared>) {
    let mut tick = tokio::time::interval(PROGRESS_INTERVAL);
    loop {
        tick.tick().await;
        let now = Instant::now();
        let updates: Vec<NodeEvent> = shared
            .state()
            .active
            .iter_mut()
            .flat_map(|(id, active)| {
                let pending = (!active.told_pending && active.progress.pending()).then(|| {
                    active.told_pending = true;
                    NodeEvent::TransferPending { id: *id }
                });
                let accepted = (!active.told_accepted && active.progress.accepted()).then(|| {
                    active.told_accepted = true;
                    NodeEvent::TransferAccepted { id: *id }
                });
                pending
                    .into_iter()
                    .chain(accepted)
                    .chain(progress_update(*id, active, now))
            })
            .collect();
        updates.into_iter().for_each(|e| shared.emit(e));
    }
}

/// A progress event for `active`, if it moved since the last one.
fn progress_update(id: u64, active: &mut Active, now: Instant) -> Option<NodeEvent> {
    let bytes = active.progress.position();
    let (last_bytes, last_time) = active.last;
    if bytes == last_bytes {
        return None;
    }
    let secs = now.duration_since(last_time).as_secs_f64().max(1e-3);
    active.last = (bytes, now);
    Some(NodeEvent::TransferProgress {
        id,
        bytes,
        size: active.size,
        bytes_per_sec: bytes.saturating_sub(last_bytes) as f64 / secs,
    })
}

const AUTO_ACCEPT_FILE: &str = "auto_accept.txt";

fn load_auto_accept(dir: &std::path::Path) -> HashSet<Fingerprint> {
    std::fs::read_to_string(dir.join(AUTO_ACCEPT_FILE))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

/// Send one file over an open connection, then close it.
async fn send_over(
    stream: &mut Conn,
    item: Outgoing,
    batch: Batch,
    progress: &Progress,
) -> Result<TransferStats> {
    let stats = match item {
        Outgoing::Path(path) => send_file_tracked(stream, &path, Some(batch), progress).await?,
        Outgoing::Open { file, name } => {
            send_open_file_tracked(stream, file, name, Some(batch), progress).await?
        }
    };
    let _ = stream.shutdown().await;
    Ok(stats)
}

/// Connect over TLS to `expected` at the first of `addrs` where it answers.
/// Addresses held by some other device (a stale discovery entry, a reused
/// IP) are skipped.
async fn connect_any(tls: &Tls, addrs: &[SocketAddr], expected: Fingerprint) -> Result<Conn> {
    let mut last_error = Error::Unreachable;
    for addr in addrs {
        let tcp = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
            Ok(Ok(tcp)) => tcp,
            _ => continue,
        };
        let _ = tcp.set_nodelay(true);
        match tokio::time::timeout(CONNECT_TIMEOUT, tls.connect(tcp)).await {
            Ok(Ok((stream, fp))) if fp == expected => return Ok(stream),
            Ok(Ok(_)) | Ok(Err(Error::UntrustedPeer(_))) => {}
            // The right device, but it doesn't trust us: that's the answer.
            Ok(Err(Error::NotTrustedByPeer)) => return Err(Error::NotTrustedByPeer),
            Ok(Err(e)) => last_error = e,
            Err(_) => {}
        }
    }
    Err(last_error)
}

/// This device's addresses worth putting in a pairing code: up, not
/// loopback, IPv4 only (short codes; phones and PCs on the same Wi-Fi
/// always share IPv4), private ranges first.
fn local_addrs() -> Vec<IpAddr> {
    let mut addrs: Vec<IpAddr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback() && i.is_oper_up() && !i.is_link_local())
        .map(|i| i.ip())
        .filter(IpAddr::is_ipv4)
        .collect();
    addrs.sort_by_key(|a| !matches!(a, IpAddr::V4(v4) if v4.is_private()));
    addrs.dedup();
    addrs.truncate(6);
    if addrs.is_empty() {
        addrs.push(IpAddr::V4(Ipv4Addr::LOCALHOST));
    }
    addrs
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn start(
        name: &str,
        udp: u16,
        other_udp: u16,
    ) -> (Node, mpsc::UnboundedReceiver<NodeEvent>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = NodeConfig::new(
            dir.path().join("id"),
            dir.path().join("downloads"),
            name,
            "android",
        );
        config.port = Some(0);
        config.ask_before_receiving = false;
        config.bind = Ipv4Addr::LOCALHOST.into();
        config.discovery = DiscoveryConfig {
            mdns: false,
            broadcast: true,
            udp_port: udp,
            extra_targets: vec![SocketAddr::from((Ipv4Addr::LOCALHOST, other_udp))],
        };
        let (node, events) = Node::start(config).await.unwrap();
        assert!(node.discovery_running(), "couldn't bind UDP port {udp}");
        (node, events, dir)
    }

    /// A free UDP port below the OS's random-port range. Discovery falls back
    /// to a random port when its own is taken, and that must never land on a
    /// port another test is using, or the tests hear each other.
    fn free_udp_port() -> u16 {
        use std::sync::atomic::{AtomicU16, Ordering};
        static NEXT: AtomicU16 = AtomicU16::new(0);
        let _ = NEXT.compare_exchange(
            0,
            20_000 + (std::process::id() % 4_000) as u16 * 2,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        loop {
            let port = NEXT.fetch_add(1, Ordering::SeqCst);
            assert!(port < 30_000, "no free test ports");
            if std::net::UdpSocket::bind(("0.0.0.0", port)).is_ok() {
                return port;
            }
        }
    }

    async fn until(mut check: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(15), async {
            while !check() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("timed out");
    }

    /// A phone with its screen off stops announcing itself, and drops out of
    /// the nearby list, but can still be sent to where it was last seen.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn sends_to_a_device_that_went_quiet() {
        let (udp_a, udp_b) = (free_udp_port(), free_udp_port());
        let (laptop, mut laptop_events, _l) = start("Laptop", udp_a, udp_b).await;
        let (phone, _phone_events, phone_dir) = start("Phone", udp_b, udp_a).await;
        let phone_fp = phone.fingerprint();
        until(|| {
            phone
                .peers()
                .iter()
                .any(|p| p.fingerprint == laptop.fingerprint())
        })
        .await;
        until(|| laptop.peers().iter().any(|p| p.fingerprint == phone_fp)).await;
        phone.pair(&laptop.start_pairing()).await.unwrap();
        until(|| laptop.peers().iter().any(|p| p.paired)).await;

        // The phone goes quiet: gone from nearby, still remembered.
        laptop.shared.state().nearby.remove(&phone_fp);
        let seen = laptop
            .peers()
            .into_iter()
            .find(|p| p.fingerprint == phone_fp)
            .unwrap();
        assert!(seen.paired && !seen.online && seen.reachable);
        assert_eq!(seen.platform, "android");
        assert!(!laptop.shared.addrs_of(&phone_fp).is_empty());

        let src = phone_dir.path().join("note.txt");
        std::fs::write(&src, b"hello from the laptop").unwrap();
        let ids = laptop.send_files(phone_fp, vec![src]);
        loop {
            match laptop_events.recv().await.unwrap() {
                NodeEvent::TransferDone { id, .. } if id == ids[0] => break,
                NodeEvent::TransferFailed { error, .. } => panic!("send failed: {error}"),
                _ => {}
            }
        }
        let got = std::fs::read(phone_dir.path().join("downloads").join("note.txt")).unwrap();
        assert_eq!(got, b"hello from the laptop");
    }
}
