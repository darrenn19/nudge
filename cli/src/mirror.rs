//! `nudge mirror-bench`: the real-time channel with generated frames.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use nudge_core::realtime::{Config, Event, OutFrame, Session, Stats};
use nudge_core::{Identity, Tls, TrustedPeers};
use tokio::net::{TcpListener, TcpStream};

/// What the sending side generates.
#[derive(Clone, Copy)]
pub struct MirrorSend {
    pub mbps: u32,
    pub fps: u32,
    pub seconds: u64,
    pub keyframe_every: u32,
}

pub async fn bench(
    identity_dir: &Path,
    listen: bool,
    to: Option<String>,
    port: u16,
    send: MirrorSend,
    config: Config,
) -> Result<()> {
    let _timer = FineTimer::new();
    if listen {
        let tls = super::load_tls(identity_dir)?;
        let listener = TcpListener::bind(("0.0.0.0", port))
            .await
            .with_context(|| format!("couldn't listen on port {port}"))?;
        println!("Waiting for a mirror-bench sender on port {port}...");
        let (tcp, addr) = listener.accept().await?;
        tcp.set_nodelay(true)?;
        let (stream, _) = tls.accept(tcp).await.inspect_err(super::hint_trust)?;
        println!("Sender connected from {addr}");
        receive(Session::accept(stream, config).await?).await
    } else if let Some(to) = to {
        let tls = super::load_tls(identity_dir)?;
        let addr = if to.contains(':') { to } else { format!("{to}:{port}") };
        let tcp = TcpStream::connect(&addr)
            .await
            .with_context(|| format!("couldn't connect to {addr}"))?;
        tcp.set_nodelay(true)?;
        let (stream, _) = tls.connect(tcp).await.inspect_err(super::hint_trust)?;
        send_frames(Session::open(stream, config).await?, send).await
    } else {
        local(send, config).await
    }
}

/// Both ends on this machine, with throwaway identities.
async fn local(send: MirrorSend, config: Config) -> Result<()> {
    let (a, b) = (Identity::generate()?, Identity::generate()?);
    let receiver_tls = Tls::new(&a, TrustedPeers::new([b.fingerprint()]))?;
    let sender_tls = Tls::new(&b, TrustedPeers::new([a.fingerprint()]))?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let receiving = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await?;
        tcp.set_nodelay(true)?;
        let (stream, _) = receiver_tls.accept(tcp).await?;
        receive(Session::accept(stream, config).await?).await
    });
    let tcp = TcpStream::connect(addr).await?;
    tcp.set_nodelay(true)?;
    let (stream, _) = sender_tls.connect(tcp).await?;
    let sender_config = Config {
        drop_percent: 0.0,
        ..Config::default()
    };
    send_frames(Session::open(stream, sender_config).await?, send).await?;
    receiving.await?
}

async fn send_frames(mut session: Session, s: MirrorSend) -> Result<()> {
    let n = s.keyframe_every.max(1) as f64;
    let avg = s.mbps as f64 * 1e6 / 8.0 / s.fps.max(1) as f64;
    // Keyframes 8× the others, keeping the average bitrate.
    let p_size = ((n * avg - 8.0 * avg) / (n - 1.0).max(1.0)).max(avg / 4.0) as usize;
    let k_size = (avg * 8.0) as usize;
    println!(
        "Sending {} Mbit/s at {} fps for {} s: frames ~{} KB, keyframes ~{} KB",
        s.mbps,
        s.fps,
        s.seconds,
        p_size / 1000,
        k_size / 1000
    );

    let mut tick = tokio::time::interval(Duration::from_secs_f64(1.0 / s.fps.max(1) as f64));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let total = s.seconds * s.fps as u64;
    let mut force_key = false;
    let mut requested = 0u64;
    for i in 0..total {
        tick.tick().await;
        while let Some(e) = session.try_event() {
            match e {
                Event::KeyframeRequested { .. } => {
                    force_key = true;
                    requested += 1;
                }
                Event::PathChanged(p) => println!("Path: {}", describe_path(p.as_ref())),
                Event::Closed => anyhow::bail!("the receiver ended the session"),
                Event::Frame(_) => {}
            }
        }
        let key = i % s.keyframe_every.max(1) as u64 == 0 || std::mem::take(&mut force_key);
        let size = if key { k_size } else { p_size };
        session.send(OutFrame {
            stream: 0,
            data: vec![i as u8; size],
            key,
            intra_refresh: false,
            capture_us: session.now_us(),
        })?;
    }
    // Let the last resends go out.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let st = session.stats();
    println!(
        "Sent {} frames, {} packets ({:.1} MB); resent {} packets for {} NACKs; \
         dropped {} frames that waited too long; keyframes asked for: {requested}",
        st.sent.frames,
        st.sent.packets,
        st.sent.bytes as f64 / 1e6,
        st.sent.resent_packets,
        st.sent.nacks_received,
        st.sent.dropped_frames,
    );
    print_paths(&st);
    session.close();
    tokio::time::sleep(Duration::from_millis(100)).await;
    Ok(())
}

async fn receive(mut session: Session) -> Result<()> {
    let mut latency: Vec<u64> = Vec::new();
    let mut window: Vec<u64> = Vec::new();
    let mut last_report = session.now_us();
    let mut first: Option<u64> = None;
    while let Some(e) = session.next_event().await {
        match e {
            Event::Frame(f) => {
                first.get_or_insert(f.received_us);
                if let Some(captured) = session.to_local_us(f.capture_us) {
                    let l = f.received_us.saturating_sub(captured);
                    latency.push(l);
                    window.push(l);
                }
            }
            Event::PathChanged(p) => println!("Path: {}", describe_path(p.as_ref())),
            Event::KeyframeRequested { .. } => {}
            Event::Closed => break,
        }
        let now = session.now_us();
        if now - last_report >= 2_000_000 && !window.is_empty() {
            let st = session.stats();
            println!(
                "{:5.1} fps | latency p50 {:5.1} ms, p95 {:5.1} ms | recovered {} given up {} | {}",
                window.len() as f64 / ((now - last_report) as f64 / 1e6),
                pct(&mut window, 0.5),
                pct(&mut window, 0.95),
                st.received.frames_recovered,
                st.received.given_up,
                describe_path(st.path.as_ref()),
            );
            window.clear();
            last_report = now;
        }
    }

    let st = session.stats();
    let seconds = first.map_or(0.0, |f| (session.now_us() - f) as f64 / 1e6);
    let r = &st.received;
    println!("\n=== Received {} frames in {seconds:.1} s ===", r.frames);
    println!(
        "send-to-arrival latency: p50 {:.1} ms, p95 {:.1} ms, p99 {:.1} ms, max {:.1} ms",
        pct(&mut latency, 0.5),
        pct(&mut latency, 0.95),
        pct(&mut latency, 0.99),
        pct(&mut latency, 1.0),
    );
    println!(
        "loss recovery: {} NACKs for {} fragments; {} fragments resent in time ({} unused); \
         {} frames recovered; {} given up; {} skipped for a keyframe; {} unusable; {} keyframe requests",
        r.nacks_sent,
        r.fragments_asked,
        r.fragments_recovered,
        r.resends_unused,
        r.frames_recovered,
        r.given_up,
        r.superseded,
        r.discarded,
        r.key_requests,
    );
    if st.rejected_packets > 0 {
        println!("{} packets rejected (failed decryption, replayed, or too far out of order)", st.rejected_packets);
    }
    if st.dropped_on_purpose > 0 {
        println!("{} fragments dropped on purpose (--drop)", st.dropped_on_purpose);
    }
    if let Some(rtt) = st.clock_rtt_us {
        println!("clock sync: best ping {:.2} ms (error at most half that)", rtt as f64 / 1000.0);
    }
    print_paths(&st);
    Ok(())
}

fn print_paths(st: &Stats) {
    for (p, alive) in &st.paths {
        let using = st.path.is_some_and(|c| c.remote == p.remote);
        println!(
            "  path {:<22} {:<8} rtt {:5.2} ms{}{}",
            p.remote.to_string(),
            p.kind.name(),
            p.rtt_us as f64 / 1000.0,
            if *alive { "" } else { "  (dead)" },
            if using { "  <- in use" } else { "" },
        );
    }
}

fn describe_path(p: Option<&nudge_core::realtime::PathInfo>) -> String {
    match p {
        Some(p) => format!("{} via {} (rtt {:.2} ms)", p.remote, p.kind.name(), p.rtt_us as f64 / 1000.0),
        None => "none".into(),
    }
}

fn pct(v: &mut [u64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_unstable();
    v[((v.len() - 1) as f64 * p).round() as usize] as f64 / 1000.0
}

/// 1 ms timers while benchmarking, as the Windows app does while mirroring
/// (Windows otherwise wakes sleeping threads every ~16 ms). `NUDGE_COARSE_TIMER=1`
/// leaves the default, to compare.
struct FineTimer(bool);

#[cfg(windows)]
#[allow(unsafe_code)]
mod winmm {
    #[link(name = "winmm")]
    unsafe extern "system" {
        fn timeBeginPeriod(period: u32) -> u32;
        fn timeEndPeriod(period: u32) -> u32;
    }
    pub fn set(fine: bool) {
        // SAFETY: no pointers; each end matches a begin.
        unsafe {
            if fine {
                timeBeginPeriod(1);
            } else {
                timeEndPeriod(1);
            }
        }
    }
}

#[cfg(not(windows))]
mod winmm {
    pub fn set(_fine: bool) {}
}

impl FineTimer {
    fn new() -> Self {
        let on = std::env::var_os("NUDGE_COARSE_TIMER").is_none();
        if on {
            winmm::set(true);
        }
        Self(on)
    }
}

impl Drop for FineTimer {
    fn drop(&mut self) {
        if self.0 {
            winmm::set(false);
        }
    }
}
