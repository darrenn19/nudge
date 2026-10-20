//! Single-file send and receive over any byte stream (plain TCP now, TLS next).
//!
//! Disk and network run in parallel: a blocking thread reads (or writes) the
//! file in large chunks while the async side moves chunks over the socket.
//! The data stream has no per-chunk acknowledgements.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::protocol::{Offer, Response};
use crate::{Error, Result};

/// Size of each chunk moved between disk and network. Tune with benchmarks.
pub const BUF_SIZE: usize = 1 << 20;

/// How many chunks may be in flight between the disk thread and the socket.
const PIPELINE_DEPTH: usize = 4;

/// Suffix for files still being received. Kept on failure so a later
/// transfer can resume from it.
pub const PARTIAL_SUFFIX: &str = ".nudgepart";

/// Timing for a finished transfer.
#[derive(Debug, Clone, Copy)]
pub struct TransferStats {
    /// Payload bytes moved over the connection (excludes protocol headers).
    pub bytes: u64,
    pub elapsed: Duration,
}

impl TransferStats {
    /// Throughput in megabytes (10^6 bytes) per second.
    pub fn mb_per_sec(&self) -> f64 {
        self.bytes as f64 / 1e6 / self.elapsed.as_secs_f64().max(1e-9)
    }
}

/// A file that has been fully received and saved.
#[derive(Debug, Clone)]
pub struct ReceivedFile {
    pub path: PathBuf,
    pub stats: TransferStats,
}

/// Send one file over `stream` and wait until the receiver confirms it is on disk.
pub async fn send_file<S>(stream: &mut S, path: &Path) -> Result<TransferStats>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| Error::BadFileName(path.display().to_string()))?
        .to_owned();
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    let start = Instant::now();

    Offer { name, size }.write_to(stream).await?;
    let offset = match Response::read_from(stream).await? {
        Response::Accept { offset } if offset <= size => offset,
        Response::Accept { .. } => {
            return Err(Error::Protocol("resume offset past end of file"));
        }
        Response::Reject => return Err(Error::Rejected),
        Response::Done => return Err(Error::Protocol("unexpected Done before data")),
    };

    let (tx, mut rx) = mpsc::channel(PIPELINE_DEPTH);
    let reader = tokio::task::spawn_blocking(move || read_chunks(file, offset, size, tx));

    let mut sent = 0u64;
    while let Some(chunk) = rx.recv().await {
        let chunk: Vec<u8> = chunk?;
        stream.write_all(&chunk).await?;
        sent += chunk.len() as u64;
    }
    stream.flush().await?;
    reader.await.map_err(io::Error::other)?;

    match Response::read_from(stream).await? {
        Response::Done => Ok(TransferStats {
            bytes: sent,
            elapsed: start.elapsed(),
        }),
        _ => Err(Error::Protocol("expected Done after data")),
    }
}

/// Receive one file from `stream` into `dir`. Never overwrites an existing
/// file: a clash gets a " (1)", " (2)", ... suffix.
pub async fn receive_file<S>(stream: &mut S, dir: &Path) -> Result<ReceivedFile>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let offer = Offer::read_from(stream).await?;
    let start = Instant::now();
    let name = sanitize_file_name(&offer.name)?;
    let final_path = unique_path(dir, &name);
    let part_path = partial_path(&final_path);

    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&part_path)?;
    // Reserve the full size up front so the OS doesn't grow the file piecemeal.
    file.set_len(offer.size)?;

    Response::Accept { offset: 0 }.write_to(stream).await?;

    let (tx, rx) = mpsc::channel(PIPELINE_DEPTH);
    let writer = tokio::task::spawn_blocking(move || write_chunks(file, rx));

    let mut received = 0u64;
    while received < offer.size {
        let want = (offer.size - received).min(BUF_SIZE as u64) as usize;
        let mut buf = vec![0u8; want];
        if let Err(e) = stream.read_exact(&mut buf).await {
            return Err(if e.kind() == io::ErrorKind::UnexpectedEof {
                Error::Truncated {
                    got: received,
                    expected: offer.size,
                }
            } else {
                e.into()
            });
        }
        received += want as u64;
        if tx.send(buf).await.is_err() {
            break; // writer hit an error; it's reported below
        }
    }
    drop(tx);
    writer.await.map_err(io::Error::other)??;

    fs::rename(&part_path, &final_path)?;
    Response::Done.write_to(stream).await?;

    Ok(ReceivedFile {
        path: final_path,
        stats: TransferStats {
            bytes: received,
            elapsed: start.elapsed(),
        },
    })
}

fn read_chunks(mut file: File, offset: u64, size: u64, tx: mpsc::Sender<io::Result<Vec<u8>>>) {
    if offset > 0 {
        if let Err(e) = file.seek(SeekFrom::Start(offset)) {
            let _ = tx.blocking_send(Err(e));
            return;
        }
    }
    let mut remaining = size - offset;
    while remaining > 0 {
        let want = remaining.min(BUF_SIZE as u64) as usize;
        let mut buf = vec![0u8; want];
        let result = file.read_exact(&mut buf).map(|()| buf);
        let failed = result.is_err();
        if tx.blocking_send(result).is_err() || failed {
            return;
        }
        remaining -= want as u64;
    }
}

fn write_chunks(mut file: File, mut rx: mpsc::Receiver<Vec<u8>>) -> io::Result<()> {
    while let Some(buf) = rx.blocking_recv() {
        file.write_all(&buf)?;
    }
    // Make sure the data is on disk before we tell the sender we're done.
    file.sync_all()
}

/// Make a sender-supplied name safe to use as a single file name on any OS.
/// Path separators and characters Windows forbids become `_`; names that
/// can't be made safe are rejected.
pub fn sanitize_file_name(raw: &str) -> Result<String> {
    const FORBIDDEN: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];

    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_control() || FORBIDDEN.contains(&c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    // Windows silently drops trailing dots and spaces, so do it ourselves.
    let cleaned = cleaned.trim_start().trim_end_matches(['.', ' ']);
    if cleaned.is_empty() {
        return Err(Error::BadFileName(raw.to_owned()));
    }
    let stem = cleaned.split('.').next().unwrap_or("").trim_end();
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        return Ok(format!("_{cleaned}"));
    }
    Ok(cleaned.to_owned())
}

/// `dir/name`, or `dir/stem (n).ext` if that (or its partial file) already exists.
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let taken = |p: &Path| p.exists() || partial_path(p).exists();
    let candidate = dir.join(name);
    if !taken(&candidate) {
        return candidate;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (1..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !taken(p))
        .expect("unbounded search always finds a free name")
}

fn partial_path(final_path: &Path) -> PathBuf {
    let mut s = final_path.as_os_str().to_owned();
    s.push(PARTIAL_SUFFIX);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_normal_names() {
        assert_eq!(sanitize_file_name("IMG_2041.jpg").unwrap(), "IMG_2041.jpg");
        assert_eq!(
            sanitize_file_name("café menu.pdf").unwrap(),
            "café menu.pdf"
        );
        assert_eq!(sanitize_file_name(".bashrc").unwrap(), ".bashrc");
    }

    #[test]
    fn sanitize_blocks_path_tricks() {
        assert_eq!(
            sanitize_file_name("../../etc/passwd").unwrap(),
            ".._.._etc_passwd"
        );
        assert_eq!(
            sanitize_file_name("C:\\Windows\\x.dll").unwrap(),
            "C__Windows_x.dll"
        );
        assert_eq!(sanitize_file_name("a\0b").unwrap(), "a_b");
        assert!(sanitize_file_name("..").is_err());
        assert!(sanitize_file_name(".").is_err());
        assert!(sanitize_file_name("").is_err());
        assert!(sanitize_file_name("   ").is_err());
    }

    #[test]
    fn sanitize_handles_windows_quirks() {
        assert_eq!(sanitize_file_name("notes. ").unwrap(), "notes");
        assert_eq!(sanitize_file_name("con.txt").unwrap(), "_con.txt");
        assert_eq!(sanitize_file_name("NUL").unwrap(), "_NUL");
        assert_eq!(sanitize_file_name("console.txt").unwrap(), "console.txt");
        assert_eq!(sanitize_file_name("what?.txt").unwrap(), "what_.txt");
    }

    #[test]
    fn unique_path_adds_counter() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(unique_path(dir.path(), "a.txt"), dir.path().join("a.txt"));
        fs::write(dir.path().join("a.txt"), b"x").unwrap();
        assert_eq!(
            unique_path(dir.path(), "a.txt"),
            dir.path().join("a (1).txt")
        );
        fs::write(dir.path().join("a (1).txt.nudgepart"), b"x").unwrap();
        assert_eq!(
            unique_path(dir.path(), "a.txt"),
            dir.path().join("a (2).txt")
        );
        fs::write(dir.path().join("README"), b"x").unwrap();
        assert_eq!(
            unique_path(dir.path(), "README"),
            dir.path().join("README (1)")
        );
    }
}
