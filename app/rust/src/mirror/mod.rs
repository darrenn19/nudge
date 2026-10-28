//! Windows: the mirror window. Shows another device's screen from a
//! real-time session: frames are decoded on the GPU and drawn as soon as the
//! display can take one, always the newest (see `spikes/mirror-latency`,
//! where this was measured).
//!
//! Two threads: a tokio task reads the session, and the window's own thread
//! decodes and draws (Win32 windows and Media Foundation want a thread of
//! their own). Closing the window ends the session; the session ending
//! closes the window.

mod video;

use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use nudge_core::realtime::{Event, Session};
use tokio::sync::mpsc as tokio_mpsc;

use crate::api::nudge::runtime;

/// A frame for the window, with its capture time in this session's clock.
struct Shown {
    data: Vec<u8>,
    captured: Option<u64>,
    /// When its last fragment arrived, in this session's clock.
    arrived: u64,
}

/// Opens a window showing `session` (the other device's screen).
pub(crate) fn show(mut session: Session, title: String) {
    let (frames, frames_rx) = mpsc::channel::<Shown>();
    // From the window: the decoder needs a keyframe (true), or the window
    // closed (false).
    let (back, mut back_rx) = tokio_mpsc::unbounded_channel::<bool>();
    let epoch = session.instant_of(0);

    runtime().spawn(async move {
        loop {
            tokio::select! {
                e = session.next_event() => match e {
                    Some(Event::Frame(f)) => {
                        let captured = session.to_local_us(f.capture_us);
                        if frames.send(Shown { data: f.data, captured, arrived: f.received_us }).is_err() {
                            break; // the window closed
                        }
                    }
                    Some(Event::PathChanged(p)) => {
                        log::info!("mirror path: {:?}", p.map(|p| (p.remote, p.kind.name(), p.rtt_us)));
                    }
                    Some(Event::KeyframeRequested { .. }) => {}
                    Some(Event::Closed) | None => break,
                },
                b = back_rx.recv() => match b {
                    Some(true) => session.need_keyframe(0),
                    Some(false) | None => break,
                },
            }
        }
        let s = session.stats();
        log::info!(
            "mirror ended: {} frames, {} recovered, {} given up, {} keyframe requests",
            s.received.frames,
            s.received.frames_recovered,
            s.received.given_up,
            s.received.key_requests,
        );
        session.close();
    });

    std::thread::Builder::new()
        .name("nudge-mirror".into())
        .spawn(move || {
            if let Err(e) = window_loop(&title, frames_rx, &back, epoch) {
                log::warn!("mirror window: {e}");
            }
            let _ = back.send(false);
        })
        .expect("couldn't start the mirror window");
}

fn window_loop(
    title: &str,
    frames: mpsc::Receiver<Shown>,
    back: &tokio_mpsc::UnboundedSender<bool>,
    epoch: Instant,
) -> windows::core::Result<()> {
    let now_us = || epoch.elapsed().as_micros() as u64;
    let mut video = video::Video::new(title)?;
    let mut stats = Latency::default();
    // The newest decoded picture, waiting for the display to be ready for it.
    let mut pending: Option<(video::Picture, Option<u64>, u64, u64)> = None;

    while video::pump() {
        let mut batch: Vec<Shown> = frames.try_iter().collect();
        if batch.is_empty() {
            let wait = if pending.is_some() { 1 } else { 2 };
            match frames.recv_timeout(Duration::from_millis(wait)) {
                Ok(f) => batch.push(f),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break, // the session ended
            }
            batch.extend(frames.try_iter());
        }
        // Every frame is decoded (later ones refer to it); only the newest
        // picture is shown.
        for f in batch {
            match video.decode(&f.data, false, f.captured.unwrap_or(0)) {
                Ok(Some(picture)) => {
                    stats.skipped += pending.is_some() as u64;
                    pending = Some((picture, f.captured, now_us(), f.arrived));
                }
                Ok(None) => {}
                Err(e) => {
                    log::warn!("mirror decode: {e}");
                    let _ = back.send(true);
                }
            }
        }
        // Draw when the display can take a frame right away; if it stops
        // signalling (window covered, starting up), draw anyway after about
        // a refresh, or the picture looks frozen.
        let overdue = pending.as_ref().is_some_and(|p| now_us().saturating_sub(p.2) > 20_000);
        if pending.is_some() && (video.ready_to_present() || overdue) {
            let (picture, captured, _, arrived) = pending.take().unwrap();
            if let Err(e) = video.present(&picture) {
                log::warn!("mirror display: {e}");
            }
            let shown = now_us();
            stats.shown(shown);
            if let Some(c) = captured {
                stats.add(shown.saturating_sub(c), arrived.saturating_sub(c), shown.saturating_sub(arrived));
            }
        }
        stats.report_every(Duration::from_secs(10), true);
    }
    stats.report("mirror session");
    video::close(&video);
    Ok(())
}

/// Capture-to-screen times, logged now and then: the same measure as the
/// spike's "TOTAL" (before the monitor's own delay).
#[derive(Default)]
struct Latency {
    window: Vec<u64>,
    all: Vec<u64>,
    /// The two halves, for the periodic report: capture to arrival (the
    /// other side's encoder and the network), and arrival to screen (here).
    there: Vec<u64>,
    here: Vec<u64>,
    since: Option<Instant>,
    /// Pictures decoded but never shown, since the last report.
    skipped: u64,
    /// Gaps between two shown frames longer than 25 ms (a visible stutter
    /// at 60 fps), since the last report, and when the last was shown.
    hitches: u64,
    last_shown: Option<u64>,
}

impl Latency {
    fn add(&mut self, total: u64, there: u64, here: u64) {
        self.window.push(total);
        self.all.push(total);
        self.there.push(there);
        self.here.push(here);
    }

    fn shown(&mut self, at: u64) {
        if self.last_shown.is_some_and(|t| at.saturating_sub(t) > 25_000) {
            self.hitches += 1;
        }
        self.last_shown = Some(at);
    }

    fn report_every(&mut self, every: Duration, _smooth: bool) {
        let since = *self.since.get_or_insert_with(Instant::now);
        if since.elapsed() >= every && !self.window.is_empty() {
            let fps = self.window.len() as f64 / since.elapsed().as_secs_f64();
            log::info!(
                "mirror: {fps:.0} fps, {}; to arrival {}; arrival to screen {}; {} hitches, {} skipped",
                summary(&mut self.window),
                brief(&mut self.there),
                brief(&mut self.here),
                self.hitches,
                self.skipped,
            );
            self.hitches = 0;
            self.skipped = 0;
            self.window.clear();
            self.there.clear();
            self.here.clear();
            self.since = Some(Instant::now());
        }
    }

    fn report(&mut self, what: &str) {
        if !self.all.is_empty() {
            log::info!("{what}: {} frames shown, {}", self.all.len(), summary(&mut self.all));
        }
    }
}

fn summary(v: &mut [u64]) -> String {
    v.sort_unstable();
    let p = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize] as f64 / 1000.0;
    format!(
        "capture to screen p50 {:.1} ms, p95 {:.1}, p99 {:.1}, max {:.1}",
        p(0.5),
        p(0.95),
        p(0.99),
        p(1.0)
    )
}

/// p50 / p95 / max, ms.
fn brief(v: &mut [u64]) -> String {
    if v.is_empty() {
        return "-".into();
    }
    v.sort_unstable();
    let p = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize] as f64 / 1000.0;
    format!("{:.1} / {:.1} / {:.1}", p(0.5), p(0.95), p(1.0))
}
