//! Two nodes on this machine: discover, pair by code, send a file.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use nudge_core::discovery::DiscoveryConfig;
use nudge_core::{Direction, Error, Node, NodeConfig, NodeEvent, Outgoing};
use tokio::sync::mpsc::UnboundedReceiver;

struct TestNode {
    node: Node,
    events: UnboundedReceiver<NodeEvent>,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
    downloads: std::path::PathBuf,
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

/// Loopback only (no firewall prompts), beacons only (mDNS on CI is flaky).
async fn start(name: &str, udp_port: u16, other_udp: u16) -> TestNode {
    start_with(name, udp_port, other_udp, |_| {}).await
}

async fn start_with(
    name: &str,
    udp_port: u16,
    other_udp: u16,
    tweak: impl FnOnce(&mut NodeConfig),
) -> TestNode {
    let data = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let mut config = NodeConfig::new(
        data.path().to_path_buf(),
        downloads.path().join("Nudge"),
        name,
        "windows",
    );
    config.port = Some(0);
    config.bind = Ipv4Addr::LOCALHOST.into();
    config.discovery = DiscoveryConfig {
        mdns: false,
        broadcast: true,
        udp_port,
        extra_targets: vec![SocketAddr::from((Ipv4Addr::LOCALHOST, other_udp))],
    };
    tweak(&mut config);
    let (node, events) = Node::start(config).await.unwrap();
    assert!(
        node.discovery_running(),
        "couldn't bind UDP port {udp_port}"
    );
    TestNode {
        node,
        events,
        downloads: downloads.path().join("Nudge"),
        _dirs: (data, downloads),
    }
}

async fn wait_for<T>(
    events: &mut UnboundedReceiver<NodeEvent>,
    mut pick: impl FnMut(&NodeEvent) -> Option<T>,
) -> T {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let event = events.recv().await.expect("node stopped");
            if let Some(t) = pick(&event) {
                return t;
            }
        }
    })
    .await
    .expect("timed out waiting for event")
}

async fn wait_until_seen(a: &mut TestNode, b: &TestNode) {
    let target = b.node.fingerprint();
    while !a.node.peers().iter().any(|p| p.fingerprint == target) {
        wait_for(&mut a.events, |e| {
            matches!(e, NodeEvent::PeersChanged).then_some(())
        })
        .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discover_pair_and_send() {
    let (udp_a, udp_b) = (free_udp_port(), free_udp_port());
    let mut laptop = start("Laptop", udp_a, udp_b).await;
    let mut phone = start("Phone", udp_b, udp_a).await;

    // They see each other, unpaired.
    wait_until_seen(&mut laptop, &phone).await;
    wait_until_seen(&mut phone, &laptop).await;
    let seen = laptop.node.peers();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].device_name, "Phone");
    assert!(!seen[0].paired && seen[0].online);

    // Sending before pairing fails cleanly.
    let src_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("photo.jpg");
    let data: Vec<u8> = (0..9_000_000u32).map(|i| (i % 253) as u8).collect();
    std::fs::write(&src, &data).unwrap();
    phone
        .node
        .send_files(laptop.node.fingerprint(), vec![src.clone()]);
    let error = wait_for(&mut phone.events, |e| match e {
        NodeEvent::TransferFailed { error, .. } => Some(error.clone()),
        _ => None,
    })
    .await;
    assert!(error.contains("not trusted"), "{error}");

    // Laptop shows a code; phone scans it.
    let code = laptop.node.start_pairing();
    let paired = phone.node.pair(&code).await.unwrap();
    assert_eq!(paired.fingerprint, laptop.node.fingerprint());
    assert_eq!(paired.device_name, "Laptop");
    assert!(paired.paired);
    let peer = wait_for(&mut laptop.events, |e| match e {
        NodeEvent::Paired { peer } => Some(peer.clone()),
        _ => None,
    })
    .await;
    assert_eq!(peer.fingerprint, phone.node.fingerprint());
    assert_eq!(peer.device_name, "Phone");

    // The code is single-use.
    let err = phone.node.pair(&code).await.unwrap_err();
    assert!(
        matches!(
            err,
            Error::PairingRejected | Error::NotTrustedByPeer | Error::Unreachable
        ),
        "{err:?}"
    );

    // Now files flow without any prompt.
    let ids = phone
        .node
        .send_files(laptop.node.fingerprint(), vec![src.clone()]);
    assert_eq!(ids.len(), 1);
    let (dir, size) = wait_for(&mut laptop.events, |e| match e {
        NodeEvent::TransferStarted {
            direction, size, ..
        } => Some((*direction, *size)),
        _ => None,
    })
    .await;
    assert_eq!(dir, Direction::Incoming);
    assert_eq!(size, data.len() as u64);
    let path = wait_for(&mut laptop.events, |e| match e {
        NodeEvent::TransferDone { path, .. } => Some(path.clone()),
        NodeEvent::TransferFailed { error, .. } => panic!("receive failed: {error}"),
        _ => None,
    })
    .await;
    assert_eq!(path, laptop.downloads.join("photo.jpg"));
    assert!(std::fs::read(&path).unwrap() == data);
    wait_for(&mut phone.events, |e| match e {
        NodeEvent::TransferDone { id, .. } if *id == ids[0] => Some(()),
        NodeEvent::TransferFailed { error, .. } => panic!("send failed: {error}"),
        _ => None,
    })
    .await;

    // An already-open file goes out under the name it's given (how Android
    // shares work: an open descriptor, no path).
    let open = std::fs::File::open(&src).unwrap();
    let ids = phone.node.send(
        laptop.node.fingerprint(),
        vec![Outgoing::Open {
            file: open,
            name: "shared.jpg".into(),
        }],
    );
    let path = wait_for(&mut laptop.events, |e| match e {
        NodeEvent::TransferDone { path, .. } => Some(path.clone()),
        NodeEvent::TransferFailed { error, .. } => panic!("receive failed: {error}"),
        _ => None,
    })
    .await;
    assert_eq!(path, laptop.downloads.join("shared.jpg"));
    assert!(std::fs::read(&path).unwrap() == data);
    wait_for(&mut phone.events, |e| match e {
        NodeEvent::TransferDone { id, .. } if *id == ids[0] => Some(()),
        NodeEvent::TransferFailed { error, .. } => panic!("send failed: {error}"),
        _ => None,
    })
    .await;

    // Unpairing on one side stops transfers again.
    laptop.node.unpair(&phone.node.fingerprint()).unwrap();
    assert!(!laptop.node.peers()[0].paired);
    phone.node.send_files(laptop.node.fingerprint(), vec![src]);
    let error = wait_for(&mut phone.events, |e| match e {
        NodeEvent::TransferFailed { error, .. } => Some(error.clone()),
        NodeEvent::TransferDone { .. } => panic!("unpaired device accepted a file"),
        _ => None,
    })
    .await;
    assert!(!error.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_code_is_rejected() {
    let (udp_a, udp_b) = (free_udp_port(), free_udp_port());
    let laptop = start("Laptop", udp_a, udp_b).await;
    let mut phone = start("Phone", udp_b, udp_a).await;
    wait_until_seen(&mut phone, &laptop).await;

    let code = laptop.node.start_pairing();
    // Same device and address, different secret.
    let secret_start = code.find("&s=").unwrap() + 3;
    let mut forged = code.clone();
    forged.replace_range(secret_start..secret_start + 32, &"0".repeat(32));
    let err = phone.node.pair(&forged).await.unwrap_err();
    assert!(matches!(err, Error::PairingRejected), "{err:?}");
    assert!(phone.node.peers().iter().all(|p| !p.paired));

    // Garbage isn't a code at all.
    assert!(matches!(
        phone.node.pair("https://example.com").await,
        Err(Error::BadPairingCode(_))
    ));
}

/// Two nodes that see and trust each other.
async fn paired(laptop_idle: Option<Duration>) -> (TestNode, TestNode) {
    let (udp_a, udp_b) = (free_udp_port(), free_udp_port());
    let mut laptop = start_with("Laptop", udp_a, udp_b, |c| {
        if let Some(idle) = laptop_idle {
            c.idle_timeout = idle;
        }
    })
    .await;
    let mut phone = start("Phone", udp_b, udp_a).await;
    wait_until_seen(&mut phone, &laptop).await;
    wait_until_seen(&mut laptop, &phone).await;
    let code = laptop.node.start_pairing();
    phone.node.pair(&code).await.unwrap();
    (laptop, phone)
}

/// Waits for every transfer in `ids` to finish on the sender.
async fn wait_sent(events: &mut UnboundedReceiver<NodeEvent>, ids: &[u64]) {
    let mut left: Vec<u64> = ids.to_vec();
    while !left.is_empty() {
        let id = wait_for(events, |e| match e {
            NodeEvent::TransferDone { id, .. } => Some(*id),
            NodeEvent::TransferFailed { error, .. } => panic!("send failed: {error}"),
            _ => None,
        })
        .await;
        left.retain(|x| *x != id);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_over_warmed_connections() {
    let (laptop, mut phone) = paired(None).await;
    let src_dir = tempfile::tempdir().unwrap();
    let files: Vec<_> = (0..3)
        .map(|i| {
            let path = src_dir.path().join(format!("file{i}.bin"));
            std::fs::write(&path, vec![i as u8; 100_000 + i]).unwrap();
            path
        })
        .collect();

    // Warm before the user picks; warming twice is harmless.
    phone.node.warm(laptop.node.fingerprint());
    phone.node.warm(laptop.node.fingerprint());
    tokio::time::sleep(Duration::from_millis(300)).await;

    let ids = phone
        .node
        .send_files(laptop.node.fingerprint(), files.clone());
    wait_sent(&mut phone.events, &ids).await;
    for (i, src) in files.iter().enumerate() {
        let got = std::fs::read(laptop.downloads.join(format!("file{i}.bin"))).unwrap();
        assert_eq!(got, std::fs::read(src).unwrap());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_warmed_connection_is_retried() {
    // The laptop drops idle connections quickly, so the phone's warmed one
    // is dead by the time it's used.
    let (laptop, mut phone) = paired(Some(Duration::from_millis(200))).await;
    let src_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("late.bin");
    std::fs::write(&src, vec![7u8; 5_000_000]).unwrap();

    phone.node.warm(laptop.node.fingerprint());
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let ids = phone
        .node
        .send_files(laptop.node.fingerprint(), vec![src.clone()]);
    wait_sent(&mut phone.events, &ids).await;
    assert_eq!(
        std::fs::read(laptop.downloads.join("late.bin")).unwrap(),
        std::fs::read(&src).unwrap()
    );
}
