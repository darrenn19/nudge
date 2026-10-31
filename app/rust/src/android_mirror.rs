//! Android: sharing this phone's screen. The capture service (`MirrorService.kt`)
//! encodes the screen and hands each frame here; the core's real-time session
//! carries it to the PC.
//!
//! Sessions are held here by ID (the core's session ID), between the call
//! that starts or claims one and `mirrorClose`.
//!
//! Remote control: the PC's input messages (`nudge_core::input`) come in on
//! the session's input stream; `ControlService.kt` reads them with
//! `mirrorInput` on a thread of its own and acts on them.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jboolean, jbyteArray, jint, jlong};
use nudge_core::input::{self, ControlStatus, Input};
use nudge_core::node::MirrorRole;
use nudge_core::realtime::{Config, Event, Frame, OutFrame, RateControl, Session};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::api::nudge::{parse_id, runtime};

fn sessions() -> &'static Mutex<HashMap<u64, Session>> {
    static SESSIONS: OnceLock<Mutex<HashMap<u64, Session>>> = OnceLock::new();
    SESSIONS.get_or_init(Mutex::default)
}

type Inputs = std::sync::Arc<tokio::sync::Mutex<UnboundedReceiver<Frame>>>;

/// Each session's incoming input messages.
fn inputs() -> &'static Mutex<HashMap<u64, Inputs>> {
    static INPUTS: OnceLock<Mutex<HashMap<u64, Inputs>>> = OnceLock::new();
    INPUTS.get_or_init(Mutex::default)
}

/// Holds a session that this phone shares, with its input stream.
fn hold(id: u64, s: Session) {
    let rx = s.subscribe(input::STREAM);
    inputs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, std::sync::Arc::new(tokio::sync::Mutex::new(rx)));
    sessions().lock().unwrap_or_else(|e| e.into_inner()).insert(id, s);
}

fn core() -> Option<std::sync::Arc<crate::api::nudge::Core>> {
    crate::api::nudge::running_core()
}

/// Claim the session behind a "mirrorStarted" event (this phone was asked,
/// and its user allowed it). False if it's gone.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_darrenn19_nudge_Core_mirrorTake<'l>(
    _: JNIEnv<'l>,
    _: JClass<'l>,
    id: jlong,
) -> jboolean {
    let Some(c) = core() else { return 0 };
    match c.node.take_mirror(id as u64) {
        Some(s) => {
            hold(id as u64, s);
            1
        }
        None => 0,
    }
}

/// Show this phone's screen to a paired device (its user started it here).
/// Blocks until the other side answers. The session ID, or -1.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_darrenn19_nudge_Core_mirrorStart<'l>(
    mut env: JNIEnv<'l>,
    _: JClass<'l>,
    device_id: JString<'l>,
) -> jlong {
    let Some(c) = core() else { return -1 };
    let id: String = env.get_string(&device_id).map(Into::into).unwrap_or_default();
    let Ok(peer) = parse_id(&id) else { return -1 };
    let node = c.node.clone();
    let started = runtime().block_on(async move {
        runtime()
            .spawn(async move { node.start_mirror(peer, MirrorRole::Sharing, Config::default()).await })
            .await
    });
    match started {
        Ok(Ok((id, session))) => {
            hold(id, session);
            id as jlong
        }
        Ok(Err(e)) => {
            log::warn!("mirroring to {id}: {e}");
            -1
        }
        Err(_) => -1,
    }
}

/// Send one encoded frame. `age_us`: how long ago it was captured.
/// Returns 0, 1 if the other side wants a keyframe, or -1 once the session
/// has ended.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_darrenn19_nudge_Core_mirrorSend<'l>(
    env: JNIEnv<'l>,
    _: JClass<'l>,
    id: jlong,
    data: JByteArray<'l>,
    key: jboolean,
    intra_refresh: jboolean,
    age_us: jlong,
) -> jint {
    let Ok(bytes) = env.convert_byte_array(&data) else { return -1 };
    let mut all = sessions().lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = all.get_mut(&(id as u64)) else { return -1 };
    let mut wants_key = false;
    let mut ended = false;
    while let Some(e) = s.try_event() {
        match e {
            Event::KeyframeRequested { .. } => wants_key = true,
            Event::Closed => ended = true,
            _ => {}
        }
    }
    let frame = OutFrame {
        stream: 0,
        data: bytes,
        key: key != 0,
        intra_refresh: intra_refresh != 0,
        capture_us: s.now_us().saturating_sub(age_us.max(0) as u64),
    };
    if ended || s.send(frame).is_err() {
        all.remove(&(id as u64));
        return -1;
    }
    jint::from(wants_key)
}

/// Whether the session is still running (for a still screen, when no
/// frames are being sent), and whether a keyframe is wanted: 0 running,
/// 1 keyframe wanted, -1 ended.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_darrenn19_nudge_Core_mirrorPoll<'l>(
    _: JNIEnv<'l>,
    _: JClass<'l>,
    id: jlong,
) -> jint {
    let mut all = sessions().lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = all.get_mut(&(id as u64)) else { return -1 };
    let mut out = 0;
    while let Some(e) = s.try_event() {
        match e {
            Event::KeyframeRequested { .. } => out = 1,
            Event::Closed => {
                all.remove(&(id as u64));
                return -1;
            }
            _ => {}
        }
    }
    out
}

/// End a session (dropping it tells the other side).
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_darrenn19_nudge_Core_mirrorClose<'l>(
    _: JNIEnv<'l>,
    _: JClass<'l>,
    id: jlong,
) {
    let session = sessions().lock().unwrap_or_else(|e| e.into_inner()).remove(&(id as u64));
    inputs().lock().unwrap_or_else(|e| e.into_inner()).remove(&(id as u64));
    rates().lock().unwrap_or_else(|e| e.into_inner()).remove(&(id as u64));
    if let Some(s) = session {
        s.close();
    }
}

/// The next input message from the PC (`nudge_core::input` wire format,
/// already checked), waiting up to `timeout_ms`. Null if none came in
/// time; an empty array once the session has ended.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_darrenn19_nudge_Core_mirrorInput<'l>(
    env: JNIEnv<'l>,
    _: JClass<'l>,
    id: jlong,
    timeout_ms: jint,
) -> jbyteArray {
    let rx = inputs().lock().unwrap_or_else(|e| e.into_inner()).get(&(id as u64)).cloned();
    let got: Option<Vec<u8>> = match rx {
        None => Some(Vec::new()),
        Some(rx) => runtime().block_on(async move {
            let mut rx = rx.lock().await;
            let wait = std::time::Duration::from_millis(timeout_ms.max(0) as u64);
            let deadline = tokio::time::Instant::now() + wait;
            loop {
                match tokio::time::timeout_at(deadline, rx.recv()).await {
                    Err(_) => return None,
                    Ok(None) => return Some(Vec::new()),
                    // Only what parses goes up; the rest is from a newer PC.
                    Ok(Some(f)) => match Input::parse(&f.data) {
                        Some(Input::Status(_)) | None => continue,
                        Some(_) => {
                            measure(id as u64, &f);
                            return Some(f.data);
                        }
                    },
                }
            }
        }),
    };
    match got {
        None => std::ptr::null_mut(),
        Some(bytes) => env.byte_array_from_slice(&bytes).map(|a| a.into_raw()).unwrap_or(std::ptr::null_mut()),
    }
}

/// Input delays, logged every 10 s: from the PC sending a message to it
/// arriving here, and to it being handed to `ControlService`.
#[derive(Default)]
struct InputDelays {
    arrived: Vec<u64>,
    handed: Vec<u64>,
    since: Option<std::time::Instant>,
}

fn measure(id: u64, f: &Frame) {
    static DELAYS: OnceLock<Mutex<InputDelays>> = OnceLock::new();
    let (sent, now) = {
        let all = sessions().lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = all.get(&id) else { return };
        let Some(sent) = s.to_local_us(f.capture_us) else { return };
        (sent, s.now_us())
    };
    let mut d = DELAYS.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner());
    d.arrived.push(f.received_us.saturating_sub(sent));
    d.handed.push(now.saturating_sub(sent));
    let since = *d.since.get_or_insert_with(std::time::Instant::now);
    if since.elapsed() >= std::time::Duration::from_secs(10) {
        fn brief(v: &mut [u64]) -> String {
            v.sort_unstable();
            let p = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize] as f64 / 1000.0;
            format!("p50 {:.1} ms, p95 {:.1}, max {:.1}", p(0.5), p(0.95), p(1.0))
        }
        let n = d.arrived.len();
        let (arrived, handed) = (brief(&mut d.arrived), brief(&mut d.handed));
        log::info!("mirror input: {n} messages; PC to here {arrived}; to the control service {handed}");
        *d = InputDelays::default();
    }
}

/// Each session's bitrate controller (see `nudge_core::realtime::RateControl`).
fn rates() -> &'static Mutex<HashMap<u64, RateControl>> {
    static RATES: OnceLock<Mutex<HashMap<u64, RateControl>>> = OnceLock::new();
    RATES.get_or_init(Mutex::default)
}

/// The encoder's bitrate for the network now, kbit/s: a new value when it
/// should change, else 0. Call every ~200 ms. `max_kbps`: the top rate
/// (where it starts).
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_darrenn19_nudge_Core_mirrorRate<'l>(
    _: JNIEnv<'l>,
    _: JClass<'l>,
    id: jlong,
    max_kbps: jint,
) -> jint {
    let (now, stats) = {
        let all = sessions().lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = all.get(&(id as u64)) else { return 0 };
        (s.now_us(), s.stats())
    };
    let mut rates = rates().lock().unwrap_or_else(|e| e.into_inner());
    let max = max_kbps.max(1000) as u32;
    let rc = rates.entry(id as u64).or_insert_with(|| RateControl::new(max, (max / 8).max(2_000)));
    match rc.update(now, stats.rtt_latest_us, stats.clock_rtt_us, stats.sent.nacks_received) {
        Some(c) => {
            log::info!(
                "mirror rate: {:.1} Mbit/s (queuing {} ms, path {})",
                f64::from(c.kbps) / 1000.0,
                c.queuing_us / 1000,
                stats.path.map_or("none", |p| p.kind.name()),
            );
            c.kbps as jint
        }
        None => 0,
    }
}

/// Tell the PC whether this phone can be controlled: 0 unavailable,
/// 1 ready, 2 needs the Accessibility switch turned on.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_darrenn19_nudge_Core_mirrorControlStatus<'l>(
    _: JNIEnv<'l>,
    _: JClass<'l>,
    id: jlong,
    status: jint,
) {
    let status = match status {
        1 => ControlStatus::Ready,
        2 => ControlStatus::NeedsSetup,
        _ => ControlStatus::Unavailable,
    };
    let all = sessions().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = all.get(&(id as u64)) {
        let _ = s.send_message(input::STREAM, Input::Status(status).encode());
    }
}
