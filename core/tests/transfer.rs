//! End-to-end transfers between a sender and receiver over loopback TCP.

use std::path::Path;

use nudge_core::protocol::{Offer, Response};
use nudge_core::transfer::{BUF_SIZE, CHUNK_SIZE};
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
        receive_file(&mut stream, &dst, None).await.unwrap()
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
async fn exactly_one_chunk() {
    round_trip(CHUNK_SIZE as usize).await;
}

#[tokio::test]
async fn several_chunks_plus_remainder() {
    round_trip(2 * CHUNK_SIZE as usize + 5).await;
}

/// Plays the sender by hand so the stream can be tampered with.
/// `tamper(chunk_index, data, hash)` may modify each chunk before it's sent.
async fn raw_send(
    data: &[u8],
    tamper: impl Fn(usize, &mut Vec<u8>, &mut [u8; 32]),
) -> nudge_core::Result<ReceivedFile> {
    let dst_dir = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dst = dst_dir.path().to_path_buf();
    let receiver = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        receive_file(&mut stream, &dst, None).await
    });

    let mut stream = TcpStream::connect(addr).await.unwrap();
    Offer {
        name: "tampered.bin".into(),
        size: data.len() as u64,
        modified: 0,
        batch: nudge_core::protocol::Batch::single(data.len() as u64),
    }
    .write_to(&mut stream)
    .await
    .unwrap();
    assert_eq!(
        Response::read_from(&mut stream).await.unwrap(),
        Response::Accept { offset: 0 }
    );
    for (i, chunk) in data.chunks(CHUNK_SIZE as usize).enumerate() {
        let mut hash = *blake3::hash(chunk).as_bytes();
        let mut chunk = chunk.to_vec();
        tamper(i, &mut chunk, &mut hash);
        // The receiver may hang up as soon as it spots a bad chunk.
        if stream.write_all(&chunk).await.is_err() || stream.write_all(&hash).await.is_err() {
            break;
        }
    }

    let result = receiver.await.unwrap();
    if result.is_err() {
        assert!(
            !dst_dir.path().join("tampered.bin").exists(),
            "corrupt file must not be saved under its final name"
        );
    }
    result
}

#[tokio::test]
async fn honest_raw_sender_is_accepted() {
    let data = test_data(CHUNK_SIZE as usize + 100);
    raw_send(&data, |_, _, _| {}).await.unwrap();
}

#[tokio::test]
async fn corrupted_data_is_caught() {
    let data = test_data(2 * CHUNK_SIZE as usize + 100);
    let err = raw_send(&data, |i, chunk, _| {
        if i == 1 {
            chunk[1234] ^= 0x01; // flip one bit in the second chunk
        }
    })
    .await
    .unwrap_err();
    assert!(
        matches!(err, Error::ChunkMismatch { chunk: 1 }),
        "got {err:?}"
    );
}

#[tokio::test]
async fn wrong_hash_is_caught() {
    let data = test_data(1000);
    let err = raw_send(&data, |_, _, hash| hash[0] ^= 0xff)
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::ChunkMismatch { chunk: 0 }),
        "got {err:?}"
    );
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
        receive_file(&mut stream, &dst, None).await
    });

    // A sender that promises 10 bytes, sends 4, then disconnects.
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let offer = Offer {
        name: "cut.bin".into(),
        size: 10,
        modified: 0,
        batch: nudge_core::protocol::Batch::single(10),
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
