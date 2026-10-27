//! A zip archive written on the fly, for sending several files to a browser
//! as one download (browsers hold back a second download no tap started,
//! so one per file isn't reliable).
//!
//! Entries are stored, not compressed: what people send is mostly photos
//! and videos, already compressed, and a stored entry streams at full speed.
//! Each file's CRC-32 is worked out as it streams and written after it (a
//! data descriptor), so nothing is read twice. Its exact size is known
//! before the first byte, so the browser can show progress. Zip64 is used
//! only where it's needed (a file or an offset past 4 GB, or 65535+ files),
//! so ordinary archives open in every unzip tool.

/// The size limit of a classic (non-Zip64) field.
const MAX32: u64 = 0xFFFF_FFFF;
const FLAGS: u16 = 0x0808; // sizes in a data descriptor; UTF-8 names
const VERSION_ZIP64: u16 = 45;
const VERSION_PLAIN: u16 = 20;

/// One file in the archive.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Its path in the archive, with `/` between folders.
    pub name: String,
    pub size: u64,
    /// Modified time, as Unix seconds (0 if unknown).
    pub modified: u64,
}

impl Entry {
    fn zip64(&self) -> bool {
        self.size >= MAX32
    }

    /// MS-DOS time and date (UTC), as zip stores them.
    fn dos_time(&self) -> (u16, u16) {
        let secs = self.modified;
        let days = (secs / 86_400) as i64;
        let rem = secs % 86_400;
        let (y, m, d) = civil_from_days(days);
        if y < 1980 {
            return (0, (1 << 5) | 1); // 1980-01-01
        }
        let time = ((rem / 3600) << 11) | (((rem % 3600) / 60) << 5) | ((rem % 60) / 2);
        let date = (((y - 1980).min(127) as u64) << 9) | ((m as u64) << 5) | d as u64;
        (time as u16, date as u16)
    }
}

/// Year, month, day of a day count from 1970-01-01 (Howard Hinnant's
/// algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Writes the parts around the file data, in order: for each entry
/// `local_header`, its bytes, `descriptor`; then `finish`.
pub struct Archive {
    entries: Vec<Entry>,
    /// Where each entry's local header starts, and its CRC once known.
    offsets: Vec<u64>,
    crcs: Vec<u32>,
    written: u64,
}

impl Archive {
    /// Names are made unique ("a.jpg", "a (2).jpg") so nothing is lost.
    pub fn new(mut entries: Vec<Entry>) -> Self {
        let mut seen = std::collections::HashSet::new();
        for e in &mut entries {
            e.name = e.name.replace('\\', "/");
            if !seen.insert(e.name.to_lowercase()) {
                let (stem, ext) = match e.name.rfind('.') {
                    Some(dot) if dot > e.name.rfind('/').map_or(0, |s| s + 1) => {
                        (e.name[..dot].to_owned(), e.name[dot..].to_owned())
                    }
                    _ => (e.name.clone(), String::new()),
                };
                let unique = (2..)
                    .map(|n| format!("{stem} ({n}){ext}"))
                    .find(|n| !seen.contains(&n.to_lowercase()))
                    .expect("unbounded search always finds a free name");
                seen.insert(unique.to_lowercase());
                e.name = unique;
            }
        }
        let n = entries.len();
        Self {
            entries,
            offsets: Vec::with_capacity(n),
            crcs: Vec::with_capacity(n),
            written: 0,
        }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The archive's size, exactly, before any of it is written.
    pub fn total_size(&self) -> u64 {
        let mut dry = Self {
            entries: self.entries.clone(),
            offsets: Vec::new(),
            crcs: Vec::new(),
            written: 0,
        };
        for i in 0..dry.entries.len() {
            let size = dry.entries[i].size;
            dry.local_header(i);
            dry.data(size);
            dry.descriptor(i, 0);
        }
        dry.finish();
        dry.written
    }

    /// Before entry `i`'s bytes.
    pub fn local_header(&mut self, i: usize) -> Vec<u8> {
        let e = &self.entries[i];
        let zip64 = e.zip64();
        let (time, date) = e.dos_time();
        let mut b = Vec::with_capacity(30 + e.name.len() + 20);
        put32(&mut b, 0x0403_4b50);
        put16(&mut b, if zip64 { VERSION_ZIP64 } else { VERSION_PLAIN });
        put16(&mut b, FLAGS);
        put16(&mut b, 0); // stored
        put16(&mut b, time);
        put16(&mut b, date);
        put32(&mut b, 0); // CRC: in the descriptor
        // Sizes are known, so they're given here too (readers that go by
        // the local header, not the central directory, need them).
        let small = if zip64 { MAX32 as u32 } else { e.size as u32 };
        put32(&mut b, small);
        put32(&mut b, small);
        put16(&mut b, e.name.len() as u16);
        put16(&mut b, if zip64 { 20 } else { 0 });
        b.extend_from_slice(e.name.as_bytes());
        if zip64 {
            put16(&mut b, 0x0001);
            put16(&mut b, 16);
            put64(&mut b, e.size);
            put64(&mut b, e.size);
        }
        self.offsets.push(self.written);
        self.written += b.len() as u64;
        b
    }

    /// Count an entry's bytes, written by the caller.
    pub fn data(&mut self, len: u64) {
        self.written += len;
    }

    /// After entry `i`'s bytes, with their CRC-32.
    pub fn descriptor(&mut self, i: usize, crc: u32) -> Vec<u8> {
        let e = &self.entries[i];
        let mut b = Vec::with_capacity(24);
        put32(&mut b, 0x0807_4b50);
        put32(&mut b, crc);
        if e.zip64() {
            put64(&mut b, e.size);
            put64(&mut b, e.size);
        } else {
            put32(&mut b, e.size as u32);
            put32(&mut b, e.size as u32);
        }
        self.crcs.push(crc);
        self.written += b.len() as u64;
        b
    }

    /// The central directory and end records.
    pub fn finish(&mut self) -> Vec<u8> {
        let start = self.written;
        let mut b = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            let offset = self.offsets[i];
            let big_size = e.zip64();
            let big_offset = offset >= MAX32;
            let (time, date) = e.dos_time();
            let mut extra = Vec::new();
            if big_size {
                put64(&mut extra, e.size);
                put64(&mut extra, e.size);
            }
            if big_offset {
                put64(&mut extra, offset);
            }
            let version = if big_size || big_offset {
                VERSION_ZIP64
            } else {
                VERSION_PLAIN
            };
            put32(&mut b, 0x0201_4b50);
            put16(&mut b, version); // made by (MS-DOS attributes)
            put16(&mut b, version);
            put16(&mut b, FLAGS);
            put16(&mut b, 0);
            put16(&mut b, time);
            put16(&mut b, date);
            put32(&mut b, self.crcs[i]);
            let small = if big_size { MAX32 as u32 } else { e.size as u32 };
            put32(&mut b, small);
            put32(&mut b, small);
            put16(&mut b, e.name.len() as u16);
            put16(&mut b, if extra.is_empty() { 0 } else { extra.len() as u16 + 4 });
            put16(&mut b, 0); // comment
            put16(&mut b, 0); // disk
            put16(&mut b, 0); // internal attributes
            put32(&mut b, 0); // external attributes
            put32(&mut b, if big_offset { MAX32 as u32 } else { offset as u32 });
            b.extend_from_slice(e.name.as_bytes());
            if !extra.is_empty() {
                put16(&mut b, 0x0001);
                put16(&mut b, extra.len() as u16);
                b.extend_from_slice(&extra);
            }
        }
        let dir_size = b.len() as u64;
        let count = self.entries.len() as u64;
        let zip64 = start >= MAX32 || dir_size >= MAX32 || count >= 0xFFFF;
        if zip64 {
            let record = start + dir_size;
            put32(&mut b, 0x0606_4b50);
            put64(&mut b, 44);
            put16(&mut b, VERSION_ZIP64);
            put16(&mut b, VERSION_ZIP64);
            put32(&mut b, 0);
            put32(&mut b, 0);
            put64(&mut b, count);
            put64(&mut b, count);
            put64(&mut b, dir_size);
            put64(&mut b, start);
            put32(&mut b, 0x0706_4b50);
            put32(&mut b, 0);
            put64(&mut b, record);
            put32(&mut b, 1);
        }
        put32(&mut b, 0x0605_4b50);
        put16(&mut b, 0);
        put16(&mut b, 0);
        let small_count = if zip64 { 0xFFFF } else { count as u16 };
        put16(&mut b, small_count);
        put16(&mut b, small_count);
        put32(&mut b, if zip64 { MAX32 as u32 } else { dir_size as u32 });
        put32(&mut b, if zip64 { MAX32 as u32 } else { start as u32 });
        put16(&mut b, 0);
        self.written += b.len() as u64;
        b
    }
}

fn put16(b: &mut Vec<u8>, v: u16) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn put64(b: &mut Vec<u8>, v: u64) {
    b.extend_from_slice(&v.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn build(files: &[(&str, &[u8])]) -> Vec<u8> {
        let entries = files
            .iter()
            .map(|(name, data)| Entry {
                name: (*name).to_owned(),
                size: data.len() as u64,
                modified: 1_761_500_000,
            })
            .collect();
        let mut zip = Archive::new(entries);
        let expected = zip.total_size();
        let mut out = Vec::new();
        for (i, (_, data)) in files.iter().enumerate() {
            out.extend(zip.local_header(i));
            out.extend_from_slice(data);
            zip.data(data.len() as u64);
            out.extend(zip.descriptor(i, crc32fast::hash(data)));
        }
        out.extend(zip.finish());
        assert_eq!(out.len() as u64, expected, "size known up front");
        out
    }

    #[test]
    fn opens_in_a_zip_reader() {
        let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let bytes = build(&[
            ("a.txt", b"hello"),
            ("Trip/day 1/café.jpg", &big),
            ("a.txt", b"second a"),
            ("empty", b""),
        ]);
        let mut archive = ::zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut got = Vec::new();
        for i in 0..archive.len() {
            let mut f = archive.by_index(i).unwrap();
            let mut data = Vec::new();
            f.read_to_end(&mut data).unwrap(); // checks the CRC
            got.push((f.name().to_owned(), data.len()));
        }
        assert_eq!(
            got,
            [
                ("a.txt".to_owned(), 5),
                ("Trip/day 1/café.jpg".to_owned(), 200_000),
                ("a (2).txt".to_owned(), 8),
                ("empty".to_owned(), 0),
            ]
        );
    }

    #[test]
    fn sizes_past_4_gb_use_zip64() {
        // Only the layout: no need to write 4 GB to check the arithmetic.
        let entries = vec![
            Entry {
                name: "huge.bin".into(),
                size: 5 << 30,
                modified: 0,
            },
            Entry {
                name: "after.txt".into(),
                size: 3,
                modified: 0,
            },
        ];
        let mut zip = Archive::new(entries);
        let total = zip.total_size();
        let mut len = 0u64;
        len += zip.local_header(0).len() as u64;
        zip.data(5 << 30);
        len += 5 << 30;
        len += zip.descriptor(0, 0).len() as u64;
        len += zip.local_header(1).len() as u64;
        zip.data(3);
        len += 3;
        len += zip.descriptor(1, 0).len() as u64;
        let tail = zip.finish();
        len += tail.len() as u64;
        assert_eq!(len, total);
        // The second entry starts past 4 GB, so the end needs Zip64 records.
        assert!(tail.windows(4).any(|w| w == 0x0606_4b50u32.to_le_bytes()));
    }

    #[test]
    fn dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_388), (2025, 10, 27));
        let e = Entry {
            name: String::new(),
            size: 0,
            modified: 1_761_571_845, // 2025-10-27 13:30:45 UTC
        };
        let (time, date) = e.dos_time();
        assert_eq!(date, ((2025 - 1980) << 9) | (10 << 5) | 27);
        assert_eq!(time, (13 << 11) | (30 << 5) | 22);
    }
}
