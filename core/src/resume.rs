//! Resume state for partially received files.
//!
//! Next to every `<name>.nudgepart` the receiver keeps `<name>.nudgemeta`:
//! a header identifying the transfer (sender, offered name, size, modified
//! time), then the 32-byte BLAKE3 hash of each chunk that has been verified
//! *and* synced to disk, in order. Hashes are only appended after the data
//! file is synced, so the meta file never claims data a crash could lose.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::protocol::{HASH_LEN, MAX_NAME_LEN};
use crate::transfer::{CHUNK_SIZE, PARTIAL_SUFFIX, unique_path};

/// Suffix of the resume-state file kept next to each partial file.
pub const META_SUFFIX: &str = ".nudgemeta";

const MAGIC: [u8; 4] = *b"NDGM";
const VERSION: u16 = 1;
const FIXED_HEADER: usize = 4 + 2 + 32 + 8 + 8 + 2;

/// Everything that must match for a partial file to be resumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferKey {
    /// Sender's fingerprint (all zeros when unknown, e.g. plain TCP).
    pub peer: [u8; 32],
    /// File name exactly as offered (before sanitizing).
    pub name: String,
    pub size: u64,
    /// Sender's modification time in ns since the Unix epoch; 0 = unknown.
    pub modified: u64,
}

impl TransferKey {
    fn encode(&self) -> Vec<u8> {
        let name = self.name.as_bytes();
        let mut b = Vec::with_capacity(FIXED_HEADER + name.len());
        b.extend_from_slice(&MAGIC);
        b.extend_from_slice(&VERSION.to_be_bytes());
        b.extend_from_slice(&self.peer);
        b.extend_from_slice(&self.size.to_be_bytes());
        b.extend_from_slice(&self.modified.to_be_bytes());
        b.extend_from_slice(&(name.len() as u16).to_be_bytes());
        b.extend_from_slice(name);
        b
    }

    /// Returns the key and the header's length in bytes.
    fn decode(b: &[u8]) -> Option<(Self, usize)> {
        if b.len() < FIXED_HEADER || b[..4] != MAGIC || b[4..6] != VERSION.to_be_bytes() {
            return None;
        }
        let peer = b[6..38].try_into().ok()?;
        let size = u64::from_be_bytes(b[38..46].try_into().ok()?);
        let modified = u64::from_be_bytes(b[46..54].try_into().ok()?);
        let len = u16::from_be_bytes(b[54..56].try_into().ok()?) as usize;
        let name = std::str::from_utf8(b.get(FIXED_HEADER..FIXED_HEADER + len)?).ok()?;
        let key = Self {
            peer,
            name: name.to_owned(),
            size,
            modified,
        };
        Some((key, FIXED_HEADER + len))
    }

    fn total_chunks(&self) -> u64 {
        self.size.div_ceil(CHUNK_SIZE)
    }
}

/// A partial file on disk: `<base>.nudgepart` plus `<base>.nudgemeta`.
#[derive(Debug)]
pub(crate) struct Partial {
    /// The name the finished file would like to have.
    pub base: PathBuf,
    /// Chunks verified and synced, counted from the start of the file.
    pub chunks_done: u64,
}

impl Partial {
    pub fn part_path(&self) -> PathBuf {
        with_suffix(&self.base, PARTIAL_SUFFIX)
    }

    pub fn meta_path(&self) -> PathBuf {
        with_suffix(&self.base, META_SUFFIX)
    }

    pub(crate) fn remove(&self) {
        let _ = fs::remove_file(self.part_path());
        let _ = fs::remove_file(self.meta_path());
    }
}

pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Look in `dir` for a partial file of exactly this transfer.
///
/// Partials from the same sender with the same name but a different size or
/// modified time are an older version of the file and can never be resumed,
/// so they are deleted. Unreadable meta files are left alone.
pub(crate) fn find(dir: &Path, key: &TransferKey) -> io::Result<Option<Partial>> {
    let mut found = None;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(base_name) = file_name.to_str().and_then(|n| n.strip_suffix(META_SUFFIX)) else {
            continue;
        };
        let Some((stored, chunks_done)) = read_meta(&entry.path()) else {
            continue;
        };
        if stored.peer != key.peer || stored.name != key.name {
            continue;
        }
        let partial = Partial {
            base: dir.join(base_name),
            chunks_done,
        };
        let part_intact = fs::metadata(partial.part_path()).is_ok_and(|m| m.len() == stored.size);
        let usable = stored == *key
            && key.modified != 0 // without a modified time, versions can't be told apart
            && part_intact
            && chunks_done <= key.total_chunks()
            && found.is_none();
        if usable {
            found = Some(partial);
        } else {
            partial.remove();
        }
    }
    Ok(found)
}

/// Header and number of whole chunk hashes in a meta file.
fn read_meta(path: &Path) -> Option<(TransferKey, u64)> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut head = Vec::new();
    (&mut file)
        .take((FIXED_HEADER + MAX_NAME_LEN) as u64)
        .read_to_end(&mut head)
        .ok()?;
    let (key, header_len) = TransferKey::decode(&head)?;
    Some((key, (len - header_len as u64) / HASH_LEN as u64))
}

/// Start a new partial file in `dir`, preallocated to the full size.
pub(crate) fn create(
    dir: &Path,
    key: &TransferKey,
    safe_name: &str,
) -> io::Result<(Partial, File, MetaWriter)> {
    let partial = Partial {
        base: unique_path(dir, safe_name),
        chunks_done: 0,
    };
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(partial.part_path())?;
    // Reserve the full size up front so the OS doesn't grow the file piecemeal.
    file.set_len(key.size)?;
    let mut meta = File::create(partial.meta_path())?;
    meta.write_all(&key.encode())?;
    Ok((partial, file, MetaWriter::new(meta)))
}

/// Reopen a partial found by `find` for writing more chunks.
pub(crate) fn reopen(partial: &Partial, key: &TransferKey) -> io::Result<(File, MetaWriter)> {
    let file = OpenOptions::new().write(true).open(partial.part_path())?;
    let mut meta = OpenOptions::new().write(true).open(partial.meta_path())?;
    // Drop any half-written hash left by a crash, then append after the last whole one.
    let keep = key.encode().len() as u64 + partial.chunks_done * HASH_LEN as u64;
    meta.set_len(keep)?;
    meta.seek(SeekFrom::End(0))?;
    Ok((file, MetaWriter::new(meta)))
}

/// Delete the meta file once the partial has become the finished file.
pub(crate) fn finish(partial: &Partial) {
    let _ = fs::remove_file(partial.meta_path());
}

/// Appends chunk hashes to a meta file in batches.
pub(crate) struct MetaWriter {
    file: File,
    pending: Vec<u8>,
}

impl MetaWriter {
    fn new(file: File) -> Self {
        Self {
            file,
            pending: Vec::new(),
        }
    }

    /// Record a verified chunk. It is written out by the next `flush`, which
    /// must only happen after the data file has been synced.
    pub fn push(&mut self, hash: &[u8; HASH_LEN]) {
        self.pending.extend_from_slice(hash);
    }

    pub fn pending_chunks(&self) -> usize {
        self.pending.len() / HASH_LEN
    }

    pub fn flush(&mut self) -> io::Result<()> {
        if !self.pending.is_empty() {
            self.file.write_all(&self.pending)?;
            self.pending.clear();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str, size: u64, modified: u64) -> TransferKey {
        TransferKey {
            peer: [7; 32],
            name: name.into(),
            size,
            modified,
        }
    }

    #[test]
    fn header_round_trips() {
        let k = key("holiday 📷.mp4", 123_456_789, 42);
        let bytes = k.encode();
        assert_eq!(TransferKey::decode(&bytes), Some((k, bytes.len())));
        assert_eq!(TransferKey::decode(b"junk"), None);
    }

    #[test]
    fn finds_matching_partial_and_counts_whole_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let k = key("a.bin", 3 * CHUNK_SIZE, 99);
        let (partial, _file, mut meta) = create(dir.path(), &k, "a.bin").unwrap();
        meta.push(&[1; 32]);
        meta.push(&[2; 32]);
        meta.flush().unwrap();
        // A torn third hash (crash mid-write) must be ignored.
        meta.file.write_all(&[3; 10]).unwrap();
        drop(meta);

        let found = find(dir.path(), &k).unwrap().unwrap();
        assert_eq!(found.base, partial.base);
        assert_eq!(found.chunks_done, 2);

        reopen(&found, &k).unwrap();
        let len = fs::metadata(found.meta_path()).unwrap().len();
        assert_eq!(len, k.encode().len() as u64 + 2 * HASH_LEN as u64);
    }

    #[test]
    fn stale_version_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let old = key("a.bin", 100, 1);
        let (partial, _, _) = create(dir.path(), &old, "a.bin").unwrap();
        let new = key("a.bin", 100, 2);
        assert!(find(dir.path(), &new).unwrap().is_none());
        assert!(!partial.part_path().exists());
        assert!(!partial.meta_path().exists());
    }

    #[test]
    fn other_senders_partials_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let mine = key("a.bin", 100, 1);
        let (partial, _, _) = create(dir.path(), &mine, "a.bin").unwrap();
        let theirs = TransferKey {
            peer: [9; 32],
            ..mine
        };
        assert!(find(dir.path(), &theirs).unwrap().is_none());
        assert!(partial.part_path().exists());
    }

    #[test]
    fn unknown_modified_time_never_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let k = key("a.bin", 100, 0);
        create(dir.path(), &k, "a.bin").unwrap();
        assert!(find(dir.path(), &k).unwrap().is_none());
    }
}
