//! Android: sharing this phone's screen. The capture service (`MirrorService.kt`)
//! encodes the screen and hands each frame here; the core's real-time session
//! carries it to the PC.
//!
//! Sessions are held here by ID (the core's session ID), between the call
//! that starts or claims one and `mirrorClose`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jboolean, jint, jlong};
use nudge_core::node::MirrorRole;
use nudge_core::realtime::{Config, Event, OutFrame, Session};

use crate::api::nudge::{parse_id, runtime};

fn sessions() -> &'static Mutex<HashMap<u64, Session>> {
    static SESSIONS: OnceLock<Mutex<HashMap<u64, Session>>> = OnceLock::new();
    SESSIONS.get_or_init(Mutex::default)
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
            sessions().lock().unwrap_or_else(|e| e.into_inner()).insert(id as u64, s);
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
            sessions().lock().unwrap_or_else(|e| e.into_inner()).insert(id, session);
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
    if let Some(s) = session {
        s.close();
    }
}
