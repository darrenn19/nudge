//! Windows: the mirror window. Shows another device's screen from a
//! real-time session: frames are decoded on the GPU and drawn as soon as the
//! display can take one, always the newest (see `spikes/mirror-latency`,
//! where this was measured).
//!
//! Two threads: a tokio task reads the session, and the window's own thread
//! decodes and draws (Win32 windows and Media Foundation want a thread of
//! their own). Closing the window ends the session; the session ending
//! closes the window.

mod control;
mod touchpad;
mod video;

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::mpsc::{self, TryRecvError};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{CreateEventW, SetEvent};
use windows::Win32::UI::WindowsAndMessaging::{MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, QS_ALLINPUT};

use nudge_core::input::{self, Input};
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

/// Wakes the window's thread when a frame arrives, so it can wait for
/// frames and window messages (mouse, keys) at once: input goes out the
/// moment it happens, not at the next check.
struct Wake(HANDLE);

// SAFETY: an event handle can be signalled and waited on from any thread.
unsafe impl Send for Wake {}
unsafe impl Sync for Wake {}

impl Wake {
    fn new() -> windows::core::Result<Self> {
        // Auto-reset: one wake per signal.
        Ok(Self(unsafe { CreateEventW(None, false, false, None)? }))
    }

    fn signal(&self) {
        unsafe {
            let _ = SetEvent(self.0);
        }
    }

    /// Waits up to `ms` for a frame (a signal) or a window message.
    fn wait(&self, ms: u32) {
        unsafe {
            MsgWaitForMultipleObjectsEx(Some(&[self.0]), ms, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        }
    }
}

impl Drop for Wake {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Opens a window showing `session` (the other device's screen).
pub(crate) fn show(mut session: Session, title: String) {
    let wake = match Wake::new() {
        Ok(w) => Arc::new(w),
        Err(e) => {
            log::warn!("mirror window: {e}");
            return;
        }
    };
    let woken = wake.clone();
    let (frames, frames_rx) = mpsc::channel::<Shown>();
    // From the window: the decoder needs a keyframe (true), or the window
    // closed (false).
    let (back, mut back_rx) = tokio_mpsc::unbounded_channel::<bool>();
    // From the window: mouse and keyboard, for the phone (`control`).
    let (input_tx, mut input_rx) = tokio_mpsc::unbounded_channel::<Input>();
    let epoch = session.instant_of(0);

    runtime().spawn(async move {
        loop {
            tokio::select! {
                e = session.next_event() => match e {
                    // Other streams aren't video: the phone says whether it can be controlled.
                    Some(Event::Frame(f)) if f.stream != 0 => {
                        if let (input::STREAM, Some(Input::Status(st))) = (f.stream, Input::parse(&f.data)) {
                            log::info!("mirror: phone control {st:?}");
                            control::set_status(st);
                        }
                    }
                    Some(Event::Frame(f)) => {
                        let captured = session.to_local_us(f.capture_us);
                        if frames.send(Shown { data: f.data, captured, arrived: f.received_us }).is_err() {
                            break; // the window closed
                        }
                        woken.signal();
                    }
                    Some(Event::PathChanged(p)) => {
                        log::info!("mirror path: {:?}", p.map(|p| (p.remote, p.kind.name(), p.rtt_us)));
                    }
                    Some(Event::KeyframeRequested { .. }) => {}
                    Some(Event::Closed) | None => break,
                },
                Some(i) = input_rx.recv() => {
                    let _ = session.send_message(input::STREAM, i.encode());
                }
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
        woken.signal(); // the window notices the end
    });

    std::thread::Builder::new()
        .name("nudge-mirror".into())
        .spawn(move || {
            control::set_sender(input_tx);
            if let Err(e) = window_loop(&title, frames_rx, &back, epoch, &wake) {
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
    wake: &Wake,
) -> windows::core::Result<()> {
    let video = video::Video::new(title)?;
    PLAYER.set(Some(Player { video, frames, back: back.clone(), epoch, stats: Latency::default(), pending: None }));
    while video::pump() {
        control::tick();
        match PLAYER.with_borrow_mut(|p| p.as_mut().map(Player::step)) {
            Some(Step::Busy) => {}
            // Until a frame, a window message (input) or the next check.
            Some(Step::Idle { waiting }) => wake.wait(if waiting { 1 } else { 2 }),
            Some(Step::Ended) | None => break, // the session ended
        }
    }
    let started = Instant::now();
    if let Some(mut p) = PLAYER.take() {
        p.stats.report("mirror session");
        video::close(&p.video);
    }
    log::info!("mirror window: closed in {} ms", started.elapsed().as_millis());
    Ok(())
}

thread_local! {
    /// The window thread's player: the window loop runs it, and so does the
    /// window procedure while Windows runs a loop of its own (`step_now`).
    static PLAYER: RefCell<Option<Player>> = const { RefCell::new(None) };
}

/// While the window is being moved or resized Windows runs its own message
/// loop until the mouse is let go, and the window loop stands still: the
/// picture froze, then raced through everything that had piled up. A timer
/// in that loop calls this to keep the picture going.
pub(super) fn step_now() {
    control::tick();
    PLAYER.with(|p| {
        // Busy: called from inside a step (a window message it caused).
        if let Ok(mut p) = p.try_borrow_mut()
            && let Some(p) = p.as_mut()
        {
            let _ = p.step();
        }
    });
}

enum Step {
    /// Frames came in.
    Busy,
    /// Nothing came; `waiting`: a picture waits for the display.
    Idle { waiting: bool },
    Ended,
}

struct Player {
    video: video::Video,
    frames: mpsc::Receiver<Shown>,
    back: tokio_mpsc::UnboundedSender<bool>,
    epoch: Instant,
    stats: Latency,
    /// The newest decoded picture, waiting for the display to be ready for it.
    pending: Option<(video::Picture, Option<u64>, u64, u64)>,
}

impl Player {
    fn now_us(&self) -> u64 {
        self.epoch.elapsed().as_micros() as u64
    }

    /// Decodes what has arrived and shows the newest picture once the display can take it.
    fn step(&mut self) -> Step {
        let mut batch = Vec::new();
        loop {
            match self.frames.try_recv() {
                Ok(f) => batch.push(f),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Step::Ended,
            }
        }
        let busy = !batch.is_empty();
        // Every frame is decoded (later ones refer to it); only the newest
        // picture is shown.
        for f in batch {
            match self.video.decode(&f.data, false, f.captured.unwrap_or(0)) {
                Ok(Some(picture)) => {
                    self.stats.skipped += self.pending.is_some() as u64;
                    self.pending = Some((picture, f.captured, self.now_us(), f.arrived));
                }
                Ok(None) => {}
                Err(e) => {
                    log::warn!("mirror decode: {e}");
                    let _ = self.back.send(true);
                }
            }
        }
        // Draw when the display can take a frame right away; if it stops
        // signalling (window covered, starting up), draw anyway after about
        // a refresh, or the picture looks frozen; but never wait for it
        // (minimizing, maximizing: everything else would wait too).
        let overdue = self.pending.as_ref().is_some_and(|p| self.now_us().saturating_sub(p.2) > 20_000);
        if self.pending.is_some() && (self.video.ready_to_present() || overdue) {
            let (picture, captured, decoded, arrived) = self.pending.take().unwrap();
            match self.video.present(&picture) {
                Ok(true) => {
                    let shown = self.now_us();
                    self.stats.shown(shown);
                    if let Some(c) = captured {
                        self.stats.add(shown.saturating_sub(c), arrived.saturating_sub(c), shown.saturating_sub(arrived));
                    }
                }
                // Minimized, or the display still busy: kept for later (a newer one replaces it).
                Ok(false) => self.pending = Some((picture, captured, decoded, arrived)),
                Err(e) => log::warn!("mirror display: {e}"),
            }
        }
        self.stats.report_every(Duration::from_secs(10), true);
        if busy { Step::Busy } else { Step::Idle { waiting: self.pending.is_some() } }
    }
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
