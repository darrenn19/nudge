//! The real-time channel end to end: TLS handshake, UDP, loss recovery.

use std::time::Duration;

use nudge_core::realtime::{Config, Event, OutFrame, Session};
use nudge_core::{Identity, Tls, TrustedPeers};
use tokio::net::{TcpListener, TcpStream};

fn paired_tls() -> (Tls, Tls) {
    let (a, b) = (Identity::generate().unwrap(), Identity::generate().unwrap());
    let ta = TrustedPeers::new([b.fingerprint()]);
    let tb = TrustedPeers::new([a.fingerprint()]);
    (Tls::new(&a, ta).unwrap(), Tls::new(&b, tb).unwrap())
}

/// A laptop (`open`) and a phone (`accept`) with a session between them.
async fn sessions(laptop_config: Config, phone_config: Config) -> (Session, Session) {
    let (laptop, phone) = paired_tls();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let accepting = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let (stream, _) = phone.accept(tcp).await.unwrap();
        Session::accept(stream, phone_config).await.unwrap()
    });
    let (stream, _) = laptop.connect(TcpStream::connect(addr).await.unwrap()).await.unwrap();
    let laptop_session = Session::open(stream, laptop_config).await.unwrap();
    (laptop_session, accepting.await.unwrap())
}

fn frame_data(i: usize) -> Vec<u8> {
    let len = if i % 30 == 0 { 60_000 } else { 4_000 + (i * 997) % 20_000 };
    (0..len).map(|j| (j ^ i) as u8).collect()
}

/// Waits for events until `done` says stop, or fails after `secs`.
async fn collect(s: &mut Session, secs: u64, mut done: impl FnMut(&Event) -> bool) -> Vec<Event> {
    let mut out = Vec::new();
    let r = tokio::time::timeout(Duration::from_secs(secs), async {
        while let Some(e) = s.next_event().await {
            let stop = done(&e);
            out.push(e);
            if stop {
                break;
            }
        }
    })
    .await;
    assert!(r.is_ok(), "timed out; got {} events", out.len());
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn frames_arrive_complete_and_in_order_despite_loss() {
    let lossy = Config {
        drop_percent: 3.0,
        ..Config::default()
    };
    let (mut laptop, phone) = sessions(lossy, Config::default()).await;

    // Phone → laptop: 120 frames at ~60 fps, a keyframe every 30.
    const FRAMES: usize = 120;
    let sender = tokio::spawn(async move {
        for i in 0..FRAMES {
            phone
                .send(OutFrame {
                    stream: 0,
                    data: frame_data(i),
                    key: i % 30 == 0,
                    intra_refresh: false,
                    capture_us: phone.now_us(),
                })
                .unwrap();
            tokio::time::sleep(Duration::from_millis(16)).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        phone
    });

    let mut got = Vec::new();
    collect(&mut laptop, 20, |e| {
        if let Event::Frame(f) = e {
            got.push(f.clone());
        }
        got.last().is_some_and(|f| f.id as usize == FRAMES - 1)
    })
    .await;
    let phone = sender.await.unwrap();

    let stats = laptop.stats();
    println!("{stats:#?}");
    assert!(stats.dropped_on_purpose > 20, "loss was simulated");
    // Everything recovered: every frame, in order, intact.
    assert_eq!(got.len(), FRAMES, "lost frames: {:?}", stats.received);
    for (i, f) in got.iter().enumerate() {
        assert_eq!(f.id as usize, i);
        assert_eq!(f.data, frame_data(i));
    }
    assert!(stats.received.frames_recovered > 0);
    assert_eq!(stats.received.given_up, 0);
    assert!(stats.path.is_some());
    assert!(stats.clock_offset_us.is_some());
    // Both run on this machine: the clocks agree to within a millisecond or so.
    let last = got.last().unwrap();
    let captured = laptop.to_local_us(last.capture_us).unwrap();
    assert!(last.received_us.abs_diff(captured) < 100_000);
    assert_eq!(phone.stats().sent.frames, FRAMES as u64);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_one_side_ends_the_other() {
    let (mut laptop, phone) = sessions(Config::default(), Config::default()).await;
    // Wait until a path is up, then hang up.
    collect(&mut laptop, 5, |e| matches!(e, Event::PathChanged(Some(_)))).await;
    drop(phone);
    collect(&mut laptop, 5, |e| *e == Event::Closed).await;
    assert!(laptop.send(OutFrame {
        stream: 0,
        data: vec![1],
        key: true,
        intra_refresh: false,
        capture_us: 0,
    })
    .is_err() || laptop.next_event().await.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_decoder_gets_a_keyframe() {
    let (mut laptop, mut phone) = sessions(Config::default(), Config::default()).await;
    collect(&mut laptop, 5, |e| matches!(e, Event::PathChanged(Some(_)))).await;
    laptop.need_keyframe(0);
    // The phone hasn't sent on stream 0 yet, but a decoder that failed asks
    // as soon as frames flow.
    phone
        .send(OutFrame {
            stream: 0,
            data: vec![0; 100],
            key: false,
            intra_refresh: false,
            capture_us: 0,
        })
        .unwrap();
    collect(&mut phone, 5, |e| *e == Event::KeyframeRequested { stream: 0 }).await;
}

/// The TLS connection breaks mid-session (its network went away: a cable
/// pulled out, the PC moved to the phone's hotspot). Frames keep flowing over
/// UDP, and hanging up still reaches the other side.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_broken_setup_connection_doesnt_end_the_session() {
    let (laptop_tls, phone_tls) = paired_tls();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // A relay between the two, killed later with a reset.
    let relay = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_addr = relay.local_addr().unwrap();
    let (kill, killed) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let (a, _) = relay.accept().await.unwrap();
        let b = TcpStream::connect(addr).await.unwrap();
        let (mut a2, mut b2) = (a, b);
        tokio::select! {
            _ = tokio::io::copy_bidirectional(&mut a2, &mut b2) => {}
            _ = killed => {
                // Reset rather than close: like a network that vanished.
                let _ = socket2::SockRef::from(&a2).set_linger(Some(Duration::ZERO));
                let _ = socket2::SockRef::from(&b2).set_linger(Some(Duration::ZERO));
            }
        }
    });
    let accepting = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let (stream, _) = phone_tls.accept(tcp).await.unwrap();
        Session::accept(stream, Config::default()).await.unwrap()
    });
    let (stream, _) = laptop_tls.connect(TcpStream::connect(relay_addr).await.unwrap()).await.unwrap();
    let mut laptop = Session::open(stream, Config::default()).await.unwrap();
    let phone = accepting.await.unwrap();
    collect(&mut laptop, 5, |e| matches!(e, Event::PathChanged(Some(_)))).await;

    kill.send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let frame = |i: u8| OutFrame {
        stream: 0,
        data: vec![i; 3_000],
        key: i == 0,
        intra_refresh: false,
        capture_us: 0,
    };
    for i in 0..3 {
        phone.send(frame(i)).unwrap();
    }
    let mut got = 0;
    collect(&mut laptop, 5, |e| {
        got += matches!(e, Event::Frame(_)) as u32;
        got == 3
    })
    .await;

    // Hanging up still gets through (as a bye over UDP).
    drop(phone);
    collect(&mut laptop, 5, |e| *e == Event::Closed).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn input_messages_arrive_in_order_on_their_own_stream_despite_loss() {
    use nudge_core::input::{self, Input, TouchAction};
    let lossy = Config {
        drop_percent: 10.0,
        ..Config::default()
    };
    // The laptop controls; the phone (sharing) loses a tenth of what comes in.
    let (mut laptop, phone) = sessions(Config::default(), lossy).await;
    let mut inputs = phone.subscribe(input::STREAM);
    collect(&mut laptop, 5, |e| matches!(e, Event::PathChanged(Some(_)))).await;

    // A drag: down, 60 moves ~8 ms apart, up.
    let mut sent = vec![Input::Touch { action: TouchAction::Down, pointer: 0, x: 100, y: 100 }];
    sent.extend((1..=60).map(|i| Input::Touch { action: TouchAction::Move, pointer: 0, x: 100 + i * 50, y: 100 }));
    sent.push(Input::Touch { action: TouchAction::Up, pointer: 0, x: 3100, y: 100 });
    sent.push(Input::Text("hi".into()));
    for m in &sent {
        laptop.send_message(input::STREAM, m.encode()).unwrap();
        tokio::time::sleep(Duration::from_millis(8)).await;
    }

    let mut got = Vec::new();
    let r = tokio::time::timeout(Duration::from_secs(5), async {
        while got.len() < sent.len() {
            let f = inputs.recv().await.expect("session ended");
            got.push(Input::parse(&f.data).expect("valid input"));
        }
    })
    .await;
    assert!(r.is_ok(), "got {} of {}: {:?}", got.len(), sent.len(), phone.stats().received);
    assert_eq!(got, sent, "every message, in order");
    assert!(phone.stats().dropped_on_purpose > 0, "loss was simulated");
}

/// Input delay, send to arrival, at several loss rates (`cargo test --test
/// realtime input_latency -- --ignored --nocapture`). On one machine, so
/// it's the channel's own delay: loss recovery and waiting, not the network.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn input_latency() {
    use nudge_core::input::{self, Input, TouchAction};
    // As in the app during a session (app/rust/src/timer.rs): 1 ms timers on Windows.
    #[cfg(windows)]
    #[allow(unsafe_code)]
    {
        #[link(name = "winmm")]
        unsafe extern "system" {
            fn timeBeginPeriod(period: u32) -> u32;
        }
        // SAFETY: no pointers; ends with the test process.
        unsafe { timeBeginPeriod(1) };
    }
    let losses: Vec<f64> = std::env::var("LOSS").ok().map(|l| vec![l.parse().unwrap()]).unwrap_or(vec![0.0, 2.0, 5.0, 10.0]);
    for loss in losses {
        let lossy = Config { drop_percent: loss, ..Config::default() };
        let (mut laptop, phone) = sessions(Config::default(), lossy).await;
        let mut inputs = phone.subscribe(input::STREAM);
        collect(&mut laptop, 5, |e| matches!(e, Event::PathChanged(Some(_)))).await;
        // Let the clocks be compared.
        tokio::time::sleep(Duration::from_millis(1500)).await;

        // Clicks: a down and an up 60 ms apart, every 150 ms, so a lost
        // message is often the last one for a while (as a lost "up" is).
        const CLICKS: usize = 150;
        let sender = tokio::spawn(async move {
            for i in 0..CLICKS {
                for action in [TouchAction::Down, TouchAction::Up] {
                    let m = Input::Touch { action, pointer: 0, x: i as u16, y: 0 };
                    laptop.send_message(input::STREAM, m.encode()).unwrap();
                    tokio::time::sleep(Duration::from_millis(if action == TouchAction::Down { 60 } else { 90 })).await;
                }
            }
            laptop
        });
        let mut delays = Vec::new();
        while delays.len() < CLICKS * 2 {
            let Ok(Some(f)) = tokio::time::timeout(Duration::from_secs(3), inputs.recv()).await else { break };
            let sent = phone.to_local_us(f.capture_us).unwrap();
            delays.push(phone.now_us().saturating_sub(sent));
        }
        let _laptop = sender.await.unwrap();
        delays.sort_unstable();
        let buckets = [2_000u64, 4_000, 9_000, 14_000, u64::MAX];
        let counts: Vec<usize> = buckets.iter().scan(0u64, |lo, &hi| { let n = delays.iter().filter(|&&d| d >= *lo && d < hi).count(); *lo = hi; Some(n) }).collect();
        println!("  under 2 / 4 / 9 / 14 / more ms: {counts:?}");
        let p = |q: f64| delays[((delays.len() - 1) as f64 * q).round() as usize] as f64 / 1000.0;
        println!(
            "loss {loss:>4}%: {} of {} arrived; delay p50 {:.2} ms, p95 {:.2}, p99 {:.2}, max {:.2}",
            delays.len(),
            CLICKS * 2,
            p(0.5),
            p(0.95),
            p(0.99),
            p(1.0)
        );
    }
}
