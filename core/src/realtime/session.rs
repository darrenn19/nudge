//! A real-time session: the handshake over an existing TLS connection, then
//! a background task that runs the UDP side.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::crypto::{Opener, Sealer};
use super::frames::{Frame, FrameReceiver, FrameSender, OutFrame, ReceiveStats, SendStats};
use super::paths::{PathInfo, Paths};
use super::wire::{self, Message};
use crate::link::{self, LinkKind};
use crate::{Error, Result};

const HELLO_MAGIC: [u8; 4] = *b"NURT";
const HELLO_VERSION: u8 = 1;
const EXPORTER_LABEL: &[u8] = b"EXPORTER-nudge-realtime-1";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CANDIDATES: usize = 16;
/// How far the pacer may run ahead of the capped rate.
const PACE_BURST_US: u64 = 1_000;
/// Most the pacer catches up after waking late. Timers are coarse on some
/// systems (Windows: ~16 ms unless raised); without catching up, sending
/// would fall below the capped rate there.
const MAX_CATCH_UP_US: u64 = 20_000;
/// How often to check on frames in flight.
const POLL_US: u64 = 2_000;
/// Clock samples older than this are forgotten.
const CLOCK_WINDOW_US: u64 = 10_000_000;
/// How often each side tells the other its addresses, and looks at its own
/// networks.
const ADDRS_EVERY_US: u64 = 1_000_000;
/// With no path working for this long, the session ends.
const NO_PATH_US: u64 = 10_000_000;

/// The TLS connection a session is set up over. It stays open for the
/// session: the other side closing it ends the session. If it breaks
/// instead (its network went away: a cable pulled out, the PC moved to the
/// phone's hotspot), the session carries on over whatever paths still work.
pub trait Control: AsyncRead + AsyncWrite + Unpin + Send + 'static {
    /// TLS key export (RFC 8446 §7.5).
    fn export_keys(&self, out: &mut [u8], label: &[u8], context: &[u8]) -> Result<()>;
    /// The other device's address on this connection.
    fn peer_ip(&self) -> Option<IpAddr>;
}

impl Control for tokio_rustls::client::TlsStream<TcpStream> {
    fn export_keys(&self, out: &mut [u8], label: &[u8], context: &[u8]) -> Result<()> {
        self.get_ref().1.export_keying_material(out, label, Some(context))?;
        Ok(())
    }
    fn peer_ip(&self) -> Option<IpAddr> {
        self.get_ref().0.peer_addr().ok().map(|a| a.ip())
    }
}

impl Control for tokio_rustls::server::TlsStream<TcpStream> {
    fn export_keys(&self, out: &mut [u8], label: &[u8], context: &[u8]) -> Result<()> {
        self.get_ref().1.export_keying_material(out, label, Some(context))?;
        Ok(())
    }
    fn peer_ip(&self) -> Option<IpAddr> {
        self.get_ref().0.peer_addr().ok().map(|a| a.ip())
    }
}

/// Session settings.
#[derive(Debug, Clone)]
pub struct Config {
    /// Cap on the sending rate, Mbit/s: frames are spread out instead of
    /// going as one burst. 0 = no cap.
    pub pace_mbps: u32,
    /// Throw away this share (%) of incoming fragments. For tests.
    #[doc(hidden)]
    pub drop_percent: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            pace_mbps: 200,
            drop_percent: 0.0,
        }
    }
}

/// Something that happened in a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A complete frame, in order.
    Frame(Frame),
    /// The encoder for this stream should make a keyframe: the other side
    /// lost frames it couldn't recover, or frames were dropped here.
    KeyframeRequested { stream: u8 },
    /// Now using this path (`None`: no path works at the moment).
    PathChanged(Option<PathInfo>),
    /// The other side ended the session, or the connection broke.
    Closed,
}

/// Counters and state, for display and benchmarks.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub path: Option<PathInfo>,
    /// Every path tried, and whether it's alive.
    pub paths: Vec<(PathInfo, bool)>,
    pub sent: SendStats,
    pub received: ReceiveStats,
    /// Add to the other side's clock to get this side's (µs), from the
    /// fastest ping of the last 10 s. Accurate to half that ping's round trip.
    pub clock_offset_us: Option<i64>,
    /// Round trip of that ping: the best of the last 10 s.
    pub clock_rtt_us: Option<u64>,
    /// The latest round trip on the path in use (not smoothed): with
    /// `clock_rtt_us`, how much is queuing in the network now.
    pub rtt_latest_us: Option<u64>,
    /// Packets that failed decryption or came from another session.
    pub rejected_packets: u64,
    /// Fragments thrown away on purpose (`Config::drop_percent`).
    pub dropped_on_purpose: u64,
}

enum Command {
    /// A frame, with extra fragment flags.
    Send(OutFrame, u8),
    /// Pause (`Some`: ending the session if still paused after that long)
    /// or resume (`None`). See `Handle::pause`.
    Pause(Option<Duration>),
    Subscribe(u8, mpsc::UnboundedSender<Frame>),
    NeedKeyframe(u8),
    Close,
}

/// One end of a real-time session. Dropping it ends the session.
pub struct Session {
    commands: mpsc::UnboundedSender<Command>,
    events: mpsc::UnboundedReceiver<Event>,
    stats: Arc<Mutex<Stats>>,
    epoch: Instant,
}

impl Session {
    /// Starts a session as the side that asked for it.
    pub async fn open<C: Control>(control: C, config: Config) -> Result<Self> {
        Self::start(control, config, true).await
    }

    /// Starts a session as the side that was asked.
    pub async fn accept<C: Control>(control: C, config: Config) -> Result<Self> {
        Self::start(control, config, false).await
    }

    async fn start<C: Control>(mut control: C, config: Config, initiator: bool) -> Result<Self> {
        let socket = bind_udp()?;
        let port = socket.local_addr()?.port();
        let nets = link::local_nets();
        let candidates: Vec<Ipv4Addr> = nets.iter().map(|n| n.ip).take(MAX_CANDIDATES).collect();

        let (nonce, their_port, theirs) = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            if initiator {
                let mut nonce = [0u8; 16];
                aws_lc_rs::rand::fill(&mut nonce).map_err(|_| Error::Protocol("no randomness"))?;
                control.write_all(&hello(&nonce, port, &candidates)).await?;
                control.flush().await?;
                let (echo, their_port, theirs) = read_hello(&mut control).await?;
                if echo != nonce {
                    return Err(Error::Protocol("real-time handshake mismatch"));
                }
                Ok((nonce, their_port, theirs))
            } else {
                let (nonce, their_port, theirs) = read_hello(&mut control).await?;
                control.write_all(&hello(&nonce, port, &candidates)).await?;
                control.flush().await?;
                Ok((nonce, their_port, theirs))
            }
        })
        .await
        .map_err(|_| Error::Protocol("real-time handshake timed out"))??;

        // Keys for each direction plus a tag naming this session, all bound
        // to the TLS session (so to both devices' certificates).
        let mut keys = [0u8; 68];
        control.export_keys(&mut keys, EXPORTER_LABEL, &nonce)?;
        let (a, b) = (keys[..32].try_into().unwrap(), keys[32..64].try_into().unwrap());
        let tag = u32::from_be_bytes(keys[64..].try_into().unwrap());
        let (send_key, recv_key) = if initiator { (a, b) } else { (b, a) };

        let mut paths = Paths::default();
        // The address the TLS connection uses is known to work.
        if let Some(ip) = control.peer_ip() {
            let kind = link::kind_toward(&nets, ip).unwrap_or(LinkKind::Other);
            paths.add(SocketAddr::new(ip, their_port), kind);
        }
        // Their other networks, where one of ours reaches it directly (the
        // rest are kept in case one of ours comes to: see `on_nets`).
        for &ip in &theirs {
            if let Some(kind) = link::kind_toward(&nets, IpAddr::V4(ip)) {
                paths.add(SocketAddr::new(IpAddr::V4(ip), their_port), kind);
            }
        }

        let epoch = Instant::now();
        let stats = Arc::new(Mutex::new(Stats::default()));
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (event_tx, events) = mpsc::unbounded_channel();
        let (nets_tx, nets_rx) = mpsc::unbounded_channel();
        tokio::spawn(watch_nets(nets.clone(), nets_tx));
        let driver = Driver {
            socket,
            control,
            sealer: Sealer::new(&send_key, tag),
            opener: Opener::new(&recv_key, tag),
            paths,
            nets,
            nets_rx,
            their_port,
            theirs,
            control_open: true,
            last_path_us: 0,
            last_addrs_us: 0,
            ending: false,
            senders: HashMap::new(),
            receivers: HashMap::new(),
            ns_per_byte: if config.pace_mbps > 0 { 8_000.0 / config.pace_mbps as f64 } else { 0.0 },
            next_send_us: 0,
            clock: Vec::new(),
            events: event_tx,
            routes: HashMap::new(),
            stats: stats.clone(),
            epoch,
            drop_percent: config.drop_percent,
            rng: 0x9E37_79B9_7F4A_7C15,
            local: Stats::default(),
            timer_late_us: 0,
            paused_until: None,
        };
        tokio::spawn(driver.run(command_rx));
        Ok(Self {
            commands,
            events,
            stats,
            epoch,
        })
    }

    /// This session's clock, µs. Use it for `OutFrame::capture_us`.
    pub fn now_us(&self) -> u64 {
        self.epoch.elapsed().as_micros() as u64
    }

    /// Converts a time in this session's clock to an `Instant`.
    pub fn instant_of(&self, us: u64) -> std::time::Instant {
        self.epoch.into_std() + Duration::from_micros(us)
    }

    /// Queues a frame to send. Fails once the session has ended.
    pub fn send(&self, frame: OutFrame) -> Result<()> {
        self.commands
            .send(Command::Send(frame, 0))
            .map_err(|_| Error::Protocol("real-time session closed"))
    }

    /// Queues a message on `stream` (input events, status): delivered in
    /// order and recovered like frames, but each stands alone, so a loss
    /// never waits for a keyframe and nothing is skipped while it can still
    /// be recovered (about 300 ms). Messages up to ~1 KB fit one packet.
    pub fn send_message(&self, stream: u8, data: Vec<u8>) -> Result<()> {
        let frame = OutFrame {
            stream,
            data,
            key: false,
            intra_refresh: true,
            capture_us: self.now_us(),
        };
        self.commands
            .send(Command::Send(frame, wire::F_STANDALONE))
            .map_err(|_| Error::Protocol("real-time session closed"))
    }

    /// From now on, frames (and messages) of `stream` go to the returned
    /// receiver instead of `next_event`. It ends with the session.
    pub fn subscribe(&self, stream: u8) -> mpsc::UnboundedReceiver<Frame> {
        let (tx, rx) = mpsc::unbounded_channel();
        let _ = self.commands.send(Command::Subscribe(stream, tx));
        rx
    }

    /// The decoder for this stream failed: skip frames until a keyframe and
    /// ask the other side for one.
    pub fn need_keyframe(&self, stream: u8) {
        let _ = self.commands.send(Command::NeedKeyframe(stream));
    }

    /// The next event; `None` after `Closed`.
    pub async fn next_event(&mut self) -> Option<Event> {
        self.events.recv().await
    }

    /// Like `next_event`, without waiting.
    pub fn try_event(&mut self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    pub fn stats(&self) -> Stats {
        self.stats.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// A time in the other side's clock (such as `Frame::capture_us`), in
    /// this session's clock. `None` until the clocks have been compared.
    pub fn to_local_us(&self, theirs: u64) -> Option<u64> {
        let offset = self.stats().clock_offset_us?;
        u64::try_from(theirs as i64 + offset).ok()
    }

    /// Ends the session (the other side gets `Closed`).
    pub fn close(&self) {
        let _ = self.commands.send(Command::Close);
    }

    /// A handle that can end this session from elsewhere, and tell when it
    /// has ended.
    pub fn handle(&self) -> Handle {
        Handle {
            commands: self.commands.clone(),
            epoch: self.epoch,
        }
    }
}

/// Ends, pauses or waits for a session, without owning it.
#[derive(Clone)]
pub struct Handle {
    commands: mpsc::UnboundedSender<Command>,
    epoch: Instant,
}

impl Handle {
    /// Like `Session::send_message`.
    pub fn send_message(&self, stream: u8, data: Vec<u8>) -> Result<()> {
        let frame = OutFrame {
            stream,
            data,
            key: false,
            intra_refresh: true,
            capture_us: self.epoch.elapsed().as_micros() as u64,
        };
        self.commands
            .send(Command::Send(frame, wire::F_STANDALONE))
            .map_err(|_| Error::Protocol("real-time session closed"))
    }

    /// Pauses the session (`Some(limit)`) or resumes it (`None`). While
    /// paused, frames given to it are dropped instead of sent (messages
    /// still go), and nothing that arrives is handed on: the session stays
    /// up, but nothing passes. Still paused after `limit`, it ends. Resuming
    /// asks this side for a keyframe on stream 0 (`Event::KeyframeRequested`).
    pub fn pause(&self, limit: Option<Duration>) {
        let _ = self.commands.send(Command::Pause(limit));
    }

    pub fn close(&self) {
        let _ = self.commands.send(Command::Close);
    }

    /// Returns once the session has ended, for whatever reason.
    pub async fn closed(&self) {
        self.commands.closed().await;
    }

    pub fn is_closed(&self) -> bool {
        self.commands.is_closed()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

/// Looks at this device's networks every `ADDRS_EVERY_US` (off the driver:
/// on Windows it can take milliseconds) and passes them on when they
/// change. Ends with the session.
async fn watch_nets(mut last: Vec<link::LocalNet>, tx: mpsc::UnboundedSender<Vec<link::LocalNet>>) {
    let key = |nets: &[link::LocalNet]| -> Vec<(Ipv4Addr, Ipv4Addr, LinkKind)> {
        nets.iter().map(|n| (n.ip, n.netmask, n.kind)).collect()
    };
    loop {
        tokio::time::sleep(Duration::from_micros(ADDRS_EVERY_US)).await;
        if tx.is_closed() {
            return;
        }
        let Ok(nets) = tokio::task::spawn_blocking(link::local_nets).await else { return };
        if key(&nets) != key(&last) {
            last = nets.clone();
            if tx.send(nets).is_err() {
                return;
            }
        }
    }
}

fn bind_udp() -> Result<UdpSocket> {
    let s = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    // Keyframes arrive as bursts of 100+ packets.
    let _ = s.set_recv_buffer_size(4 << 20);
    let _ = s.set_send_buffer_size(4 << 20);
    s.bind(&SocketAddr::from(([0, 0, 0, 0], 0)).into())?;
    s.set_nonblocking(true)?;
    Ok(UdpSocket::from_std(s.into())?)
}

/// magic, version u8, nonce [16], UDP port u16, n u8, n × IPv4.
fn hello(nonce: &[u8; 16], port: u16, candidates: &[Ipv4Addr]) -> Vec<u8> {
    let mut m = Vec::new();
    m.extend_from_slice(&HELLO_MAGIC);
    m.push(HELLO_VERSION);
    m.extend_from_slice(nonce);
    m.extend_from_slice(&port.to_be_bytes());
    m.push(candidates.len() as u8);
    for ip in candidates {
        m.extend_from_slice(&ip.octets());
    }
    m
}

async fn read_hello<R: AsyncRead + Unpin>(r: &mut R) -> Result<([u8; 16], u16, Vec<Ipv4Addr>)> {
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic).await?;
    if magic != HELLO_MAGIC {
        return Err(Error::Protocol("not a real-time handshake"));
    }
    if r.read_u8().await? != HELLO_VERSION {
        return Err(Error::Protocol("unsupported real-time version"));
    }
    let mut nonce = [0u8; 16];
    r.read_exact(&mut nonce).await?;
    let port = r.read_u16().await?;
    let n = r.read_u8().await? as usize;
    if n > MAX_CANDIDATES {
        return Err(Error::Protocol("too many addresses"));
    }
    let mut ips = Vec::with_capacity(n);
    for _ in 0..n {
        let mut ip = [0u8; 4];
        r.read_exact(&mut ip).await?;
        ips.push(Ipv4Addr::from(ip));
    }
    Ok((nonce, port, ips))
}

#[derive(Clone, Copy)]
struct ClockSample {
    at: u64,
    rtt: u64,
    offset: i64,
}

struct Driver<C> {
    socket: UdpSocket,
    control: C,
    sealer: Sealer,
    opener: Opener,
    paths: Paths,
    nets: Vec<link::LocalNet>,
    /// This side's networks when they change (`watch_nets`).
    nets_rx: mpsc::UnboundedReceiver<Vec<link::LocalNet>>,
    their_port: u16,
    /// Every address the other side has said it has.
    theirs: Vec<Ipv4Addr>,
    /// The TLS connection still works.
    control_open: bool,
    /// When a path last worked, and when addresses were last sent.
    last_path_us: u64,
    last_addrs_us: u64,
    /// Time to end the session (no path for `NO_PATH_US`).
    ending: bool,
    senders: HashMap<u8, FrameSender>,
    receivers: HashMap<u8, FrameReceiver>,
    ns_per_byte: f64,
    next_send_us: u64,
    clock: Vec<ClockSample>,
    events: mpsc::UnboundedSender<Event>,
    /// Streams whose frames go elsewhere (`Session::subscribe`).
    routes: HashMap<u8, mpsc::UnboundedSender<Frame>>,
    stats: Arc<Mutex<Stats>>,
    epoch: Instant,
    drop_percent: f64,
    rng: u64,
    /// Counters kept here; `stats` gets a copy after each round.
    local: Stats,
    /// How late timers have been firing lately (decays).
    timer_late_us: u64,
    /// Paused (`Handle::pause`): until when, before the session ends.
    paused_until: Option<u64>,
}

impl<C: Control> Driver<C> {
    fn now(&self) -> u64 {
        self.epoch.elapsed().as_micros() as u64
    }

    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<Command>) {
        let mut buf = vec![0u8; 2048];
        let mut control_buf = [0u8; 64];
        loop {
            let now = self.now();
            // Paused for too long: over.
            if self.paused_until.is_some_and(|t| now >= t) {
                self.say_bye().await;
                break;
            }
            let wake = self.housekeeping(now).await;
            self.publish();
            if self.ending {
                break;
            }
            let deadline = self.epoch + Duration::from_micros(wake);
            tokio::select! {
                cmd = commands.recv() => match cmd {
                    // Paused: frames are dropped before they get a number, so the
                    // other side sees no gap; messages still go.
                    Some(Command::Send(f, flags)) if self.paused_until.is_some() && flags & wire::F_STANDALONE == 0 => {
                        drop(f);
                    }
                    Some(Command::Send(f, flags)) => {
                        let now = self.now();
                        self.senders.entry(f.stream).or_insert_with(|| FrameSender::new(f.stream)).push_with(&f, flags, now);
                    }
                    Some(Command::Pause(limit)) => {
                        let now = self.now();
                        let was = self.paused_until.is_some();
                        self.paused_until = limit.map(|l| now + l.as_micros() as u64);
                        if was && limit.is_none() {
                            // The picture since is new: start it afresh.
                            let _ = self.events.send(Event::KeyframeRequested { stream: 0 });
                        }
                    }
                    Some(Command::Subscribe(stream, to)) => {
                        self.routes.insert(stream, to);
                    }
                    Some(Command::NeedKeyframe(stream)) => {
                        self.receiver(stream).need_keyframe();
                    }
                    Some(Command::Close) | None => {
                        self.say_bye().await;
                        break;
                    }
                },
                r = self.socket.recv_from(&mut buf) => {
                    // Errors here are mostly Windows' "connection reset" after
                    // an ICMP unreachable: not fatal.
                    if let Ok((n, from)) = r {
                        if !self.on_packet(&mut buf[..n], from).await {
                            break;
                        }
                    }
                }
                r = self.control.read(&mut control_buf), if self.control_open => match r {
                    Ok(0) => break, // the other side closed it: the session is over
                    Ok(_) => {}
                    Err(_) => {
                        // Its network went away; carry on over the others.
                        self.control_open = false;
                    }
                },
                Some(nets) = self.nets_rx.recv() => self.on_nets(nets).await,
                () = tokio::time::sleep_until(deadline) => {
                    let late = self.now().saturating_sub(wake);
                    self.timer_late_us = late.max(self.timer_late_us * 15 / 16);
                }
            }
        }
        self.publish();
        let _ = self.events.send(Event::Closed);
    }

    /// Tells the other side the session is over: on every path that works,
    /// a few times (the TLS connection may be gone, and a lost bye leaves
    /// the other side waiting for `NO_PATH_US`).
    async fn say_bye(&mut self) {
        let now = self.now();
        for _ in 0..3 {
            for to in self.paths.alive(now) {
                self.send_to(&wire::bye(), to).await;
            }
        }
        if self.control_open {
            let _ = self.control.shutdown().await;
        }
    }

    fn receiver(&mut self, stream: u8) -> &mut FrameReceiver {
        self.receivers.entry(stream).or_insert_with(|| FrameReceiver::new(stream))
    }

    async fn send_to(&mut self, msg: &[u8], to: SocketAddr) {
        let packet = self.sealer.seal(msg);
        let _ = self.socket.send_to(&packet, to).await;
    }

    async fn send_to_current(&mut self, msg: &[u8]) {
        if let Some(p) = self.paths.current() {
            self.send_to(msg, p.remote).await;
        }
    }

    /// Returns false when the session should end.
    async fn on_packet(&mut self, packet: &mut [u8], from: SocketAddr) -> bool {
        let Some(plain) = self.opener.open(packet) else {
            self.local.rejected_packets += 1;
            return true;
        };
        // Genuine, so it's the other device: a path we may not have known.
        if !self.paths.knows(from) {
            let kind = link::kind_toward(&self.nets, from.ip()).unwrap_or(LinkKind::Other);
            self.paths.add(from, kind);
        }
        let plain = plain.to_vec();
        let now = self.now();
        // Proof the link is delivering: recovery deadlines move on.
        for r in self.receivers.values_mut() {
            r.on_packet(now);
        }
        match wire::parse(&plain) {
            Some(Message::Ping(t)) => self.send_to(&wire::pong(t, now), from).await,
            Some(Message::Pong { echo, theirs }) => {
                let rtt = now.saturating_sub(echo);
                self.paths.on_pong(from, rtt, now);
                if self.paths.current().is_some_and(|p| p.remote == from) {
                    self.local.rtt_latest_us = Some(rtt);
                }
                self.clock.retain(|c| now.saturating_sub(c.at) < CLOCK_WINDOW_US);
                self.clock.push(ClockSample {
                    at: now,
                    rtt,
                    offset: (echo + rtt / 2) as i64 - theirs as i64,
                });
            }
            Some(Message::Fragment(f)) => {
                if self.drop_percent > 0.0 && chance(&mut self.rng, self.drop_percent) {
                    self.local.dropped_on_purpose += 1;
                } else {
                    self.receiver(f.stream).on_fragment(&f, now);
                }
            }
            Some(Message::Nack { stream, frame, missing }) => {
                let resends = match self.senders.get_mut(&stream) {
                    Some(s) => s.on_nack(frame, missing.as_deref()),
                    None => Vec::new(),
                };
                for m in resends {
                    self.send_to_current(&m).await;
                }
            }
            Some(Message::KeyRequest { stream }) => {
                if let Some(s) = self.senders.get_mut(&stream) {
                    s.stats.key_requests_received += 1;
                }
                let _ = self.events.send(Event::KeyframeRequested { stream });
            }
            Some(Message::Bye) => return false,
            Some(Message::Addrs(ips)) => {
                for ip in ips {
                    if !self.theirs.contains(&ip) && self.theirs.len() < 32 {
                        self.theirs.push(ip);
                    }
                    if let Some(kind) = link::kind_toward(&self.nets, IpAddr::V4(ip)) {
                        self.paths.add(SocketAddr::new(IpAddr::V4(ip), self.their_port), kind);
                    }
                }
            }
            None => {}
        }
        true
    }

    /// This side's networks changed: paths that are reachable now (or of
    /// another kind), and the other side hears the new addresses at once.
    async fn on_nets(&mut self, nets: Vec<link::LocalNet>) {
        self.nets = nets;
        for &ip in &self.theirs {
            if let Some(kind) = link::kind_toward(&self.nets, IpAddr::V4(ip)) {
                let to = SocketAddr::new(IpAddr::V4(ip), self.their_port);
                self.paths.add(to, kind);
                self.paths.set_kind(to, kind);
            }
        }
        let msg = wire::addrs(&self.our_addrs());
        for to in self.paths.remotes() {
            self.send_to(&msg, to).await;
        }
        self.last_addrs_us = self.now();
    }

    fn our_addrs(&self) -> Vec<Ipv4Addr> {
        self.nets.iter().map(|n| n.ip).take(wire::MAX_ADDRS).collect()
    }

    /// Pings, path choice, recovery and pacing. Returns when to wake next.
    async fn housekeeping(&mut self, now: u64) -> u64 {
        for to in self.paths.due_pings(now) {
            self.send_to(&wire::ping(now), to).await;
        }
        if let Some(change) = self.paths.choose(now) {
            let _ = self.events.send(Event::PathChanged(change));
        }
        if self.paths.current().is_some() {
            self.last_path_us = now;
        } else if now.saturating_sub(self.last_path_us) >= NO_PATH_US {
            self.ending = true;
            return now;
        }
        if now.saturating_sub(self.last_addrs_us) >= ADDRS_EVERY_US {
            self.last_addrs_us = now;
            let msg = wire::addrs(&self.our_addrs());
            for to in self.paths.alive(now) {
                self.send_to(&msg, to).await;
            }
        }
        let rtt = self.paths.current().map_or(5_000, |p| p.rtt_us);

        let mut outgoing = Vec::new();
        let mut busy = false;
        let paused = self.paused_until.is_some();
        for r in self.receivers.values_mut() {
            let polled = r.poll(now, rtt);
            for f in polled.frames {
                if paused {
                    continue; // nothing passes while paused
                }
                let f = match self.routes.get(&f.stream) {
                    Some(to) => match to.send(f) {
                        Ok(()) => continue,
                        Err(back) => back.0, // nobody listening any more
                    },
                    None => f,
                };
                let _ = self.events.send(Event::Frame(f));
            }
            outgoing.extend(polled.nacks);
            if polled.key_request {
                outgoing.push(wire::key_request(r.stream()));
            }
            busy |= r.busy();
        }
        for m in outgoing {
            self.send_to_current(&m).await;
        }

        // Paced sending: up to PACE_BURST_US ahead of the capped rate, plus
        // catching up for however late the timer woke us.
        let catch_up = self.timer_late_us.clamp(PACE_BURST_US, MAX_CATCH_UP_US);
        let mut queued = false;
        if self.paths.current().is_some() {
            let streams: Vec<u8> = self.senders.keys().copied().collect();
            'send: for stream in streams {
                loop {
                    let now = self.now();
                    if self.ns_per_byte > 0.0 && self.next_send_us > now + PACE_BURST_US {
                        queued = true;
                        break 'send;
                    }
                    let Some(msg) = self.senders.get_mut(&stream).and_then(|s| s.pop(now)) else { break };
                    let bytes = (msg.len() + super::crypto::OVERHEAD + 28) as f64;
                    self.next_send_us =
                        self.next_send_us.max(now.saturating_sub(catch_up)) + (bytes * self.ns_per_byte / 1000.0) as u64;
                    self.send_to_current(&msg).await;
                }
            }
        }
        for (&stream, s) in self.senders.iter_mut() {
            if s.take_keyframe_needed() {
                let _ = self.events.send(Event::KeyframeRequested { stream });
            }
            queued |= s.has_queued();
        }

        let mut wake = (now + 50_000).min(self.last_addrs_us + ADDRS_EVERY_US);
        if let Some(t) = self.senders.values().filter_map(|s| s.next_echo_at()).min() {
            wake = wake.min(t);
        }
        if let Some(t) = self.paths.next_ping_at() {
            wake = wake.min(t);
        }
        if busy {
            wake = wake.min(now + POLL_US);
        }
        if let Some(t) = self.paused_until {
            wake = wake.min(t);
        }
        if queued {
            // With no path yet, check again shortly; frames age out meanwhile.
            let at = if self.paths.current().is_some() { self.next_send_us.saturating_sub(PACE_BURST_US) } else { now + POLL_US };
            wake = wake.min(at.max(now + 100));
        }
        wake.max(now + 100)
    }

    fn publish(&mut self) {
        let now = self.now();
        let best = self.clock.iter().min_by_key(|c| c.rtt).copied();
        let mut s = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        s.path = self.paths.current();
        s.paths = self.paths.all(now);
        s.sent = sum_send(self.senders.values().map(|x| &x.stats));
        s.received = sum_receive(self.receivers.values().map(|x| &x.stats));
        s.clock_offset_us = best.map(|c| c.offset);
        s.clock_rtt_us = best.map(|c| c.rtt);
        s.rtt_latest_us = self.local.rtt_latest_us;
        s.rejected_packets = self.local.rejected_packets;
        s.dropped_on_purpose = self.local.dropped_on_purpose;
    }
}

fn sum_send<'a>(all: impl Iterator<Item = &'a SendStats>) -> SendStats {
    all.fold(SendStats::default(), |mut t, s| {
        t.frames += s.frames;
        t.packets += s.packets;
        t.bytes += s.bytes;
        t.nacks_received += s.nacks_received;
        t.resent_packets += s.resent_packets;
        t.dropped_frames += s.dropped_frames;
        t.key_requests_received += s.key_requests_received;
        t
    })
}

fn sum_receive<'a>(all: impl Iterator<Item = &'a ReceiveStats>) -> ReceiveStats {
    all.fold(ReceiveStats::default(), |mut t, s| {
        t.frames += s.frames;
        t.keyframes += s.keyframes;
        t.nacks_sent += s.nacks_sent;
        t.fragments_asked += s.fragments_asked;
        t.fragments_recovered += s.fragments_recovered;
        t.frames_recovered += s.frames_recovered;
        t.resends_unused += s.resends_unused;
        t.given_up += s.given_up;
        t.superseded += s.superseded;
        t.discarded += s.discarded;
        t.key_requests += s.key_requests;
        t
    })
}

/// A tiny random generator for `Config::drop_percent`.
fn chance(state: &mut u64, percent: f64) -> bool {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state % 1_000_000) as f64 / 10_000.0 < percent
}
