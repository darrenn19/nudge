//! Single-file send and receive over any byte stream (TLS in practice).
//!
//! Disk and network run in parallel: a blocking thread reads (or writes) the
//! file while the async side moves data over the socket. The data stream has
//! no per-chunk acknowledgements.
//!
//! The file is split into fixed `CHUNK_SIZE` chunks. After each chunk the
//! sender sends its 32-byte BLAKE3 hash; the receiver checks it and aborts on
//! a mismatch. Hashing happens on the disk threads, off the network path.
//!
//! Interrupted transfers resume after the last verified chunk; see `resume`.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, UNIX_EPOCH};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::identity::Fingerprint;
use crate::pack;
use crate::protocol::{Batch, HASH_LEN, Offer, Response};
use crate::resume::{self, MetaWriter, TransferKey, with_suffix};
use crate::{Error, Result};

/// Bytes covered by each BLAKE3 chunk hash. Part of the wire format: resume
/// offsets are always a multiple of this. Must be a multiple of `BUF_SIZE`.
pub const CHUNK_SIZE: u64 = 4 << 20;

/// Size of each buffer moved between disk and network. Tune with benchmarks.
pub const BUF_SIZE: usize = 1 << 20;

const _: () = assert!(CHUNK_SIZE % BUF_SIZE as u64 == 0);

/// How many buffers may be in flight between the disk thread and the socket.
const PIPELINE_DEPTH: usize = 4;

/// Suffix for files still being received. Kept on failure so a later
/// transfer can resume from it.
pub const PARTIAL_SUFFIX: &str = ".nudgepart";

/// How many verified chunks the receiver collects before syncing the data
/// file and recording them as resumable (64 × 4 MiB = 256 MiB). A crash loses
/// at most this much progress; a dropped connection loses none.
const CHECKPOINT_CHUNKS: usize = 64;

/// Timing for a finished transfer.
#[derive(Debug, Clone, Copy)]
pub struct TransferStats {
    /// File bytes moved over this connection (excludes headers and hashes).
    pub bytes: u64,
    /// Offset this connection started from; non-zero means it resumed an
    /// earlier, interrupted transfer.
    pub resumed_from: u64,
    pub elapsed: Duration,
}

impl TransferStats {
    /// Throughput in megabytes (10^6 bytes) per second.
    pub fn mb_per_sec(&self) -> f64 {
        self.bytes as f64 / 1e6 / self.elapsed.as_secs_f64().max(1e-9)
    }
}

/// A file that has been fully received, verified and saved.
#[derive(Debug, Clone)]
pub struct ReceivedFile {
    pub path: PathBuf,
    pub stats: TransferStats,
}

/// Spare buffers handed back by whoever used them last, so each transfer
/// allocates (and zeroes) its few buffers once instead of one per megabyte.
#[derive(Clone, Default)]
struct Pool(Arc<std::sync::Mutex<Vec<Vec<u8>>>>);

impl Pool {
    /// A buffer of exactly `len` bytes. Reused contents are left as they
    /// are: every caller overwrites the whole buffer.
    fn get(&self, len: usize) -> Vec<u8> {
        let spare = self.0.lock().unwrap_or_else(|e| e.into_inner()).pop();
        let mut buf = spare.unwrap_or_default();
        buf.resize(len, 0);
        buf
    }

    fn put(&self, buf: Vec<u8>) {
        let mut spares = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if spares.len() <= PIPELINE_DEPTH {
            spares.push(buf);
        }
    }
}

/// What travels between a disk thread and the network loop.
enum Piece {
    Data(Vec<u8>),
    /// End of a chunk, with its BLAKE3 hash.
    ChunkEnd([u8; HASH_LEN]),
}

/// End offset of the chunk containing `pos`, capped at `size`.
fn chunk_end(pos: u64, size: u64) -> u64 {
    ((pos / CHUNK_SIZE + 1) * CHUNK_SIZE).min(size)
}

/// Where received bytes go: a file normally, or a discarding sink for benchmarks.
pub trait Sink: Write + Send + 'static {
    /// Make everything written so far durable.
    fn sync(&mut self) -> io::Result<()>;
}

impl Sink for File {
    fn sync(&mut self) -> io::Result<()> {
        self.sync_data()
    }
}

/// Who stopped a transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelledBy {
    /// The user on this device.
    Here,
    /// The user on the other device.
    There,
}

impl CancelledBy {
    /// How the transfer ends, on this side.
    pub fn error(self) -> Error {
        match self {
            Self::Here => Error::Cancelled,
            Self::There => Error::CancelledThere,
        }
    }
}

/// A way to stop a transfer (or a whole send) from outside it: set once,
/// seen by everything holding a clone.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<CancelInner>);

#[derive(Debug, Default)]
struct CancelInner {
    /// 0: not cancelled; 1: here; 2: there.
    by: std::sync::atomic::AtomicU8,
    note: tokio::sync::Notify,
}

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stop it. The first call decides who did.
    pub fn cancel(&self, by: CancelledBy) {
        let code = match by {
            CancelledBy::Here => 1,
            CancelledBy::There => 2,
        };
        let _ = self
            .0
            .by
            .compare_exchange(0, code, Ordering::AcqRel, Ordering::Acquire);
        self.0.note.notify_waiters();
    }

    pub fn by(&self) -> Option<CancelledBy> {
        match self.0.by.load(Ordering::Acquire) {
            1 => Some(CancelledBy::Here),
            2 => Some(CancelledBy::There),
            _ => None,
        }
    }

    /// Whether `other` is this same signal (not just a lookalike).
    pub fn same(&self, other: &Cancel) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// Finishes once it's cancelled.
    pub async fn until(&self) -> CancelledBy {
        loop {
            let note = self.0.note.notified();
            tokio::pin!(note);
            note.as_mut().enable();
            if let Some(by) = self.by() {
                return by;
            }
            note.await;
        }
    }
}

/// Live position of a transfer, readable from any thread while it runs.
/// Updating it is one relaxed atomic store per buffer, so it costs nothing.
#[derive(Clone, Debug, Default)]
pub struct Progress(Arc<ProgressInner>);

#[derive(Debug, Default)]
struct ProgressInner {
    position: AtomicU64,
    accepted: std::sync::atomic::AtomicBool,
    pending: std::sync::atomic::AtomicBool,
    held: std::sync::atomic::AtomicBool,
    /// Wakes `until_pending`.
    became_pending: tokio::sync::Notify,
    cancel: Cancel,
}

impl Progress {
    pub fn new() -> Self {
        Self::default()
    }

    /// One stopped by `cancel`, shared with others (files sent together
    /// that stop together).
    pub fn with_cancel(cancel: Cancel) -> Self {
        Self(Arc::new(ProgressInner {
            cancel,
            ..ProgressInner::default()
        }))
    }

    /// Bytes of the file that have crossed the network so far, counted from
    /// the start of the file (a resumed transfer starts at its offset).
    pub fn position(&self) -> u64 {
        self.0.position.load(Ordering::Relaxed)
    }

    pub(crate) fn set(&self, pos: u64) {
        self.0.position.store(pos, Ordering::Relaxed);
    }

    /// The receiver said yes (sending side).
    pub fn accepted(&self) -> bool {
        self.0.accepted.load(Ordering::Relaxed)
    }

    pub(crate) fn mark_accepted(&self) {
        self.0.accepted.store(true, Ordering::Relaxed);
    }

    /// The receiver is asking its user (sending side). An answer follows.
    pub fn pending(&self) -> bool {
        self.0.pending.load(Ordering::Relaxed)
    }

    pub(crate) fn mark_pending(&self) {
        self.0.pending.store(true, Ordering::Relaxed);
        self.0.became_pending.notify_one();
    }

    /// Finishes once the receiver says it's asking its user.
    pub async fn until_pending(&self) {
        while !self.pending() {
            self.0.became_pending.notified().await;
        }
    }

    /// Every byte is with the receiver, kept until its user answers
    /// (sending side).
    pub fn held(&self) -> bool {
        self.0.held.load(Ordering::Relaxed)
    }

    pub(crate) fn mark_held(&self) {
        self.0.held.store(true, Ordering::Relaxed);
    }

    /// Stops this transfer when cancelled.
    pub fn cancel(&self) -> &Cancel {
        &self.0.cancel
    }
}

/// Send one file over `stream` and wait until the receiver confirms it is on disk.
pub async fn send_file<S>(stream: &mut S, path: &Path) -> Result<TransferStats>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    send_file_tracked(stream, path, None, &Progress::new()).await
}

/// `send_file`, reporting its position through `progress`.
pub async fn send_file_tracked<S>(
    stream: &mut S,
    path: &Path,
    batch: Option<Batch>,
    progress: &Progress,
) -> Result<TransferStats>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| Error::BadFileName(path.display().to_string()))?
        .to_owned();
    send_open_file_tracked(stream, File::open(path)?, name, batch, progress).await
}

/// Send a file that is already open, under `name`. Android shares files as
/// open descriptors rather than paths, so this is how they go out.
pub async fn send_open_file_tracked<S>(
    stream: &mut S,
    file: File,
    name: String,
    batch: Option<Batch>,
    progress: &Progress,
) -> Result<TransferStats>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let metadata = file.metadata()?;
    // Resume needs to seek, and the offer needs a size up front.
    if !metadata.is_file() {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} isn't a regular file"),
        )));
    }
    let offer = Offer {
        name,
        size: metadata.len(),
        modified: modified_time(&metadata),
        batch: batch.unwrap_or(Batch::single(metadata.len())),
        pack: false,
    };
    send_from_tracked(stream, file, offer, progress).await
}

/// Send `offer.size` bytes read from `source` (a file, or generated data for
/// benchmarks) and wait until the receiver confirms it has them.
pub async fn send_from<S, R>(stream: &mut S, source: R, offer: Offer) -> Result<TransferStats>
where
    S: AsyncRead + AsyncWrite + Unpin,
    R: Read + Seek + Send + 'static,
{
    send_from_tracked(stream, source, offer, &Progress::new()).await
}

/// `send_from`, reporting its position through `progress`.
pub async fn send_from_tracked<S, R>(
    stream: &mut S,
    source: R,
    offer: Offer,
    progress: &Progress,
) -> Result<TransferStats>
where
    S: AsyncRead + AsyncWrite + Unpin,
    R: Read + Seek + Send + 'static,
{
    let start = Instant::now();
    let size = offer.size;
    offer.write_to(stream).await?;
    let offset = loop {
        match Response::read_from(stream).await? {
            // The receiver is asking its user. The bytes go anyway; the
            // answer comes later.
            Response::Asking => progress.mark_pending(),
            Response::Approved => progress.mark_accepted(),
            Response::Accept { offset } if offset <= size && offset % CHUNK_SIZE == 0 => {
                break offset;
            }
            Response::Accept { .. } => return Err(Error::Protocol("invalid resume offset")),
            Response::Reject => return Err(Error::Rejected),
            Response::Declined => return Err(Error::Declined),
            Response::NoAnswer => return Err(Error::NoAnswer),
            Response::NotPaired => return Err(Error::NotTrustedByPeer),
            Response::Cancelled => return Err(Error::CancelledThere),
            Response::Done | Response::Held => {
                return Err(Error::Protocol("unexpected answer before data"));
            }
        }
    };
    // Taken without asking: accepted from the start.
    if !progress.pending() {
        progress.mark_accepted();
    }

    let (tx, mut rx) = mpsc::channel(PIPELINE_DEPTH);
    let pool = Pool::default();
    let spares = pool.clone();
    let reader = tokio::task::spawn_blocking(move || read_pieces(source, offset, size, tx, spares));

    progress.set(offset);
    let mut sent = 0u64;
    while let Some(piece) = rx.recv().await {
        match piece? {
            Piece::Data(buf) => {
                stream.write_all(&buf).await?;
                let len = buf.len() as u64;
                pool.put(buf);
                sent += len;
                progress.set(offset + sent);
            }
            Piece::ChunkEnd(hash) => stream.write_all(&hash).await?,
        }
    }
    stream.flush().await?;
    reader.await.map_err(io::Error::other)?;

    let stats = TransferStats {
        bytes: sent,
        resumed_from: offset,
        elapsed: start.elapsed(),
    };
    loop {
        match Response::read_from(stream).await? {
            Response::Done => {
                progress.mark_accepted();
                return Ok(stats);
            }
            // Stored on the other side, waiting for its user's answer.
            Response::Held => {
                progress.mark_held();
                return Ok(stats);
            }
            Response::Approved => progress.mark_accepted(),
            Response::Declined => return Err(Error::Declined),
            Response::NoAnswer => return Err(Error::NoAnswer),
            Response::Cancelled => return Err(Error::CancelledThere),
            _ => return Err(Error::Protocol("expected Done after data")),
        }
    }
}

/// A file's modification time in nanoseconds since the Unix epoch, or 0 if
/// the platform can't tell. Lets the receiver tell versions of a file apart.
pub fn modified_time(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as u64)
}

/// Receive one file from `stream` into `dir`.
///
/// `peer` is the sender's fingerprint (from the TLS handshake). If an earlier
/// transfer of the same file from the same sender was interrupted, this
/// resumes after its last verified chunk. Never overwrites an existing file:
/// a clash gets a " (1)", " (2)", ... suffix.
pub async fn receive_file<S>(
    stream: &mut S,
    dir: &Path,
    peer: Option<Fingerprint>,
) -> Result<ReceivedFile>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let offer = Offer::read_from(stream).await?;
    receive_offered(stream, offer, dir, peer, &Progress::new()).await
}

/// Second half of `receive_file`, for callers that already read the `Offer`
/// (to show what's arriving before it does). Reports its position through
/// `progress`.
pub async fn receive_offered<S>(
    stream: &mut S,
    offer: Offer,
    dir: &Path,
    peer: Option<Fingerprint>,
    progress: &Progress,
) -> Result<ReceivedFile>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let landed = land(
        stream,
        offer,
        dir,
        peer,
        progress,
        std::future::pending::<Stop>(),
    )
    .await?
    .expect("never stopped");
    let file = landed.keep()?;
    Response::Done.write_to(stream).await?;
    Ok(file)
}

/// A file that has fully arrived, verified and synced to disk, but isn't
/// under its own name yet: still a partial file, so nothing shows it. The
/// receiver holds it like this while its user decides.
#[derive(Debug)]
pub struct Landed {
    partial: resume::Partial,
    dir: PathBuf,
    name: String,
    /// Where it goes under `dir`, for a file from a folder ("Trip/a.txt").
    parts: Vec<String>,
    /// A pack of files, unpacked when kept (see `pack`).
    pack: bool,
    pub stats: TransferStats,
}

impl Landed {
    /// Give it its real name (or unpack it): it's received. For a pack,
    /// the path is the folder that arrived, or the download folder.
    pub fn keep(self) -> Result<ReceivedFile> {
        if self.pack {
            let path = pack::unpack(&self.partial.part_path(), &self.dir);
            self.partial.remove();
            return Ok(ReceivedFile {
                path: path?,
                stats: self.stats,
            });
        }
        if self.parts.len() > 1 {
            let final_path = pack::place(&self.dir, &self.parts)?;
            fs::rename(self.partial.part_path(), &final_path)?;
            resume::finish(&self.partial);
            return Ok(ReceivedFile {
                path: final_path,
                stats: self.stats,
            });
        }
        // Pick the final name now: something may have taken it since we started.
        let final_path = if self.partial.base.exists() {
            unique_path(&self.dir, &self.name)
        } else {
            self.partial.base.clone()
        };
        fs::rename(self.partial.part_path(), &final_path)?;
        resume::finish(&self.partial);
        Ok(ReceivedFile {
            path: final_path,
            stats: self.stats,
        })
    }

    /// Delete it: the user didn't want it.
    pub fn discard(self) {
        self.partial.remove();
    }
}

/// Why [`land`] stopped before the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The user declined: delete what arrived.
    Discard,
    /// Another connection is taking this file over (the sender moved to a
    /// faster path before this one noticed): keep what arrived, saved, for
    /// that one to resume from.
    Keep,
}

/// Receive an offered file into a partial file: `Accept`, then the data.
/// Sends nothing after the data; the caller decides between `Done`, `Held`
/// and a refusal.
///
/// Stops early when `stop` finishes, deleting what had arrived or keeping
/// it as it says; returns `None` then.
pub async fn land<S, F>(
    stream: &mut S,
    offer: Offer,
    dir: &Path,
    peer: Option<Fingerprint>,
    progress: &Progress,
    stop: F,
) -> Result<Option<Landed>>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: Future<Output = Stop>,
{
    let start = Instant::now();
    let size = offer.size;
    // A file from a folder ("Trip/a.txt") lands under a flat name and moves
    // into its folder when kept. A pack's name is only for showing.
    let checked = if offer.pack {
        sanitize_file_name(&offer.name).map(|name| (name, Vec::new()))
    } else {
        pack::safe_relative(&offer.name).map(|parts| (parts.join("_"), parts))
    };
    let (name, parts) = match checked {
        Ok(checked) => checked,
        Err(e) => {
            let _ = Response::Reject.write_to(stream).await;
            return Err(e);
        }
    };
    let pack = offer.pack;
    let key = TransferKey {
        peer: peer.map_or([0; 32], |p| *p.as_bytes()),
        name: offer.name,
        size,
        modified: offer.modified,
    };

    let (partial, mut file, meta) = match resume::find(dir, &key)? {
        Some(partial) => {
            let (file, meta) = resume::reopen(&partial, &key)?;
            (partial, file, meta)
        }
        None => resume::create(dir, &key, &name)?,
    };
    let offset = (partial.chunks_done * CHUNK_SIZE).min(size);
    file.seek(SeekFrom::Start(offset))?;

    Response::Accept { offset }.write_to(stream).await?;
    let pos = match receive_pieces(stream, file, offset, size, Some(meta), progress, stop).await? {
        Ok(pos) => pos,
        Err(Stop::Discard) => {
            partial.remove();
            return Ok(None);
        }
        Err(Stop::Keep) => return Ok(None),
    };
    Ok(Some(Landed {
        partial,
        dir: dir.to_owned(),
        name,
        parts,
        pack,
        stats: TransferStats {
            bytes: pos - offset,
            resumed_from: offset,
            elapsed: start.elapsed(),
        },
    }))
}

/// Receive one transfer into `sink` without saving a file or resume state.
/// Used to measure network speed without the disk (`nudge receive --discard`).
pub async fn receive_into<S, W>(stream: &mut S, sink: W) -> Result<(Offer, TransferStats)>
where
    S: AsyncRead + AsyncWrite + Unpin,
    W: Sink,
{
    let offer = Offer::read_from(stream).await?;
    let start = Instant::now();
    Response::Accept { offset: 0 }.write_to(stream).await?;
    let bytes = receive_pieces(
        stream,
        sink,
        0,
        offer.size,
        None,
        &Progress::new(),
        std::future::pending::<()>(),
    )
    .await?
    .expect("never stopped");
    Response::Done.write_to(stream).await?;
    let stats = TransferStats {
        bytes,
        resumed_from: 0,
        elapsed: start.elapsed(),
    };
    Ok((offer, stats))
}

/// Read the data stream from `offset` to `size` and hand it to a writer thread
/// that checks hashes and writes into `sink`. Returns the end position, or
/// what `stop` gave if it finished first. Network errors are held until the
/// writer has finished, so its progress is saved before we return.
async fn receive_pieces<S, W, F, T>(
    stream: &mut S,
    sink: W,
    offset: u64,
    size: u64,
    meta: Option<MetaWriter>,
    progress: &Progress,
    stop: F,
) -> Result<std::result::Result<u64, T>>
where
    S: AsyncRead + Unpin,
    W: Sink,
    F: Future<Output = T>,
{
    progress.set(offset);
    let (tx, rx) = mpsc::channel(PIPELINE_DEPTH);
    let pool = Pool::default();
    let spares = pool.clone();
    let writer = tokio::task::spawn_blocking(move || write_pieces(sink, rx, offset, meta, spares));

    let network = async move {
        let truncated = |e: io::Error, pos: u64| {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                Error::Truncated {
                    got: pos,
                    expected: size,
                }
            } else {
                e.into()
            }
        };
        let mut pos = offset;
        while pos < size {
            let end = chunk_end(pos, size);
            while pos < end {
                let want = (end - pos).min(BUF_SIZE as u64) as usize;
                let mut buf = pool.get(want);
                stream
                    .read_exact(&mut buf)
                    .await
                    .map_err(|e| truncated(e, pos))?;
                pos += want as u64;
                progress.set(pos);
                if tx.send(Piece::Data(buf)).await.is_err() {
                    return Ok(pos); // writer failed; its error is reported below
                }
            }
            let mut hash = [0u8; HASH_LEN];
            stream
                .read_exact(&mut hash)
                .await
                .map_err(|e| truncated(e, pos))?;
            if tx.send(Piece::ChunkEnd(hash)).await.is_err() {
                return Ok(pos);
            }
        }
        Ok(pos)
        // `tx` is dropped here, which tells the writer the stream has ended.
    };
    // Stopping drops the network side, which closes the channel: the writer
    // then finishes what it has and returns, so nothing is still writing
    // when the caller deletes the file.
    let network_result: std::result::Result<Result<u64>, T> = tokio::select! {
        result = network => Ok(result),
        why = stop => Err(why),
    };
    writer.await.map_err(io::Error::other)??;
    match network_result {
        Ok(result) => result.map(Ok),
        Err(why) => Ok(Err(why)),
    }
}

/// Disk side of sending: read from `offset`, hash each chunk, and pass data
/// and hashes to the network loop in order.
fn read_pieces<R: Read + Seek>(
    mut source: R,
    offset: u64,
    size: u64,
    tx: mpsc::Sender<io::Result<Piece>>,
    pool: Pool,
) {
    let mut run = || -> io::Result<()> {
        source.seek(SeekFrom::Start(offset))?;
        let mut hasher = blake3::Hasher::new();
        let mut pos = offset;
        while pos < size {
            let end = chunk_end(pos, size);
            let want = (end - pos).min(BUF_SIZE as u64) as usize;
            let mut buf = pool.get(want);
            source.read_exact(&mut buf)?;
            hasher.update(&buf);
            pos += want as u64;
            if tx.blocking_send(Ok(Piece::Data(buf))).is_err() {
                return Ok(()); // network side gave up
            }
            if pos == end {
                let hash = *hasher.finalize().as_bytes();
                hasher.reset();
                if tx.blocking_send(Ok(Piece::ChunkEnd(hash))).is_err() {
                    return Ok(());
                }
            }
        }
        Ok(())
    };
    if let Err(e) = run() {
        let _ = tx.blocking_send(Err(e));
    }
}

/// Disk side of receiving: write data (the sink is already positioned at
/// `offset`), check each chunk's hash, and record verified chunks for resume
/// when `meta` is given. Always syncs and records progress before returning,
/// whether the transfer finished, failed or was cut off, so `Done` is only
/// sent once the data is safely stored.
fn write_pieces<W: Sink>(
    mut sink: W,
    mut rx: mpsc::Receiver<Piece>,
    offset: u64,
    mut meta: Option<MetaWriter>,
    pool: Pool,
) -> Result<()> {
    let mut receive = || -> Result<()> {
        let mut hasher = blake3::Hasher::new();
        let mut chunk = offset / CHUNK_SIZE;
        while let Some(piece) = rx.blocking_recv() {
            match piece {
                Piece::Data(buf) => {
                    hasher.update(&buf);
                    sink.write_all(&buf)?;
                    pool.put(buf);
                }
                Piece::ChunkEnd(expected) => {
                    if hasher.finalize().as_bytes() != &expected {
                        return Err(Error::ChunkMismatch { chunk });
                    }
                    hasher.reset();
                    chunk += 1;
                    if let Some(meta) = meta.as_mut() {
                        meta.push(&expected);
                        if meta.pending_chunks() >= CHECKPOINT_CHUNKS {
                            checkpoint(&mut sink, meta)?;
                        }
                    }
                }
            }
        }
        Ok(())
    };
    let result = receive();
    let saved = match meta.as_mut() {
        Some(meta) => checkpoint(&mut sink, meta),
        None => sink.sync(),
    };
    result?;
    saved?;
    Ok(())
}

/// Make written data durable, then record its chunks as resumable.
/// The order matters: the meta file must never get ahead of the data.
fn checkpoint<W: Sink>(sink: &mut W, meta: &mut MetaWriter) -> io::Result<()> {
    sink.sync()?;
    meta.flush()
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
pub(crate) fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let taken = |p: &Path| p.exists() || with_suffix(p, PARTIAL_SUFFIX).exists();
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
