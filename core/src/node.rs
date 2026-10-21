//! `Node`: everything a Nudge app needs, behind one object.
//!
//! A node listens for connections, finds nearby devices, pairs by QR code,
//! sends files and accepts files from paired devices without asking. Apps
//! call its methods and react to the `NodeEvent`s it emits; they never touch
//! sockets or TLS themselves.
//!
//! Must be started inside a tokio runtime.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
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
use crate::protocol::{PairReply, PairRequest, Request, Response};
use crate::transfer::{Progress, TransferStats, receive_offered, send_file_tracked};
use crate::{DEFAULT_PORT, Error, Result, Tls};

/// How long to wait for each address when connecting.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// How long a new connection may take to say what it wants.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

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
}

/// A running Nudge device. Dropping it stops everything.
pub struct Node {
    shared: Arc<Shared>,
    discovery: Option<Discovery>,
    tasks: Vec<JoinHandle<()>>,
}

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
}

#[derive(Default)]
struct State {
    nearby: HashMap<Fingerprint, DiscoveredPeer>,
    /// The code this device is showing, and when it stops working.
    pairing: Option<(PairingCode, Instant)>,
    active: HashMap<u64, Active>,
}

/// A running transfer, for progress reporting.
struct Active {
    progress: Progress,
    size: u64,
    last: (u64, Instant),
}

impl Node {
    /// Load (or create) this device's identity, start listening and start
    /// discovery. Events arrive on the returned channel.
    pub async fn start(config: NodeConfig) -> Result<(Self, mpsc::UnboundedReceiver<NodeEvent>)> {
        let identity = Identity::load_or_create(&config.data_dir)?;
        let trusted = TrustedPeers::load(&config.data_dir)?;
        let tls = Tls::new(&identity, trusted.clone())?;
        let fingerprint = identity.fingerprint();

        let listener = match config.port {
            Some(port) => TcpListener::bind((config.bind, port)).await?,
            None => match TcpListener::bind((config.bind, DEFAULT_PORT)).await {
                Ok(l) => l,
                Err(_) => TcpListener::bind((config.bind, 0)).await?,
            },
        };
        let port = listener.local_addr()?.port();

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
                let nearby = state.nearby.get(&fp);
                Peer {
                    fingerprint: fp,
                    device_name: if name.is_empty() {
                        nearby.map_or_else(|| fp.short(), |n| n.device_name.clone())
                    } else {
                        name
                    },
                    platform: nearby.map(|n| n.platform.clone()).unwrap_or_default(),
                    paired: true,
                    online: nearby.is_some(),
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
                }),
        );
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
        addrs.extend(self.shared.nearby_addrs(&code.fingerprint));
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
            self.shared.emit(NodeEvent::PeersChanged);
        }
        Ok(())
    }

    /// Send files to a paired, nearby device, one after another, in the
    /// background. Returns the transfer IDs (one per file, in order) that
    /// progress events will use.
    pub fn send_files(&self, peer: Fingerprint, paths: Vec<PathBuf>) -> Vec<u64> {
        let jobs: Vec<(u64, PathBuf)> = paths
            .into_iter()
            .map(|p| (self.shared.next_id.fetch_add(1, Ordering::Relaxed), p))
            .collect();
        let ids = jobs.iter().map(|(id, _)| *id).collect();
        let shared = self.shared.clone();
        tokio::spawn(async move {
            for (id, path) in jobs {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                shared.emit(NodeEvent::TransferStarted {
                    id,
                    peer,
                    direction: Direction::Outgoing,
                    file_name: name,
                    size,
                });
                let progress = shared.track(id, size);
                let result = shared.send_one(peer, &path, &progress).await;
                shared.finish(id, result.map(|stats| (path, stats)));
            }
        });
        ids
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.tasks.iter().for_each(JoinHandle::abort);
    }
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn emit(&self, event: NodeEvent) {
        let _ = self.events.send(event);
    }

    fn close_pairing(&self) {
        self.tls.set_pairing_open(false);
        self.emit(NodeEvent::PairingClosed);
    }

    fn trust(&self, fp: Fingerprint, name: &str) -> Result<()> {
        self.trusted.add_named(fp, name);
        self.trusted.save(&self.data_dir)?;
        self.emit(NodeEvent::Paired {
            peer: self.peer(fp),
        });
        self.emit(NodeEvent::PeersChanged);
        Ok(())
    }

    fn peer(&self, fp: Fingerprint) -> Peer {
        let state = self.state();
        let nearby = state.nearby.get(&fp);
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
            online: nearby.is_some(),
        }
    }

    fn nearby_addrs(&self, fp: &Fingerprint) -> Vec<SocketAddr> {
        self.state()
            .nearby
            .get(fp)
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
            },
        );
        progress
    }

    fn finish(&self, id: u64, result: Result<(PathBuf, TransferStats)>) {
        self.state().active.remove(&id);
        self.emit(match result {
            Ok((path, stats)) => NodeEvent::TransferDone { id, path, stats },
            Err(e) => NodeEvent::TransferFailed {
                id,
                error: e.to_string(),
            },
        });
    }

    async fn send_one(
        &self,
        peer: Fingerprint,
        path: &Path,
        progress: &Progress,
    ) -> Result<TransferStats> {
        if !self.trusted.contains(&peer) {
            return Err(Error::UntrustedPeer(peer));
        }
        let addrs = self.nearby_addrs(&peer);
        let mut stream = connect_any(&self.tls, &addrs, peer).await?;
        let stats = send_file_tracked(&mut stream, path, progress).await?;
        let _ = stream.shutdown().await;
        Ok(stats)
    }

    /// Handle one incoming connection: a file from a paired device, or a
    /// pairing request from a device that scanned our code.
    async fn serve<S>(self: &Arc<Self>, mut stream: S, peer: Fingerprint) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let request = tokio::time::timeout(REQUEST_TIMEOUT, Request::read_from(&mut stream))
            .await
            .map_err(|_| Error::Protocol("no request in time"))??;
        let trusted = self.tls.is_trusted(&peer);
        match request {
            Request::File(offer) if trusted => {
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
                let _ = Response::Reject.write_to(&mut stream).await;
                Err(Error::UntrustedPeer(peer))
            }
            Request::Pair(request) => self.answer_pairing(stream, peer, request).await,
        }
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
            .filter_map(|(id, active)| {
                let bytes = active.progress.position();
                let (last_bytes, last_time) = active.last;
                if bytes == last_bytes {
                    return None;
                }
                let secs = now.duration_since(last_time).as_secs_f64().max(1e-3);
                active.last = (bytes, now);
                Some(NodeEvent::TransferProgress {
                    id: *id,
                    bytes,
                    size: active.size,
                    bytes_per_sec: bytes.saturating_sub(last_bytes) as f64 / secs,
                })
            })
            .collect();
        updates.into_iter().for_each(|e| shared.emit(e));
    }
}

/// Connect over TLS to `expected` at the first of `addrs` where it answers.
/// Addresses held by some other device (a stale discovery entry, a reused
/// IP) are skipped.
async fn connect_any(
    tls: &Tls,
    addrs: &[SocketAddr],
    expected: Fingerprint,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
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
