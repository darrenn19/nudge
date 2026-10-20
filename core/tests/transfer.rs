//! End-to-end transfers between a sender and receiver over loopback TCP.

use std::path::Path;

use nudge_core::protocol::{Offer, Response};
use nudge_core::transfer::BUF_SIZE;
use nudge_core::{Error, ReceivedFile, receive_file, send_file};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

/// Deterministic test data that doesn't repeat on buffer boundaries.
fn test_data(size: usize) -> Vec<u8> {
    (0..size)
        .map(|i| (i.wrapping_mul(31) ^ (i >> 9)) as u8)
        .collect()
}

async fn transfer(src: &Path, dst_dir: &Path) -> ReceivedFile {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dst = dst_dir.to_path_buf();
    let receiver = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        receive_file(&mut stream, &dst).await.unwrap()
    });
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let sent = send_file(&mut stream, src).await.unwrap();
    let received = receiver.await.unwrap();
    assert_eq!(sent.bytes, received.stats.bytes);
    received
}

async fn round_trip(size: usize) {
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let data = test_data(size);
    let src = src_dir.path().join("test file.bin");
    std::fs::write(&src, &data).unwrap();

    let received = transfer(&src, dst_dir.path()).await;

    assert_eq!(received.stats.bytes, size as u64);
    assert_eq!(received.path, dst_dir.path().join("test file.bin"));
    assert!(
        std::fs::read(&received.path).unwrap() == data,
        "content differs"
    );
    let entries = std::fs::read_dir(dst_dir.path()).unwrap().count();
    assert_eq!(entries, 1, "partial file left behind");
}

#[tokio::test]
async fn empty_file() {
    round_trip(0).await;
}

#[tokio::test]
async fn tiny_file() {
    round_trip(1).await;
}

#[tokio::test]
async fn exactly_one_buffer() {
    round_trip(BUF_SIZE).await;
}

#[tokio::test]
async fn several_buffers_plus_remainder() {
    round_trip(3 * BUF_SIZE + 12_345).await;
}

#[tokio::test]
async fn same_name_twice_keeps_both() {
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("photo.jpg");
    std::fs::write(&src, b"first").unwrap();
    let first = transfer(&src, dst_dir.path()).await;
    std::fs::write(&src, b"second").unwrap();
    let second = transfer(&src, dst_dir.path()).await;

    assert_eq!(first.path, dst_dir.path().join("photo.jpg"));
    assert_eq!(second.path, dst_dir.path().join("photo (1).jpg"));
    assert_eq!(std::fs::read(&first.path).unwrap(), b"first");
    assert_eq!(std::fs::read(&second.path).unwrap(), b"second");
}

#[tokio::test]
async fn dropped_connection_keeps_partial_file_only() {
    let dst_dir = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dst = dst_dir.path().to_path_buf();
    let receiver = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        receive_file(&mut stream, &dst).await
    });

    // A sender that promises 10 bytes, sends 4, then disconnects.
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let offer = Offer {
        name: "cut.bin".into(),
        size: 10,
    };
    offer.write_to(&mut stream).await.unwrap();
    assert_eq!(
        Response::read_from(&mut stream).await.unwrap(),
        Response::Accept { offset: 0 }
    );
    stream.write_all(b"abcd").await.unwrap();
    drop(stream);

    let err = receiver.await.unwrap().unwrap_err();
    assert!(
        matches!(err, Error::Truncated { expected: 10, .. }),
        "got {err:?}"
    );
    assert!(!dst_dir.path().join("cut.bin").exists());
    assert!(dst_dir.path().join("cut.bin.nudgepart").exists());
}
