//! Interrupted transfers pick up where they left off.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use nudge_core::protocol::{Offer, Response};
use nudge_core::transfer::{CHUNK_SIZE, modified_time};
use nudge_core::{Error, Fingerprint, ReceivedFile, Result, receive_file, send_file};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

const CHUNK: usize = CHUNK_SIZE as usize;

fn test_data(size: usize, seed: u8) -> Vec<u8> {
    (0..size)
        .map(|i| (i.wrapping_mul(31) ^ (i >> 9)) as u8 ^ seed)
        .collect()
}

fn peer(n: u8) -> Fingerprint {
    format!("{n:02x}").repeat(32).parse().unwrap()
}

/// Start a receiver for one connection, saving into `dir` as if `sender` connected.
async fn receiver(
    dir: &Path,
    sender: Fingerprint,
) -> (std::net::SocketAddr, JoinHandle<Result<ReceivedFile>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = dir.to_path_buf();
    let handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        receive_file(&mut stream, &dir, Some(sender)).await
    });
    (addr, handle)
}

/// Send `src` but hang up after `cut_at` bytes of the file (counted from the
/// start of the file), optionally corrupting one chunk. Returns the offset
/// the receiver asked to start from.
async fn interrupted_send(
    src: &Path,
    dir: &Path,
    sender: Fingerprint,
    cut_at: usize,
    corrupt_chunk: Option<usize>,
) -> (u64, Error) {
    let data = std::fs::read(src).unwrap();
    let (addr, handle) = receiver(dir, sender).await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    Offer {
        name: src.file_name().unwrap().to_str().unwrap().into(),
        size: data.len() as u64,
        modified: modified_time(&std::fs::metadata(src).unwrap()),
        batch: nudge_core::protocol::Batch::single(data.len() as u64),
    }
    .write_to(&mut stream)
    .await
    .unwrap();
    let Response::Accept { offset } = Response::read_from(&mut stream).await.unwrap() else {
        panic!("not accepted");
    };

    let mut pos = offset as usize;
    'send: while pos < cut_at {
        let index = pos / CHUNK;
        let end = ((index + 1) * CHUNK).min(data.len());
        let mut chunk = data[pos..end].to_vec();
        let hash = *blake3::hash(&chunk).as_bytes();
        if corrupt_chunk == Some(index) {
            chunk[0] ^= 1;
        }
        let take = chunk.len().min(cut_at - pos);
        if stream.write_all(&chunk[..take]).await.is_err() {
            break 'send;
        }
        pos += take;
        if take == chunk.len() && stream.write_all(&hash).await.is_err() {
            break 'send;
        }
    }
    drop(stream);
    let err = handle.await.unwrap().unwrap_err();
    (offset, err)
}

/// Send `src` normally and check the result.
async fn full_send(src: &Path, dir: &Path, sender: Fingerprint) -> (u64, ReceivedFile) {
    let (addr, handle) = receiver(dir, sender).await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let sent = send_file(&mut stream, src).await.unwrap();
    let received = handle.await.unwrap().unwrap();
    assert_eq!(sent.resumed_from, received.stats.resumed_from);
    assert_eq!(sent.bytes, received.stats.bytes);
    assert!(
        std::fs::read(&received.path).unwrap() == std::fs::read(src).unwrap(),
        "content differs"
    );
    (sent.resumed_from, received)
}

fn dir_listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn setup(size: usize) -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("video.mp4");
    std::fs::write(&src, test_data(size, 0)).unwrap();
    (src_dir, dst_dir, src)
}

#[tokio::test]
async fn resumes_after_last_whole_chunk() {
    let (_s, dst, src) = setup(5 * CHUNK + 100);
    // Cut 1000 bytes into the third chunk: two whole chunks are safe.
    let (offset, err) = interrupted_send(&src, dst.path(), peer(1), 2 * CHUNK + 1000, None).await;
    assert_eq!(offset, 0);
    assert!(matches!(err, Error::Truncated { .. }), "got {err:?}");
    assert_eq!(
        dir_listing(dst.path()),
        ["video.mp4.nudgemeta", "video.mp4.nudgepart"]
    );

    let (resumed_from, received) = full_send(&src, dst.path(), peer(1)).await;
    assert_eq!(resumed_from, 2 * CHUNK_SIZE);
    assert_eq!(received.stats.bytes, (3 * CHUNK + 100) as u64);
    assert_eq!(dir_listing(dst.path()), ["video.mp4"]);
}

#[tokio::test]
async fn resumes_more_than_once() {
    let (_s, dst, src) = setup(4 * CHUNK);
    let (offset, _) = interrupted_send(&src, dst.path(), peer(1), CHUNK, None).await;
    assert_eq!(offset, 0);
    let (offset, _) = interrupted_send(&src, dst.path(), peer(1), 3 * CHUNK - 5, None).await;
    assert_eq!(offset, CHUNK_SIZE);
    let (resumed_from, _) = full_send(&src, dst.path(), peer(1)).await;
    assert_eq!(resumed_from, 2 * CHUNK_SIZE);
    assert_eq!(dir_listing(dst.path()), ["video.mp4"]);
}

#[tokio::test]
async fn corrupted_chunk_is_resent() {
    let (_s, dst, src) = setup(4 * CHUNK);
    let (_, err) = interrupted_send(&src, dst.path(), peer(1), 4 * CHUNK, Some(2)).await;
    assert!(
        matches!(err, Error::ChunkMismatch { chunk: 2 }),
        "got {err:?}"
    );
    let (resumed_from, _) = full_send(&src, dst.path(), peer(1)).await;
    assert_eq!(resumed_from, 2 * CHUNK_SIZE);
}

#[tokio::test]
async fn changed_file_starts_over() {
    let (_s, dst, src) = setup(3 * CHUNK);
    interrupted_send(&src, dst.path(), peer(1), 2 * CHUNK, None).await;

    // Same name and size, new content and modified time.
    std::fs::write(&src, test_data(3 * CHUNK, 0x55)).unwrap();
    let later = SystemTime::now() + Duration::from_secs(60);
    std::fs::File::options()
        .write(true)
        .open(&src)
        .unwrap()
        .set_modified(later)
        .unwrap();

    let (resumed_from, _) = full_send(&src, dst.path(), peer(1)).await;
    assert_eq!(resumed_from, 0);
    // The stale partial was cleaned up, not left beside the new file.
    assert_eq!(dir_listing(dst.path()), ["video.mp4"]);
}

#[tokio::test]
async fn other_senders_partial_is_not_used() {
    let (_s, dst, src) = setup(3 * CHUNK);
    interrupted_send(&src, dst.path(), peer(1), 2 * CHUNK, None).await;

    let (resumed_from, received) = full_send(&src, dst.path(), peer(2)).await;
    assert_eq!(resumed_from, 0);
    // Device 1's partial keeps its name; device 2's file takes the next one.
    assert_eq!(received.path, dst.path().join("video (1).mp4"));
    assert_eq!(
        dir_listing(dst.path()),
        [
            "video (1).mp4",
            "video.mp4.nudgemeta",
            "video.mp4.nudgepart"
        ]
    );

    // Device 1 can still finish its own transfer.
    let (resumed_from, received) = full_send(&src, dst.path(), peer(1)).await;
    assert_eq!(resumed_from, 2 * CHUNK_SIZE);
    assert_eq!(received.path, dst.path().join("video.mp4"));
}

#[tokio::test]
async fn finished_file_with_same_name_is_not_overwritten_on_resume() {
    let (_s, dst, src) = setup(2 * CHUNK);
    interrupted_send(&src, dst.path(), peer(1), CHUNK, None).await;
    // Someone saves a different file under the name we were going to use.
    std::fs::write(dst.path().join("video.mp4"), b"keep me").unwrap();

    let (resumed_from, received) = full_send(&src, dst.path(), peer(1)).await;
    assert_eq!(resumed_from, CHUNK_SIZE);
    assert_eq!(received.path, dst.path().join("video (1).mp4"));
    assert_eq!(
        std::fs::read(dst.path().join("video.mp4")).unwrap(),
        b"keep me"
    );
}
