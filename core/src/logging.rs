//! A small log of what the engine decided and why (paths, moves, the direct
//! link), for working out what happened in a transfer after the fact.
//!
//! Lines go to `nudge.log` in the data folder; past [`MAX_SIZE`] the file
//! becomes `nudge.log.old` and a new one starts, so it never grows beyond
//! about twice that. Nothing leaves the device.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use log::{Level, LevelFilter, Log, Metadata, Record};

/// Start a new file past this size.
const MAX_SIZE: u64 = 2 << 20;

pub const FILE: &str = "nudge.log";

/// Where lines go: nowhere until [`to_file`] or [`to_stderr`] says.
static SINK: Sink = Sink {
    file: Mutex::new(None),
    stderr: AtomicBool::new(false),
};

/// Make this the process's logger, with nowhere to write yet. Call it early,
/// before anything else claims the (one) logger slot; harmless to repeat.
pub fn install() {
    if log::set_logger(&SINK).is_ok() {
        log::set_max_level(LevelFilter::Info);
    }
}

/// Log to `nudge.log` in `dir` from now on.
pub fn to_file(dir: &Path) {
    install();
    let path = dir.join(FILE);
    let _ = fs::create_dir_all(dir);
    let Ok(file) = open(&path) else {
        return;
    };
    let size = file.metadata().map_or(0, |m| m.len());
    *SINK.file.lock().unwrap_or_else(|e| e.into_inner()) = Some(Open { path, file, size });
}

/// Log to standard error (the command-line tool).
pub fn to_stderr() {
    install();
    SINK.stderr.store(true, Ordering::Relaxed);
}

fn open(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

struct Open {
    path: PathBuf,
    file: File,
    size: u64,
}

struct Sink {
    file: Mutex<Option<Open>>,
    stderr: AtomicBool,
}

impl Log for Sink {
    fn enabled(&self, metadata: &Metadata) -> bool {
        // Only Nudge's own lines, not its libraries'.
        metadata.level() <= Level::Info && metadata.target().starts_with("nudge")
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} {:5} {}
",
            timestamp(SystemTime::now()),
            record.level(),
            record.args()
        );
        if self.stderr.load(Ordering::Relaxed) {
            eprint!("{line}");
        }
        let mut guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let Some(open) = guard.as_mut() else {
            return;
        };
        if open.size >= MAX_SIZE {
            let _ = fs::rename(&open.path, open.path.with_extension("log.old"));
            if let Ok(new) = self::open(&open.path) {
                open.file = new;
                open.size = 0;
            }
        }
        if open.file.write_all(line.as_bytes()).is_ok() {
            open.size += line.len() as u64;
        }
    }

    fn flush(&self) {
        if let Some(open) = self.file.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = open.file.flush();
        }
    }
}

/// "2026-10-24 09:15:02.481" in UTC.
fn timestamp(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs();
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let (y, m, day) = civil_from_days(days as i64);
    format!(
        "{y:04}-{m:02}-{day:02} {:02}:{:02}:{:02}.{:03}",
        rest / 3600,
        rest / 60 % 60,
        rest % 60,
        d.subsec_millis()
    )
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn formats_time() {
        let t = UNIX_EPOCH + Duration::from_millis(1_792_833_302_481);
        assert_eq!(timestamp(t), "2026-10-24 09:15:02.481");
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01 00:00:00.000");
    }
}
