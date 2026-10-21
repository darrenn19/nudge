//! Two nodes on this machine: discover, pair by code, send a file.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use nudge_core::discovery::DiscoveryConfig;
use nudge_core::{Direction, Error, Node, NodeConfig, NodeEvent};
use tokio::sync::mpsc::UnboundedReceiver;

struct TestNode {
    node: Node,
    events: UnboundedReceiver<NodeEvent>,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
    downloads: std::path::PathBuf,
}

fn free_udp_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Loopback only (no firewall prompts), beacons only (mDNS on CI is flaky).
async fn start(name: &str, udp_port: u16, other_udp: u16) -> TestNode {
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
    let (node, events) = Node::start(config).await.unwrap();
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
