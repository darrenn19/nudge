//! Android: the background service's way in (JNI, called from `Core.kt`).
//!
//! The service starts the core when the process starts, before any window,
//! and hears every event so it can show notifications, answer questions
//! from them, put clipboard text in place and run the hotspot. The window,
//! when open, attaches to the same core through flutter_rust_bridge.

use std::path::PathBuf;
use std::sync::Arc;

use jni::JNIEnv;
use jni::JavaVM;
use jni::objects::{
    GlobalRef, JByteArray, JClass, JIntArray, JObject, JObjectArray, JString, JValue,
};
use jni::sys::{jboolean, jint, jlong, jstring};
use nudge_core::{HotspotInfo, NodeConfig, Outgoing, Receiving};
use serde_json::{Value, json};

use crate::api::nudge::{AppEvent, Core, DeviceInfo, parse_id, runtime, start_core};

/// The running core, once `start` has been called.
fn core() -> Option<Arc<Core>> {
    crate::api::nudge::running_core()
}

fn string(env: &mut JNIEnv, s: &JString) -> String {
    env.get_string(s).map(Into::into).unwrap_or_default()
}

fn optional_string(env: &mut JNIEnv, s: &JString) -> Option<String> {
    if s.is_null() {
        None
    } else {
        Some(string(env, s))
    }
}

fn new_string(env: &mut JNIEnv, s: &str) -> jstring {
    env.new_string(s)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// Start the core (or attach to it) and send every event to `listener`'s
/// `onEvent(String)`, as JSON. Returns null, or what went wrong.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_darrenn19_nudge_Core_start<'l>(
    mut env: JNIEnv<'l>,
    _: JClass<'l>,
    data_dir: JString<'l>,
    download_dir: JString<'l>,
    device_name: JString<'l>,
    listener: JObject<'l>,
) -> jstring {
    let config = NodeConfig::new(
        PathBuf::from(string(&mut env, &data_dir)),
        PathBuf::from(string(&mut env, &download_dir)),
        &string(&mut env, &device_name),
        "android",
    );
    let (vm, listener) = match (env.get_java_vm(), env.new_global_ref(listener)) {
        (Ok(vm), Ok(listener)) => (vm, listener),
        _ => return new_string(&mut env, "couldn't hold the listener"),
    };
    match runtime().block_on(start_core(config)) {
        Ok(core) => {
            core.set_native(Box::new(move |event| tell(&vm, &listener, event)));
            std::ptr::null_mut()
        }
        Err(e) => new_string(&mut env, &format!("{e:#}")),
    }
}

fn tell(vm: &JavaVM, listener: &GlobalRef, event: &AppEvent) {
    let Some(json) = event_json(event) else {
        return;
    };
    // Core threads stay attached for their whole life.
    let Ok(mut env) = vm.attach_current_thread_as_daemon() else {
        return;
    };
    let Ok(text) = env.new_string(json.to_string()) else {
        return;
    };
    let _ = env.call_method(
        listener,
        "onEvent",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&text)],
    );
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    let _ = env.delete_local_ref(text);
}

/// The events the service acts on, as JSON with a "type".
fn event_json(event: &AppEvent) -> Option<Value> {
    Some(match event {
        AppEvent::DevicesChanged | AppEvent::Paired { .. } => json!({"type": "devices"}),
        AppEvent::TransferStarted {
            id,
            device_id,
            incoming,
            file_name,
            size,
        } => json!({"type": "started", "id": id, "device": device_id, "incoming": incoming,
                    "name": file_name, "size": size}),
        AppEvent::TransferProgress {
            id,
            bytes,
            bytes_per_sec,
            ..
        } => json!({"type": "progress", "id": id, "bytes": bytes, "speed": bytes_per_sec}),
        AppEvent::TransferDone { id, path, .. } => json!({"type": "done", "id": id, "path": path}),
        AppEvent::TransferFailed { id, error } => {
            json!({"type": "failed", "id": id, "error": error})
        }
        AppEvent::TransferDelivered { id } => json!({"type": "delivered", "id": id}),
        AppEvent::TransferPending { id } => json!({"type": "pending", "id": id}),
        AppEvent::TransferAccepted { id } => json!({"type": "accepted", "id": id}),
        AppEvent::OfferReceived {
            id,
            device_id,
            file_name,
            count,
            size,
        } => json!({"type": "offer", "id": id, "device": device_id, "name": file_name,
                    "count": count, "size": size}),
        AppEvent::OfferClosed { id } => json!({"type": "offerClosed", "id": id}),
        AppEvent::PairAsked {
            id,
            device_name,
            code,
            ..
        } => json!({"type": "pairAsked", "id": id, "name": device_name, "code": code}),
        AppEvent::ClipboardReceived { device_id, text } => {
            json!({"type": "clipboard", "device": device_id, "text": text})
        }
        AppEvent::ClipboardImageReceived {
            device_id,
            path,
            mime,
        } => json!({"type": "clipboardImage", "device": device_id, "path": path, "mime": mime}),
        AppEvent::HotspotWanted { id } => json!({"type": "hotspotWanted", "id": id}),
        AppEvent::HotspotDone => json!({"type": "hotspotDone"}),
        _ => return None,
    })
}

fn device_json(d: &DeviceInfo) -> Value {
    json!({"id": d.id, "name": d.name, "platform": d.platform, "paired": d.paired, "online": d.online,
           "reachable": d.reachable, "clipboard": d.clipboard})
}

/// Every known device, as a JSON array.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_darrenn19_nudge_Core_devices<'l>(
    mut env: JNIEnv<'l>,
    _: JClass<'l>,
) -> jstring {
    let devices: Vec<Value> = core()
        .map(|c| {
            c.node
                .peers()
                .into_iter()
                .map(DeviceInfo::from)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
        .iter()
        .map(device_json)
        .collect();
    new_string(&mut env, &Value::Array(devices).to_string())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_darrenn19_nudge_Core_answerOffer<'l>(
    _: JNIEnv<'l>,
    _: JClass<'l>,
    id: jlong,
    accept: jboolean,
) {
    if let Some(c) = core() {
        let _guard = runtime().enter();
        let _ = c.node.answer_offer(id as u64, accept != 0, false);
    }
}

/// Stop a transfer (the Cancel on the progress notification).
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_darrenn19_nudge_Core_cancel<'l>(
    _: JNIEnv<'l>,
    _: JClass<'l>,
    id: jlong,
) {
    if let Some(c) = core() {
        let _guard = runtime().enter();
        c.node.cancel(id as u64);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_darrenn19_nudge_Core_answerPair<'l>(
    _: JNIEnv<'l>,
    _: JClass<'l>,
    id: jlong,
    accept: jboolean,
) {
    if let Some(c) = core() {
        let _guard = runtime().enter();
        c.node.answer_pair(id as u64, accept != 0);
    }
}

/// "usual", "ask" or "refuse": see `NudgeApp::set_receiving`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_darrenn19_nudge_Core_setReceiving<'l>(
    mut env: JNIEnv<'l>,
    _: JClass<'l>,
    mode: JString<'l>,
) {
    let mode = string(&mut env, &mode);
    if let Some(c) = core() {
        c.node.set_receiving(match mode.as_str() {
            "ask" => Receiving::Ask,
            "refuse" => Receiving::Refuse,
            _ => Receiving::Usual,
        });
    }
}

/// Plugged into a computer with USB tethering off: see
/// `NudgeApp::set_cable_unused`. From the background, so paired computers
/// hear it with the window closed too.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_darrenn19_nudge_Core_setCableUnused<'l>(
    _: JNIEnv<'l>,
    _: JClass<'l>,
    on: jboolean,
) {
    if let Some(c) = core() {
        let _guard = runtime().enter();
        c.node.set_cable_unused(on != 0);
    }
}

/// Answer a "hotspotWanted" event; a null `ssid` means it didn't start.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_darrenn19_nudge_Core_hotspotReady<'l>(
    mut env: JNIEnv<'l>,
    _: JClass<'l>,
    id: jlong,
    ssid: JString<'l>,
    passphrase: JString<'l>,
    wpa3: jboolean,
    mhz: jint,
) {
    let ssid = optional_string(&mut env, &ssid);
    let passphrase = string(&mut env, &passphrase);
    if let Some(c) = core() {
        let info = ssid.map(|ssid| HotspotInfo {
            ssid,
            passphrase,
            wpa3: wpa3 != 0,
            mhz: mhz.max(0) as u32,
        });
        let _guard = runtime().enter();
        c.node.hotspot_ready(id as u64, info);
    }
}

/// Open a connection to a device ahead of a send.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_darrenn19_nudge_Core_warm<'l>(
    mut env: JNIEnv<'l>,
    _: JClass<'l>,
    device: JString<'l>,
) {
    let device = string(&mut env, &device);
    if let (Some(c), Ok(peer)) = (core(), parse_id(&device)) {
        let _guard = runtime().enter();
        c.node.warm(peer);
    }
}

/// Send open descriptors (with their names) to a device, taking ownership
/// of them. Returns whether it started; if not, they're still the caller's.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_darrenn19_nudge_Core_sendOpenFiles<'l>(
    mut env: JNIEnv<'l>,
    _: JClass<'l>,
    device: JString<'l>,
    fds: JIntArray<'l>,
    names: JObjectArray<'l>,
) -> jboolean {
    use std::os::fd::{FromRawFd, OwnedFd};
    let device = string(&mut env, &device);
    // Checked first: on failure the descriptors stay the caller's.
    let (Some(c), Ok(peer)) = (core(), parse_id(&device)) else {
        return 0;
    };
    let count = env.get_array_length(&fds).unwrap_or(0) as usize;
    let mut raw = vec![0i32; count];
    if env.get_int_array_region(&fds, 0, &mut raw).is_err() {
        return 0;
    }
    let mut items = Vec::with_capacity(count);
    for (i, fd) in raw.into_iter().enumerate() {
        let name = env
            .get_object_array_element(&names, i as i32)
            .map(JString::from)
            .map(|s| string(&mut env, &s))
            .unwrap_or_else(|_| "file".into());
        if fd < 0 {
            continue;
        }
        // SAFETY: Kotlin detached each descriptor and hands it over here.
        #[allow(unsafe_code)]
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        items.push(Outgoing::Open {
            file: fd.into(),
            name,
        });
    }
    let _guard = runtime().enter();
    c.node.send(peer, items);
    1
}

/// What goes onto other devices' clipboards.
#[derive(Clone)]
enum ClipItem {
    Text(String),
    Image { mime: String, data: Vec<u8> },
}

/// Put `item` on a device's clipboard, or (with no `device`) on every paired
/// device that syncs it and is around. Returns the names of the devices it
/// reached. Blocks.
fn send_clip(device: Option<String>, item: ClipItem) -> Vec<String> {
    let Some(c) = core() else {
        return vec![];
    };
    let targets: Vec<DeviceInfo> = c
        .node
        .peers()
        .into_iter()
        .map(DeviceInfo::from)
        .filter(|d| match &device {
            Some(id) => d.paired && &d.id == id,
            None => d.paired && d.clipboard && d.reachable,
        })
        .collect();
    runtime().block_on(async {
        let mut reached = vec![];
        let mut sends = tokio::task::JoinSet::new();
        for d in targets {
            let (node, item) = (c.node.clone(), item.clone());
            sends.spawn(async move {
                let peer = parse_id(&d.id).ok()?;
                match item {
                    ClipItem::Text(text) => node.send_clipboard(peer, text).await.ok()?,
                    ClipItem::Image { mime, data } => {
                        node.send_clipboard_image(peer, mime, data).await.ok()?
                    }
                }
                Some(d.name)
            });
        }
        while let Some(done) = sends.join_next().await {
            if let Ok(Some(name)) = done {
                reached.push(name);
            }
        }
        reached
    })
}

/// Put text on a device's clipboard, or (with a null `device`) on every
/// paired device that syncs it and is around. Returns the names of the
/// devices it reached, as a JSON array. Blocks: call off the main thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_darrenn19_nudge_Core_sendClipboard<'l>(
    mut env: JNIEnv<'l>,
    _: JClass<'l>,
    device: JString<'l>,
    text: JString<'l>,
) -> jstring {
    let device = optional_string(&mut env, &device);
    let text = string(&mut env, &text);
    let reached = send_clip(device, ClipItem::Text(text));
    new_string(&mut env, &json!(reached).to_string())
}

/// The same for a copied image (`mime` like "image/png").
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_darrenn19_nudge_Core_sendClipboardImage<'l>(
    mut env: JNIEnv<'l>,
    _: JClass<'l>,
    device: JString<'l>,
    mime: JString<'l>,
    data: JByteArray<'l>,
) -> jstring {
    let device = optional_string(&mut env, &device);
    let mime = string(&mut env, &mime);
    let data = env.convert_byte_array(&data).unwrap_or_default();
    let reached = if data.is_empty() {
        vec![]
    } else {
        send_clip(device, ClipItem::Image { mime, data })
    };
    new_string(&mut env, &json!(reached).to_string())
}
