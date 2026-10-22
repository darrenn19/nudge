//! Encrypted transfers and certificate pinning.

use nudge_core::{Error, Identity, Tls, TrustedPeers, receive_file, send_file};
use tokio::net::{TcpListener, TcpStream};

struct Device {
    identity: Identity,
    trusted: TrustedPeers,
    tls: Tls,
}

fn device() -> Device {
    let identity = Identity::generate().unwrap();
    let trusted = TrustedPeers::default();
    let tls = Tls::new(&identity, trusted.clone()).unwrap();
    Device {
        identity,
        trusted,
        tls,
    }
}

fn pair(a: &Device, b: &Device) {
    a.trusted.add(b.identity.fingerprint());
    b.trusted.add(a.identity.fingerprint());
}

#[tokio::test]
async fn paired_devices_transfer_over_tls() {
    let (phone, laptop) = (device(), device());
    // Pairing after the TLS configs exist must take effect immediately.
    pair(&phone, &laptop);

    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("secret.txt");
    let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&src, &data).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let laptop_tls = laptop.tls.clone();
    let dst = dst_dir.path().to_path_buf();
    let receiver = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let (mut stream, peer) = laptop_tls.accept(tcp).await.unwrap();
        let file = receive_file(&mut stream, &dst, Some(peer)).await.unwrap();
        (file, peer)
    });

    let tcp = TcpStream::connect(addr).await.unwrap();
    let (mut stream, peer) = phone.tls.connect(tcp).await.unwrap();
    assert_eq!(peer, laptop.identity.fingerprint());
    send_file(&mut stream, &src).await.unwrap();

    let (file, peer) = receiver.await.unwrap();
    assert_eq!(peer, phone.identity.fingerprint());
    assert!(std::fs::read(file.path).unwrap() == data);
}

#[tokio::test]
async fn sender_refuses_unknown_receiver() {
    let (phone, stranger) = (device(), device());
    stranger.trusted.add(phone.identity.fingerprint()); // one-sided

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let stranger_tls = stranger.tls.clone();
    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let _ = stranger_tls.accept(tcp).await;
    });

    let tcp = TcpStream::connect(addr).await.unwrap();
    let err = phone.tls.connect(tcp).await.unwrap_err();
    assert!(
        matches!(err, Error::UntrustedPeer(fp) if fp == stranger.identity.fingerprint()),
        "got {err:?}"
    );
}

#[tokio::test]
async fn receiver_refuses_unknown_sender() {
    let (stranger, laptop) = (device(), device());
    stranger.trusted.add(laptop.identity.fingerprint()); // one-sided

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let laptop_tls = laptop.tls.clone();
    let receiver = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        laptop_tls.accept(tcp).await.map(|_| ())
    });

    let tcp = TcpStream::connect(addr).await.unwrap();
    // The sender may finish its half of the handshake before the receiver
    // rejects it, in which case the rejection shows up on the first read.
    let sender_err = match stranger.tls.connect(tcp).await {
        Err(e) => e,
        Ok((mut stream, _)) => {
            let src = tempfile::NamedTempFile::new().unwrap();
            send_file(&mut stream, src.path()).await.unwrap_err()
        }
    };
    assert!(
        matches!(sender_err, Error::NotTrustedByPeer),
        "got {sender_err:?}"
    );

    let err = receiver.await.unwrap().unwrap_err();
    assert!(
        matches!(err, Error::UntrustedPeer(fp) if fp == stranger.identity.fingerprint()),
        "got {err:?}"
    );
}

#[tokio::test]
async fn plain_tcp_client_is_rejected() {
    let laptop = device();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let laptop_tls = laptop.tls.clone();
    let receiver = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        laptop_tls.accept(tcp).await.map(|_| ())
    });

    // Speak the unencrypted protocol at a TLS receiver.
    let mut tcp = TcpStream::connect(addr).await.unwrap();
    let offer = nudge_core::protocol::Offer {
        name: "x".into(),
        size: 1,
        modified: 0,
        batch: nudge_core::protocol::Batch::single(1),
    };
    let _ = offer.write_to(&mut tcp).await;
    assert!(receiver.await.unwrap().is_err());
}
