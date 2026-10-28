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
