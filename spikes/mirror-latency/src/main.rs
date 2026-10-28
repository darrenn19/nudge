//! Screen-mirroring latency spike (throwaway). See README.md.
//!
//! Receives H.264 / H.265 frames from the phone over plain UDP, decodes them on
//! the GPU (Media Foundation), shows them in a window and measures, per frame,
//! how long each stage took: phone encode, network, PC decode, PC draw. The phone
//! and PC clocks are lined up with a ping exchange, so "total" is capture-to-screen.
//!
//! Usage: mirror-latency [label] [--selftest] [--seconds=N] [--snap]   (S in the window saves a full-size PNG)

mod selftest;
mod video;

use std::collections::{BTreeMap, HashSet};
use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PORT: u16 = 47_900;

// Phone -> PC.
const T_FRAG: u8 = 1;
const T_PONG: u8 = 2;
const T_HELLO: u8 = 3;
const T_INFO: u8 = 4;
// PC -> phone.
const T_PING: u8 = 10;
const T_KEY: u8 = 11;
/// frame id u32, count u16 (0xFFFF = the whole frame), then that many fragment indexes u16.
const T_NACK: u8 = 12;

/// Fragment flags.
const F_KEY: u8 = 1;
const F_HEVC: u8 = 2;
/// A resend, answering a NACK.
const F_RESENT: u8 = 4;
/// The encoder uses intra-refresh: a lost frame heals by itself, no keyframe needed.
const F_INTRA_REFRESH: u8 = 8;

/// type u8, frame id u32, fragment u16, count u16, flags u8, capture us u64, encoded us u64.
const FRAG_HEADER: usize = 26;

/// How many times to ask for the same missing fragment.
const NACK_TRIES: u32 = 3;
/// Shortest wait before asking again.
const MIN_RETRY_US: u64 = 3_000;
/// Give up on a frame (and ask for a keyframe) this long after noticing it's incomplete.
const GIVE_UP_US: u64 = 50_000;
/// A frame with no new fragment for this long has a gap at the end.
const STALL_US: u64 = 30_000;
/// Between keyframe requests while waiting for one.
const KEY_REQUEST_EVERY_US: u64 = 150_000;

pub(crate) fn now_us() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_micros() as u64
}

struct Frame {
    key: bool,
    hevc: bool,
    /// Phone clock (µs): when the compositor produced the frame.
    capture: u64,
    /// Phone clock (µs): when the encoder handed it out.
    encoded: u64,
    /// PC clock (µs): when its last fragment arrived.
    received: u64,
    data: Vec<u8>,
}

#[derive(Clone, Copy)]
struct ClockSample {
    at: u64,
    rtt: u64,
    /// PC clock minus phone clock.
    offset: i64,
}

#[derive(Default)]
struct Stats {
    nacks: u64,
    frags_asked: u64,
    frags_recovered: u64,
    /// Resent fragments that arrived after the frame was complete or given up.
    resent_unused: u64,
    frames_recovered: u64,
    /// From noticing the gap to the frame being complete (µs).
    recovery: Vec<i64>,
    given_up: u64,
    key_requests: u64,
    keyframes: u64,
    sim_dropped: u64,
    /// Skipped because a later keyframe made them unnecessary.
    superseded: u64,
    /// How long the picture stayed frozen after each give-up (µs).
    freezes: Vec<i64>,
}

#[derive(Default)]
struct Shared {
    clock: Vec<ClockSample>,
    want_key: bool,
    bytes: u64,
    first_id: Option<u32>,
    last_id: u32,
    delivered: u64,
    stats: Stats,
}

impl Shared {
    /// The offset from the lowest-RTT ping of the last 10 s: the most precise one.
    fn offset(&self) -> Option<(i64, u64)> {
        self.clock.iter().min_by_key(|s| s.rtt).map(|s| (s.offset, s.rtt))
    }
}

struct Partial {
    /// Empty until the first fragment arrives (the count isn't known before).
    parts: Vec<Option<Vec<u8>>>,
    have: usize,
    /// Highest fragment index seen, plus one.
    top: usize,
    flags: u8,
    capture: u64,
    encoded: u64,
    /// When its latest fragment arrived (or when it was found missing).
    last_seen: u64,
    /// When we first noticed something missing.
    gap_since: Option<u64>,
    /// Per fragment: times asked for, and when last.
    asks: Vec<(u32, u64)>,
    /// The same, for the whole frame while none of it has arrived.
    whole_asks: (u32, u64),
    resent: bool,
}

impl Partial {
    fn new(now: u64) -> Self {
        Partial {
            parts: Vec::new(),
            have: 0,
            top: 0,
            flags: 0,
            capture: 0,
            encoded: 0,
            last_seen: now,
            gap_since: None,
            asks: Vec::new(),
            whole_asks: (0, 0),
            resent: false,
        }
    }
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(b[i..i + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().unwrap())
}

/// Puts frames back together, asks the phone to resend what's missing, and hands
/// complete frames to the decoder in order. Same rules as `core/src/realtime/frames.rs`.
struct Receiver {
    sock: UdpSocket,
    phone: Option<SocketAddr>,
    partials: BTreeMap<u32, Partial>,
    /// Complete frames waiting for an earlier one.
    ready: BTreeMap<u32, Frame>,
    /// The next frame the decoder needs.
    next: Option<u32>,
    highest: Option<u32>,
    /// The decoder can't continue without a keyframe.
    need_key: bool,
    intra_refresh: bool,
    srtt: Option<u64>,
    /// Smoothed time from a NACK to its resend arriving.
    resend_rtt: Option<u64>,
    last_key_req: Option<u64>,
    /// When the picture froze (a frame was given up), until the next frame is shown.
    frozen_since: Option<u64>,
}

impl Receiver {
    fn reset(&mut self) {
        self.partials.clear();
        self.ready.clear();
        self.next = None;
        self.highest = None;
        self.need_key = true;
        self.frozen_since = None;
    }

    fn send(&self, p: &[u8]) {
        if let Some(addr) = self.phone {
            let _ = self.sock.send_to(p, addr);
        }
    }

    /// Wait this long for an answer before asking again.
    fn retry_us(&self) -> u64 {
        (self.srtt.unwrap_or(5_000).max(self.resend_rtt.unwrap_or(0)) * 3 / 2).max(MIN_RETRY_US)
    }

    /// From now on only a keyframe will do: ask for one right away.
    fn start_needing_key(&mut self) {
        if !self.need_key {
            self.need_key = true;
            self.last_key_req = None;
        }
    }

    fn on_fragment(&mut self, b: &[u8], now: u64, s: &mut Stats) {
        let id = u32_at(b, 1);
        let (frag, count) = (u16_at(b, 5) as usize, u16_at(b, 7) as usize);
        let flags = b[9];
        let resent = flags & F_RESENT != 0;
        if frag >= count {
            return;
        }
        if self.next.is_some_and(|n| id < n) {
            s.resent_unused += resent as u64;
            return; // already shown or given up
        }
        if self.next.is_some_and(|n| id - n > 600) {
            self.reset(); // ten seconds behind, or the phone restarted: start over
        }
        if self.next.is_none() {
            // The first frame is usually a keyframe: give it time to arrive.
            self.last_key_req = Some(now);
        }
        let next = *self.next.get_or_insert(id);
        // Frames we haven't heard from at all, between the newest one and this one.
        for gap in self.highest.map_or(next, |h| h + 1)..id {
            self.partials.entry(gap).or_insert_with(|| Partial::new(now));
        }
        if self.highest.is_none_or(|h| id > h) {
            self.highest = Some(id);
        }
        if self.ready.contains_key(&id) {
            s.resent_unused += resent as u64;
            return;
        }

        let p = self.partials.entry(id).or_insert_with(|| Partial::new(now));
        if p.parts.is_empty() {
            p.parts = vec![None; count];
            p.asks = vec![(0, 0); count];
            p.flags = flags & !F_RESENT;
            p.capture = u64_at(b, 10);
            p.encoded = u64_at(b, 18);
        }
        if p.parts.len() != count || p.parts[frag].is_some() {
            s.resent_unused += resent as u64;
            return;
        }
        p.parts[frag] = Some(b[FRAG_HEADER..].to_vec());
        p.have += 1;
        p.top = p.top.max(frag + 1);
        p.last_seen = now;
        if resent {
            // Only after a single NACK is it clear which one this answers.
            let (asked, at) = p.asks[frag];
            if asked == 1 || asked == 0 && p.whole_asks.0 == 1 {
                let at = if asked == 1 { at } else { p.whole_asks.1 };
                let sample = now.saturating_sub(at);
                self.resend_rtt = Some(self.resend_rtt.map_or(sample, |r| (r * 7 + sample) / 8));
            }
            p.resent = true;
            s.frags_recovered += 1;
        }
        if p.have < count {
            return;
        }

        let p = self.partials.remove(&id).unwrap();
        if p.resent {
            s.frames_recovered += 1;
            if let Some(g) = p.gap_since {
                s.recovery.push((now - g) as i64);
            }
        }
        self.intra_refresh = p.flags & F_INTRA_REFRESH != 0;
        let frame = Frame {
            key: p.flags & F_KEY != 0,
            hevc: p.flags & F_HEVC != 0,
            capture: p.capture,
            encoded: p.encoded,
            received: now,
            data: p.parts.into_iter().flatten().flatten().collect(),
        };
        self.ready.insert(id, frame);
    }

    /// Sends NACKs for gaps, gives up on frames that can't be recovered, and hands
    /// frames to the decoder in order. Returns false once the decoder has gone.
    fn poll(&mut self, now: u64, shared: &Mutex<Shared>, tx: &Sender<Frame>) -> bool {
        let retry = self.retry_us();
        let mut sh = shared.lock().unwrap();
        if std::mem::take(&mut sh.want_key) {
            self.start_needing_key();
        }

        let mut nacks = Vec::new();
        for (&id, p) in self.partials.iter_mut() {
            // Missing: a later frame has started, a later fragment of this one has
            // arrived, or nothing more has come for a while.
            let behind = self.highest.is_some_and(|h| id < h) || now.saturating_sub(p.last_seen) > STALL_US;
            // Ask for a fragment again only after `retry`, at most NACK_TRIES times.
            let due = |(tries, last): (u32, u64)| tries < NACK_TRIES && (tries == 0 || now.saturating_sub(last) >= retry);
            let mut m = vec![T_NACK];
            m.extend_from_slice(&id.to_le_bytes());
            if p.parts.is_empty() {
                p.gap_since.get_or_insert(now);
                if due(p.whole_asks) {
                    p.whole_asks = (p.whole_asks.0 + 1, now);
                    m.extend_from_slice(&0xFFFFu16.to_le_bytes());
                    sh.stats.nacks += 1;
                    sh.stats.frags_asked += 1;
                    nacks.push(m);
                }
                continue;
            }
            let upto = if behind { p.parts.len() } else { p.top };
            let missing: Vec<usize> = (0..upto).filter(|&i| p.parts[i].is_none()).collect();
            if missing.is_empty() {
                continue;
            }
            p.gap_since.get_or_insert(now);
            let ask: Vec<usize> = missing.into_iter().filter(|&i| due(p.asks[i])).take(600).collect();
            if ask.is_empty() {
                continue;
            }
            m.extend_from_slice(&(ask.len() as u16).to_le_bytes());
            for &i in &ask {
                p.asks[i] = (p.asks[i].0 + 1, now);
                m.extend_from_slice(&(i as u16).to_le_bytes());
            }
            sh.stats.nacks += 1;
            sh.stats.frags_asked += ask.len() as u64;
            nacks.push(m);
        }
        for m in &nacks {
            self.send(m);
        }

        while let Some(n) = self.next {
            if let Some(f) = self.ready.remove(&n) {
                self.next = Some(n + 1);
                if self.need_key {
                    if !f.key {
                        continue; // the decoder can't use it
                    }
                    self.need_key = false;
                }
                if let Some(t) = self.frozen_since.take() {
                    sh.stats.freezes.push((now - t) as i64);
                }
                sh.delivered += 1;
                sh.stats.keyframes += f.key as u64;
                if tx.send(f).is_err() {
                    return false;
                }
                continue;
            }
            // A complete keyframe further on: nothing before it is needed.
            if let Some(k) = self.ready.iter().find(|(_, f)| f.key).map(|(&id, _)| id) {
                sh.stats.superseded += (k - n) as u64;
                self.partials.retain(|&id, _| id > k);
                self.ready.retain(|&id, _| id >= k);
                self.next = Some(k);
                continue;
            }
            let Some(p) = self.partials.get(&n) else { break };
            // By time only: resends may be slow to come, and a keyframe costs
            // more than waiting.
            if !p.gap_since.is_some_and(|g| now.saturating_sub(g) >= GIVE_UP_US) {
                break;
            }
            self.partials.remove(&n);
            self.next = Some(n + 1);
            sh.stats.given_up += 1;
            self.frozen_since.get_or_insert(now);
            // With intra-refresh the decoder carries on and the picture heals by
            // itself; otherwise later frames reference the lost one.
            if !self.intra_refresh {
                self.start_needing_key();
            }
        }

        // A keyframe already on its way will do.
        let key_coming = self.partials.values().any(|p| p.flags & F_KEY != 0);
        if self.need_key
            && !key_coming
            && self.phone.is_some()
            && self.last_key_req.is_none_or(|t| now.saturating_sub(t) >= KEY_REQUEST_EVERY_US)
        {
            self.last_key_req = Some(now);
            sh.stats.key_requests += 1;
            drop(sh);
            self.send(&[T_KEY]);
        }
        true
    }
}

/// A tiny random generator for simulated loss.
fn chance(state: &mut u64, percent: f64) -> bool {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state % 1_000_000) as f64 / 10_000.0 < percent
}

fn network(sock: UdpSocket, tx: Sender<Frame>, shared: Arc<Mutex<Shared>>, drop_percent: f64) {
    let mut buf = [0u8; 2048];
    let mut r = Receiver {
        sock: sock.try_clone().expect("socket"),
        phone: None,
        partials: BTreeMap::new(),
        ready: BTreeMap::new(),
        next: None,
        highest: None,
        need_key: true,
        intra_refresh: false,
        srtt: None,
        resend_rtt: None,
        last_key_req: None,
        frozen_since: None,
    };
    let mut last_ping = 0u64;
    let mut rng = 0x9E37_79B9_7F4A_7C15u64;
    let mut info_seen = HashSet::new();

    loop {
        let now = now_us();
        if r.phone.is_some() && now - last_ping >= 100_000 {
            let mut p = vec![T_PING];
            p.extend_from_slice(&now.to_le_bytes());
            r.send(&p);
            last_ping = now;
        }
        if !r.poll(now, &shared, &tx) {
            return;
        }

        let (n, from) = match sock.recv_from(&mut buf) {
            Ok(x) => x,
            // Timeouts, and Windows' "connection reset" after an ICMP unreachable.
            Err(_) => continue,
        };
        let b = &buf[..n];
        if b.is_empty() {
            continue;
        }
        if b[0] == T_HELLO || r.phone.is_none() {
            if r.phone != Some(from) {
                println!("Phone connected from {from}");
            }
            r.phone = Some(from);
            if b[0] == T_HELLO {
                // A fresh stream: start over.
                r.reset();
                *shared.lock().unwrap() = Shared::default();
                info_seen.clear();
            }
        }

        let now = now_us();
        match b[0] {
            T_PONG if n >= 17 => {
                let (pc_t0, phone_t) = (u64_at(b, 1), u64_at(b, 9));
                let rtt = now.saturating_sub(pc_t0);
                r.srtt = Some(r.srtt.map_or(rtt, |s| (s * 7 + rtt) / 8));
                let offset = (pc_t0 + rtt / 2) as i64 - phone_t as i64;
                let mut s = shared.lock().unwrap();
                s.clock.retain(|c| now - c.at < 10_000_000);
                s.clock.push(ClockSample { at: now, rtt, offset });
            }
            T_INFO => {
                let text = String::from_utf8_lossy(&b[1..]).into_owned();
                if info_seen.insert(text.clone()) {
                    println!("Phone: {text}");
                }
            }
            T_FRAG if n > FRAG_HEADER => {
                let mut s = shared.lock().unwrap();
                if drop_percent > 0.0 && chance(&mut rng, drop_percent) {
                    s.stats.sim_dropped += 1;
                    continue;
                }
                let id = u32_at(b, 1);
                s.bytes += n as u64;
                s.first_id.get_or_insert(id);
                s.last_id = s.last_id.max(id);
                r.on_fragment(b, now, &mut s.stats);
            }
            _ => {}
        }
    }
}

/// Per-frame timings in µs.
#[derive(Clone, Copy)]
struct Record {
    key: bool,
    encode: i64,
    network: i64,
    decode: i64,
    draw: i64,
    total: i64,
}

fn percentile(v: &mut [i64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_unstable();
    let i = ((v.len() - 1) as f64 * p).round() as usize;
    v[i] as f64 / 1000.0
}

fn summary(label: &str, records: &[Record], shared: &Shared, seconds: f64) {
    println!("\n=== {label}: {} frames shown in {seconds:.0} s ===", records.len());
    let span = shared.first_id.map_or(0, |f| (shared.last_id - f + 1) as u64);
    println!(
        "fps shown {:.1} | frames lost or skipped {} of {} | average {:.1} Mbit/s",
        records.len() as f64 / seconds,
        span.saturating_sub(shared.delivered),
        span,
        shared.bytes as f64 * 8.0 / seconds / 1e6
    );
    println!("{:<22} {:>7} {:>7} {:>7} {:>7}", "stage (ms)", "p50", "p95", "p99", "max");
    let stages: [(&str, fn(&Record) -> i64); 5] = [
        ("phone encode", |r| r.encode),
        ("network", |r| r.network),
        ("PC queue + decode", |r| r.decode),
        ("PC draw", |r| r.draw),
        ("TOTAL capture->screen", |r| r.total),
    ];
    for (name, f) in stages {
        let mut v: Vec<i64> = records.iter().map(f).collect();
        let (p50, p95, p99) = (percentile(&mut v, 0.5), percentile(&mut v, 0.95), percentile(&mut v, 0.99));
        let max = percentile(&mut v, 1.0);
        println!("{name:<22} {p50:>7.1} {p95:>7.1} {p99:>7.1} {max:>7.1}");
    }
    let st = &shared.stats;
    let mut rec = st.recovery.clone();
    println!(
        "loss recovery: {} NACKs for {} fragments; {} fragments resent in time, {} too late; \
         {} frames recovered (gap to complete: p50 {:.1} ms, p95 {:.1} ms); {} frames given up",
        st.nacks,
        st.frags_asked,
        st.frags_recovered,
        st.resent_unused,
        st.frames_recovered,
        percentile(&mut rec, 0.5),
        percentile(&mut rec, 0.95),
        st.given_up,
    );
    let mut fr = st.freezes.clone();
    let frozen: i64 = fr.iter().sum();
    println!(
        "freezes (a frame given up, picture stuck until a keyframe): {} | total {:.0} ms | each p50 {:.0} ms, max {:.0} ms | {} frames skipped for a keyframe",
        fr.len(),
        frozen as f64 / 1000.0,
        percentile(&mut fr, 0.5),
        percentile(&mut fr, 1.0),
        st.superseded,
    );
    println!(
        "keyframes: {} shown, {} requested{}",
        st.keyframes,
        st.key_requests,
        if st.sim_dropped > 0 { format!(" | {} packets dropped on purpose (--drop)", st.sim_dropped) } else { String::new() },
    );
    if let Some((_, rtt)) = shared.offset() {
        println!("clock sync: best ping RTT {:.1} ms (error at most half that)", rtt as f64 / 1000.0);
    }
    println!("Add ~1 frame (8-17 ms) for the PC monitor and ~1 frame for the phone's own display pipeline.");
}

fn write_csv(label: &str, records: &[Record]) {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let name = format!("mirror-{label}-{stamp}.csv");
    let mut out = String::from("key,encode_us,network_us,decode_us,draw_us,total_us\n");
    for r in records {
        out += &format!("{},{},{},{},{},{}\n", r.key as u8, r.encode, r.network, r.decode, r.draw, r.total);
    }
    match std::fs::write(&name, out) {
        Ok(()) => println!("Per-frame data: {name}"),
        Err(e) => eprintln!("Couldn't write {name}: {e}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let selftest = args.iter().any(|a| a == "--selftest");
    // --seconds=N stops by itself; --snap saves a PNG after 3 s (both for unattended runs).
    let stop_after = args.iter().find_map(|a| a.strip_prefix("--seconds=")).and_then(|s| s.parse::<u64>().ok());
    // --drop=P throws away P% of the video packets on arrival, to exercise loss recovery.
    let drop_percent = args.iter().find_map(|a| a.strip_prefix("--drop=")).and_then(|s| s.parse::<f64>().ok());
    let drop_percent = drop_percent.unwrap_or(0.0);
    let mut auto_snap = args.iter().any(|a| a == "--snap");
    let label = args.iter().find(|a| !a.starts_with("--")).cloned().unwrap_or_else(|| "run".into());
    now_us();
    // 1 ms timer resolution, so short waits don't round up to 15.6 ms.
    unsafe {
        windows::Win32::Media::timeBeginPeriod(1);
    }

    println!("Nudge mirror latency spike. Label: {label}");
    if selftest {
        println!("Self-test: a fake phone on this PC sends a generated video.");
    } else {
        println!("Type one of these into the phone's Mirror spike screen:");
        for i in if_addrs::get_if_addrs().unwrap_or_default() {
            if let std::net::IpAddr::V4(ip) = i.ip() {
                if !ip.is_loopback() && !ip.is_link_local() {
                    println!("  {ip:<16} {}", i.name);
                }
            }
        }
    }
    if drop_percent > 0.0 {
        println!("Dropping {drop_percent}% of video packets on purpose.");
    }
    println!("Listening on UDP {PORT}. Press S in the window to save a full-size PNG; Esc, Ctrl+C or close to stop.\n");

    let sock = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))
        .expect("socket");
    // Keyframes arrive as a burst of ~100+ packets; the default buffer drops some.
    let _ = sock.set_recv_buffer_size(8 << 20);
    sock.bind(&SocketAddr::from(([0, 0, 0, 0], PORT)).into())
        .unwrap_or_else(|e| panic!("can't listen on UDP {PORT}: {e}"));
    let sock: UdpSocket = sock.into();
    // Short, so NACKs and give-ups happen on time even when no packets arrive.
    sock.set_read_timeout(Some(Duration::from_millis(1))).unwrap();

    let shared = Arc::new(Mutex::new(Shared::default()));
    let (tx, rx) = mpsc::channel::<Frame>();
    {
        let shared = shared.clone();
        std::thread::spawn(move || network(sock, tx, shared, drop_percent));
    }
    if selftest {
        selftest::start(PORT);
    }

    let mut video = video::Video::new().expect("GPU video setup");
    video::catch_ctrl_c();
    let mut records: Vec<Record> = Vec::new();
    let mut started: Option<u64> = None;
    let (mut last_report, mut reported) = (now_us(), 0usize);
    let mut snapshots = 0;
    // The newest decoded picture, waiting for the display to be ready for it.
    let mut pending: Option<(Frame, video::Picture, u64)> = None;
    let mut replaced = 0u64;

    while video::pump() {
        let mut batch: Vec<Frame> = rx.try_iter().collect();
        if batch.is_empty() {
            let wait = if pending.is_some() { 1 } else { 2 };
            match rx.recv_timeout(Duration::from_millis(wait)) {
                Ok(f) => batch.push(f),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            batch.extend(rx.try_iter());
        }

        // Every frame must be decoded (later frames reference it), but only the
        // newest picture is shown: if we fall behind, stale frames are skipped.
        for f in batch {
            match video.decode(&f.data, f.hevc, f.capture) {
                Ok(Some(picture)) => {
                    replaced += pending.is_some() as u64;
                    pending = Some((f, picture, now_us()));
                }
                Ok(None) => {}
                Err(e) => {
                    eprintln!("decode error: {e}");
                    shared.lock().unwrap().want_key = true;
                }
            }
        }

        // Don't block on the display: draw when the swap chain can take a frame
        // right away; until then, newer frames replace the pending one. But if
        // the display stops signalling (window starting up, partly covered),
        // draw anyway after a refresh or so, or the picture looks frozen.
        let overdue = pending.as_ref().is_some_and(|p| now_us().saturating_sub(p.2) > 20_000);
        if pending.is_some() && (video.ready_to_present() || overdue) {
            let (f, picture, decoded) = pending.take().unwrap();
            if let Err(e) = video.present(&picture) {
                eprintln!("display error: {e}");
            }
            let drawn = now_us();
            let auto = auto_snap && started.is_some_and(|s| drawn - s > 3_000_000);
            if video::snapshot_requested() || auto {
                auto_snap = false;
                snapshots += 1;
                let path = format!("snap-{label}-{snapshots}.png");
                match video.snapshot(&picture, &path) {
                    Ok(()) => println!("Saved {path}"),
                    Err(e) => eprintln!("Couldn't save {path}: {e}"),
                }
            }

            if let Some((offset, _)) = shared.lock().unwrap().offset() {
                let pc = |phone_t: u64| phone_t as i64 + offset;
                started.get_or_insert(drawn);
                records.push(Record {
                    key: f.key,
                    encode: f.encoded as i64 - f.capture as i64,
                    network: f.received as i64 - pc(f.encoded),
                    decode: decoded as i64 - f.received as i64,
                    draw: drawn as i64 - decoded as i64,
                    total: drawn as i64 - pc(f.capture),
                });
            }
        }

        let now = now_us();
        if stop_after.is_some_and(|secs| started.is_some_and(|s| now - s > secs * 1_000_000)) {
            break;
        }
        if now - last_report >= 2_000_000 && records.len() > reported {
            let mut totals: Vec<i64> = records[reported..].iter().map(|r| r.total).collect();
            let s = shared.lock().unwrap();
            let rtt = s.offset().map_or(f64::NAN, |o| o.1 as f64 / 1000.0);
            println!(
                "{:5.1} fps | total p50 {:5.1} ms, p95 {:5.1} ms | ping {rtt:4.1} ms | recovered {} given up {} freezes {} keyframes {}",
                (records.len() - reported) as f64 / ((now - last_report) as f64 / 1e6),
                percentile(&mut totals, 0.5),
                percentile(&mut totals, 0.95),
                s.stats.frames_recovered,
                s.stats.given_up,
                s.stats.freezes.len(),
                s.stats.keyframes,
            );
            (last_report, reported) = (now, records.len());
        }
    }

    let seconds = started.map_or(0.0, |s| (now_us() - s) as f64 / 1e6).max(0.001);
    summary(&label, &records, &shared.lock().unwrap(), seconds);
    println!("{replaced} decoded pictures replaced by a newer one before the display was ready");
    write_csv(&label, &records);
}
