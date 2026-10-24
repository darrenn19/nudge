//! Packs: many small files sent as one transfer, unpacked on arrival.
//!
//! Each file sent on its own costs a connection, an offer and its answer,
//! and a sync to disk. For a folder of a thousand notes that overhead is
//! the whole transfer. So small files go together, as one stream:
//!
//! ```text
//! "NDGP"  u16 version  u32 count
//! count × (u16 path length, path (UTF-8, '/'-separated), u64 size, u64 modified)
//! the files' bytes, one after another, in the same order
//! ```
//!
//! On the wire a pack is an ordinary file transfer (`Offer` with `pack`
//! set), so it's chunk-hashed, resumed and asked about exactly like one
//! file. The receiver lands it as one partial file and unpacks it once it's
//! accepted. Unpacking copies the data once more, which is why only small
//! files are packed: large ones go on their own, where the per-file cost
//! doesn't matter.
//!
//! A path ending in '/' is an empty folder (size 0, no bytes), so folders
//! arrive as they were, empty ones included.

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use crate::transfer::{BUF_SIZE, sanitize_file_name, unique_path};
use crate::{Error, Result};

const MAGIC: [u8; 4] = *b"NDGP";
const VERSION: u16 = 1;

/// Files larger than this are sent on their own.
pub const PACK_FILE_MAX: u64 = 8 << 20;
/// Only worth it with at least this many small files.
pub const PACK_MIN_FILES: usize = 4;
/// A pack holds at most this many bytes of files...
pub const PACK_MAX_BYTES: u64 = 256 << 20;
/// ...and at most this many files.
pub const PACK_MAX_FILES: usize = 4000;
/// Folders deeper than this aren't followed (and aren't accepted).
const MAX_DEPTH: usize = 64;

/// Where a packed file's bytes come from.
#[derive(Debug)]
pub enum Source {
    Path(PathBuf),
    /// Already open (Android shares files as descriptors).
    File(File),
    /// An empty folder: no bytes. Its entry's path ends in '/'.
    Folder,
}

impl Source {
    fn open(&self) -> io::Result<Option<File>> {
        match self {
            Self::Path(path) => File::open(path).map(Some),
            Self::File(file) => file.try_clone().map(Some),
            Self::Folder => Ok(None),
        }
    }

    fn try_clone(&self) -> io::Result<Self> {
        Ok(match self {
            Self::Path(path) => Self::Path(path.clone()),
            Self::File(file) => Self::File(file.try_clone()?),
            Self::Folder => Self::Folder,
        })
    }
}

/// One file in a pack.
#[derive(Debug)]
pub struct Entry {
    /// Where it goes, relative to the receiver's folder: "notes.txt", or
    /// "Trip/day 1/notes.txt" for a file from a folder.
    pub path: String,
    pub size: u64,
    /// Modification time in ns since the Unix epoch; 0 = unknown.
    pub modified: u64,
    pub source: Source,
}

impl Entry {
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            path: self.path.clone(),
            size: self.size,
            modified: self.modified,
            source: self.source.try_clone()?,
        })
    }
}

/// Files to send as one transfer.
#[derive(Debug)]
pub struct Pack {
    entries: Vec<Entry>,
    manifest: Vec<u8>,
}

impl Pack {
    pub fn new(entries: Vec<Entry>) -> Self {
        let mut manifest = Vec::new();
        manifest.extend_from_slice(&MAGIC);
        manifest.extend_from_slice(&VERSION.to_be_bytes());
        manifest.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for e in &entries {
            let path = e.path.as_bytes();
            manifest.extend_from_slice(&(path.len() as u16).to_be_bytes());
            manifest.extend_from_slice(path);
            manifest.extend_from_slice(&e.size.to_be_bytes());
            manifest.extend_from_slice(&e.modified.to_be_bytes());
        }
        Self { entries, manifest }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Bytes on the wire: the file list and every file.
    pub fn size(&self) -> u64 {
        self.manifest.len() as u64 + self.entries.iter().map(|e| e.size).sum::<u64>()
    }

    /// Stands in for a modified time, so an interrupted pack resumes only
    /// into exactly the same files: a hash of the file list (never 0).
    pub fn fingerprint(&self) -> u64 {
        let hash = blake3::hash(&self.manifest);
        u64::from_be_bytes(hash.as_bytes()[..8].try_into().expect("8 bytes")).max(1)
    }

    /// What people see: the folder's name if everything is in one, else
    /// "12 files".
    pub fn display_name(&self) -> String {
        let top = |e: &Entry| e.path.split('/').next().unwrap_or("").to_owned();
        let first = self.entries.first().map(top).unwrap_or_default();
        let one_folder = self
            .entries
            .iter()
            .all(|e| e.path.contains('/') && top(e) == first);
        if one_folder && !first.is_empty() {
            first
        } else {
            format!("{} files", self.entries.len())
        }
    }

    /// The pack's bytes, from any position.
    pub fn reader(&self) -> io::Result<PackReader> {
        let mut files = Vec::with_capacity(self.entries.len());
        let mut start = self.manifest.len() as u64;
        for e in &self.entries {
            if let Some(file) = e.source.open()? {
                files.push((start, e.size, file));
                start += e.size;
            }
        }
        Ok(PackReader {
            manifest: self.manifest.clone(),
            files,
            total: start,
            pos: 0,
        })
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            entries: self
                .entries
                .iter()
                .map(Entry::try_clone)
                .collect::<io::Result<_>>()?,
            manifest: self.manifest.clone(),
        })
    }
}

/// A pack as one seekable stream: the file list, then each file in turn.
pub struct PackReader {
    manifest: Vec<u8>,
    /// (start in the stream, size, file)
    files: Vec<(u64, u64, File)>,
    total: u64,
    pos: u64,
}

impl Read for PackReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.total || buf.is_empty() {
            return Ok(0);
        }
        let manifest_len = self.manifest.len() as u64;
        if self.pos < manifest_len {
            let from = self.pos as usize;
            let n = buf.len().min(self.manifest.len() - from);
            buf[..n].copy_from_slice(&self.manifest[from..from + n]);
            self.pos += n as u64;
            return Ok(n);
        }
        // The file holding `pos` (files are in order; skip empty ones).
        let i = self
            .files
            .partition_point(|(start, size, _)| start + size <= self.pos);
        let (start, size, file) = &mut self.files[i];
        let within = self.pos - *start;
        file.seek(SeekFrom::Start(within))?;
        let want = buf.len().min((*size - within) as usize);
        let n = file.read(&mut buf[..want])?;
        if n == 0 {
            // It got shorter since the pack was made.
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "a file changed while sending",
            ));
        }
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for PackReader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
            SeekFrom::End(d) => self.total as i128 + d as i128,
        };
        if pos < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        self.pos = pos as u64;
        Ok(self.pos)
    }
}

/// A relative path from the other side, made safe: each part sanitized
/// like a file name, and nothing that could climb out of the folder.
pub fn safe_relative(raw: &str) -> Result<Vec<String>> {
    let parts: Vec<&str> = raw.split('/').collect();
    if parts.is_empty() || parts.len() > MAX_DEPTH {
        return Err(Error::BadFileName(raw.to_owned()));
    }
    parts
        .into_iter()
        .map(|part| match part {
            "" | "." | ".." => Err(Error::BadFileName(raw.to_owned())),
            part => sanitize_file_name(part),
        })
        .collect()
}

/// Where a received file at relative path `parts` goes under `dir`: its
/// folders created, and a free name (never overwriting anything).
pub fn place(dir: &Path, parts: &[String]) -> io::Result<PathBuf> {
    let (name, folders) = parts.split_last().expect("at least one part");
    let mut parent = dir.to_path_buf();
    for folder in folders {
        parent.push(folder);
    }
    fs::create_dir_all(&parent)?;
    let wanted = parent.join(name);
    Ok(if wanted.exists() {
        unique_path(&parent, name)
    } else {
        wanted
    })
}

/// Unpack a received pack (`packed`, verified) into `dir`. Returns where it
/// went: the folder if everything was in one, else `dir` itself. On an
/// error, files already unpacked stay.
pub fn unpack(packed: &Path, dir: &Path) -> Result<PathBuf> {
    let total = fs::metadata(packed)?.len();
    let mut r = BufReader::with_capacity(BUF_SIZE, File::open(packed)?);
    let bad = || Error::Protocol("damaged pack");
    let mut head = [0u8; 10];
    r.read_exact(&mut head)?;
    if head[..4] != MAGIC || head[4..6] != VERSION.to_be_bytes() {
        return Err(bad());
    }
    let count = u32::from_be_bytes(head[6..10].try_into().expect("4 bytes")) as usize;
    if count > PACK_MAX_FILES {
        return Err(bad());
    }
    let mut entries = Vec::with_capacity(count);
    let mut listed = 10u64;
    for _ in 0..count {
        let mut len = [0u8; 2];
        r.read_exact(&mut len)?;
        let mut path = vec![0u8; u16::from_be_bytes(len) as usize];
        r.read_exact(&mut path)?;
        let mut numbers = [0u8; 16];
        r.read_exact(&mut numbers)?;
        let size = u64::from_be_bytes(numbers[..8].try_into().expect("8 bytes"));
        let modified = u64::from_be_bytes(numbers[8..].try_into().expect("8 bytes"));
        let path = String::from_utf8(path).map_err(|_| bad())?;
        listed += 2 + path.len() as u64 + 16;
        // "Trip/empty/" is an empty folder.
        let folder = path.ends_with('/');
        if folder && size != 0 {
            return Err(bad());
        }
        let parts = safe_relative(path.strip_suffix('/').unwrap_or(&path))?;
        entries.push((folder, parts, size, modified));
    }
    let data: u64 = entries.iter().map(|(_, _, size, _)| size).sum();
    if listed.checked_add(data) != Some(total) {
        return Err(bad());
    }

    let mut tops = Vec::new();
    for (folder, parts, size, modified) in entries {
        tops.push(if parts.len() > 1 || folder {
            Some(parts[0].clone())
        } else {
            None
        });
        if folder {
            let mut path = dir.to_path_buf();
            path.extend(&parts);
            fs::create_dir_all(path)?;
            continue;
        }
        let dest = place(dir, &parts)?;
        let buffer = (size as usize).clamp(8 << 10, BUF_SIZE);
        let mut out = BufWriter::with_capacity(buffer, File::create(&dest)?);
        let copied = io::copy(&mut (&mut r).take(size), &mut out)?;
        if copied != size {
            return Err(bad());
        }
        let file = out.into_inner().map_err(|e| e.into_error())?;
        if modified != 0 {
            let _ = file.set_modified(UNIX_EPOCH + Duration::from_nanos(modified));
        }
        // Not synced one by one: for a thousand small files that alone
        // doubles the time (0.75 s vs 1.6 s on the benchmark). The pack was
        // verified and synced before this; what's at risk is a power cut in
        // the seconds after, as with any ordinary copy.
    }
    // Everything in one folder: that folder is what arrived.
    match tops.first() {
        Some(Some(top)) if tops.iter().all(|t| t.as_ref() == Some(top)) => Ok(dir.join(top)),
        _ => Ok(dir.to_path_buf()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(dir: &Path, rel: &str, bytes: &[u8]) -> Entry {
        let path = dir.join(rel.replace('/', "_"));
        fs::write(&path, bytes).unwrap();
        Entry {
            path: rel.to_owned(),
            size: bytes.len() as u64,
            modified: 1_700_000_000_000_000_000,
            source: Source::Path(path),
        }
    }

    #[test]
    fn packs_and_unpacks_a_folder() {
        let src = tempfile::tempdir().unwrap();
        let pack = Pack::new(vec![
            entry(src.path(), "Trip/a.txt", b"first"),
            entry(src.path(), "Trip/day 2/b.txt", b"second file"),
            entry(src.path(), "Trip/empty.txt", b""),
            Entry {
                path: "Trip/nothing yet/".into(),
                size: 0,
                modified: 0,
                source: Source::Folder,
            },
        ]);
        assert_eq!(pack.display_name(), "Trip");

        // Read in odd-sized pieces, with a seek back, as resume would.
        let mut reader = pack.reader().unwrap();
        let mut bytes = Vec::new();
        let mut buf = [0u8; 7];
        loop {
            let n = reader.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&buf[..n]);
        }
        assert_eq!(bytes.len() as u64, pack.size());
        reader.seek(SeekFrom::Start(3)).unwrap();
        let mut again = vec![0u8; bytes.len() - 3];
        reader.read_exact(&mut again).unwrap();
        assert_eq!(again, bytes[3..]);

        let out = tempfile::tempdir().unwrap();
        let packed = out.path().join("p.nudgepart");
        fs::write(&packed, &bytes).unwrap();
        // Something called Trip/a.txt is already there: kept, not replaced.
        fs::create_dir_all(out.path().join("Trip")).unwrap();
        fs::write(out.path().join("Trip/a.txt"), b"mine").unwrap();
        let arrived = unpack(&packed, out.path()).unwrap();
        assert_eq!(arrived, out.path().join("Trip"));
        assert_eq!(fs::read(out.path().join("Trip/a.txt")).unwrap(), b"mine");
        assert_eq!(
            fs::read(out.path().join("Trip/a (1).txt")).unwrap(),
            b"first"
        );
        assert_eq!(
            fs::read(out.path().join("Trip/day 2/b.txt")).unwrap(),
            b"second file"
        );
        assert_eq!(fs::read(out.path().join("Trip/empty.txt")).unwrap(), b"");
        assert!(out.path().join("Trip/nothing yet").is_dir());
    }

    #[test]
    fn refuses_paths_that_climb_out() {
        assert!(safe_relative("../evil.txt").is_err());
        assert!(safe_relative("a/../../evil.txt").is_err());
        assert!(safe_relative("/abs.txt").is_err());
        assert!(safe_relative("a//b.txt").is_err());
        assert_eq!(safe_relative("a/b:c.txt").unwrap(), ["a", "b_c.txt"]);
    }

    #[test]
    fn damaged_packs_are_refused() {
        let out = tempfile::tempdir().unwrap();
        let packed = out.path().join("p");
        fs::write(&packed, b"NDGP\x00\x01\x00\x00\x00\x01\x00\x01a").unwrap();
        assert!(unpack(&packed, out.path()).is_err());
    }
}
