//! What the Flutter app can call. A thin layer over `nudge_core::Node`:
//! plain types that cross into Dart, and one tokio runtime for the core.
//!
//! flutter_rust_bridge generates the Dart side from this file
//! (`flutter_rust_bridge_codegen generate` in `app/`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use flutter_rust_bridge::frb;
use nudge_core::{
    Cable, Direction, Fingerprint, HotspotInfo, LinkKind, Node, NodeConfig, NodeEvent, Outgoing,
    Peer, Receiving,
};
use nudge_core::node::MirrorRole;
use tokio::runtime::Runtime;

use crate::frb_generated::StreamSink;

/// The core's runtime. Separate from flutter_rust_bridge's own, so the
/// core's background tasks never depend on how the bridge schedules calls.
pub(crate) fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("nudge-core")
            .enable_all()
            .build()
            .expect("couldn't start the Nudge runtime")
    })
}

#[frb(init)]
pub fn init_app() {
    // Nudge's own log (see `nudge_core::logging`), not the console one
    // `setup_default_user_utils` would claim the logger for.
    nudge_core::logging::install();
    flutter_rust_bridge::setup_backtrace();
}

/// A device as the app shows it.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// Full fingerprint (64 hex characters): the device's permanent ID.
    pub id: String,
    /// First 8 characters of the fingerprint, to show next to the name.
    pub short_id: String,
    pub name: String,
    /// "windows", "android", ..., "browser" for someone using the web page,
    /// or empty if not known yet.
    pub platform: String,
    pub paired: bool,
    /// Nearby right now.
    pub online: bool,
    /// Worth sending to: nearby, or seen earlier this session (a phone with
    /// its screen off goes quiet but can still receive).
    pub reachable: bool,
    /// Files from it are accepted without asking.
    pub auto_accept: bool,
    /// It accepts files from this device without asking; `None` until it
    /// has said.
    pub accepts_from_us: Option<bool>,
    /// Clipboard text syncs with it, both ways.
    pub clipboard: bool,
    /// A cable between the two: "connected" (files can go over it),
    /// "tethering" (plugged in, but the phone's USB tethering is off), or
    /// empty.
    pub cable: String,
}

impl From<Peer> for DeviceInfo {
    fn from(p: Peer) -> Self {
        Self {
            id: p.fingerprint.to_string(),
            short_id: p.fingerprint.short(),
            name: p.device_name,
            platform: p.platform,
            paired: p.paired,
            online: p.online,
            reachable: p.reachable,
            auto_accept: p.auto_accept,
            accepts_from_us: p.accepts_from_us,
            clipboard: p.clipboard,
            cable: match p.cable {
                Cable::Connected => "connected",
                Cable::NeedsTethering => "tethering",
                Cable::None => "",
            }
            .into(),
        }
    }
}

/// Something the app should react to. See `nudge_core::NodeEvent`.
/// Sizes and IDs are i64 so they arrive in Dart as plain `int`s.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// Re-read `NudgeApp::devices`.
    DevicesChanged,
    Paired {
        device: DeviceInfo,
    },
    /// The pairing code on screen expired or was used.
    PairingClosed,
    TransferStarted {
        id: i64,
        device_id: String,
        incoming: bool,
        file_name: String,
        size: i64,
    },
    TransferProgress {
        id: i64,
        bytes: i64,
        size: i64,
        bytes_per_sec: f64,
    },
    TransferDone {
        id: i64,
        /// Saved file (incoming) or source file (outgoing).
        path: String,
        bytes: i64,
        seconds: f64,
        resumed_from: i64,
    },
    TransferFailed {
        id: i64,
        error: String,
    },
    /// Sharing with browsers stopped (see `NudgeApp::start_web_share`).
    WebShareStopped,
    /// A paired device wants to send files: ask, then `answer_offer`
    /// within a minute.
    OfferReceived {
        id: i64,
        device_id: String,
        /// The first file's name.
        file_name: String,
        count: i64,
        size: i64,
    },
    /// Stop showing that question (answered, or it timed out).
    OfferClosed {
        id: i64,
    },
    /// The receiver said yes to an outgoing transfer.
    TransferAccepted {
        id: i64,
    },
    /// The receiver has it and is asking its user: delivered, answer to come.
    TransferPending {
        id: i64,
    },
    /// Every byte is there, held until the receiver's user answers.
    /// Outgoing: sent, waiting for them. Incoming: here, waiting for the
    /// user; `TransferDone` follows the moment they accept.
    TransferDelivered {
        id: i64,
    },
    /// The network an outgoing transfer is using: "usb", "wired",
    /// "hotspot", "wifi" or "other". Can change mid-file.
    TransferRoute {
        id: i64,
        route: String,
    },
    /// Clipboard text from a paired device: put it on the clipboard.
    ClipboardReceived {
        device_id: String,
        text: String,
    },
    /// A copied image from a paired device, saved at `path`: put it on the
    /// clipboard.
    ClipboardImageReceived {
        device_id: String,
        path: String,
        mime: String,
    },
    /// Android: start the hotspot for a direct link, then `hotspot_ready`.
    HotspotWanted {
        id: i64,
        /// For screen mirroring: only a Wi-Fi Direct group, never a
        /// local-only hotspot.
        direct_only: bool,
    },
    /// Android: stop that hotspot.
    HotspotDone,
    /// Windows: join a phone's hotspot, then `joined`.
    JoinWanted {
        id: i64,
        ssid: String,
        passphrase: String,
        wpa3: bool,
    },
    /// Windows: go back to the usual network.
    LeaveWanted,
    /// A nearby device wants to pair without a QR code: show `code` and
    /// ask, then `answer_pair`.
    PairAsked {
        id: i64,
        device_id: String,
        device_name: String,
        code: String,
    },
    /// A paired device wants to see this device's screen: ask, then
    /// `answer_mirror` within a minute.
    MirrorAsked {
        id: i64,
        device_id: String,
    },
    /// A screen-mirroring session started. While `sharing`, show that this
    /// device's screen is being shared.
    MirrorStarted {
        id: i64,
        device_id: String,
        sharing: bool,
    },
    MirrorEnded {
        id: i64,
    },
}

impl From<NodeEvent> for AppEvent {
    fn from(e: NodeEvent) -> Self {
        match e {
            NodeEvent::PeersChanged => Self::DevicesChanged,
            NodeEvent::Paired { peer } => Self::Paired {
                device: peer.into(),
            },
            NodeEvent::PairingClosed => Self::PairingClosed,
            NodeEvent::TransferStarted {
                id,
                peer,
                direction,
                file_name,
                size,
            } => Self::TransferStarted {
                id: id as i64,
                device_id: peer.to_string(),
                incoming: direction == Direction::Incoming,
                file_name,
                size: size as i64,
            },
            NodeEvent::TransferProgress {
                id,
                bytes,
                size,
                bytes_per_sec,
            } => Self::TransferProgress {
                id: id as i64,
                bytes: bytes as i64,
                size: size as i64,
                bytes_per_sec,
            },
            NodeEvent::TransferDone { id, path, stats } => Self::TransferDone {
                id: id as i64,
                path: path.to_string_lossy().into_owned(),
                bytes: stats.bytes as i64,
                seconds: stats.elapsed.as_secs_f64(),
                resumed_from: stats.resumed_from as i64,
            },
            NodeEvent::TransferFailed { id, error } => Self::TransferFailed {
                id: id as i64,
                error,
            },
            NodeEvent::WebShareStopped => Self::WebShareStopped,
            NodeEvent::OfferReceived {
                id,
                peer,
                file_name,
                count,
                size,
            } => Self::OfferReceived {
                id: id as i64,
                device_id: peer.to_string(),
                file_name,
                count: count as i64,
                size: size as i64,
            },
            NodeEvent::OfferClosed { id } => Self::OfferClosed { id: id as i64 },
            NodeEvent::TransferAccepted { id } => Self::TransferAccepted { id: id as i64 },
            NodeEvent::TransferPending { id } => Self::TransferPending { id: id as i64 },
            NodeEvent::TransferDelivered { id } => Self::TransferDelivered { id: id as i64 },
            NodeEvent::TransferRoute { id, route } => Self::TransferRoute {
                id: id as i64,
                route: route_name(route).to_owned(),
            },
            NodeEvent::ClipboardReceived { peer, text } => Self::ClipboardReceived {
                device_id: peer.to_string(),
                text,
            },
            NodeEvent::ClipboardImageReceived { peer, path, mime } => {
                Self::ClipboardImageReceived {
                    device_id: peer.to_string(),
                    path: path.to_string_lossy().into_owned(),
                    mime,
                }
            }
            NodeEvent::HotspotWanted { id, direct_only } => Self::HotspotWanted {
                id: id as i64,
                direct_only,
            },
            NodeEvent::HotspotDone => Self::HotspotDone,
            NodeEvent::JoinWanted {
                id,
                ssid,
                passphrase,
                wpa3,
            } => Self::JoinWanted {
                id: id as i64,
                ssid,
                passphrase,
                wpa3,
            },
            NodeEvent::LeaveWanted => Self::LeaveWanted,
            NodeEvent::PairAsked {
                id,
                peer,
                device_name,
                code,
            } => Self::PairAsked {
                id: id as i64,
                device_id: peer.to_string(),
                device_name,
                code,
            },
            NodeEvent::MirrorAsked { id, peer } => Self::MirrorAsked {
                id: id as i64,
                device_id: peer.to_string(),
            },
            NodeEvent::MirrorStarted { id, peer, role } => Self::MirrorStarted {
                id: id as i64,
                device_id: peer.to_string(),
                sharing: role == MirrorRole::Sharing,
            },
            NodeEvent::MirrorEnded { id } => Self::MirrorEnded { id: id as i64 },
        }
    }
}

/// This device, running. Create one with `NudgeApp::start` and keep it for
/// the life of the app.
pub struct NudgeApp {
    node: Arc<Node>,
}

/// The one core in this process, shared by whoever started it and whoever
/// attaches later. On Android the background service starts it (see
/// `crate::android`) and the window attaches when it opens; on Windows the
/// window starts it.
pub(crate) struct Core {
    pub(crate) node: Arc<Node>,
    /// The window's event stream, while a window listens.
    dart: Mutex<Option<StreamSink<AppEvent>>>,
    /// The platform's own listener (Android's service), for good.
    native: Mutex<Option<NativeListener>>,
    /// Events from before anyone listened, handed to the first listener.
    early: Mutex<Vec<AppEvent>>,
}

pub(crate) type NativeListener = Box<dyn Fn(&AppEvent) + Send + Sync>;

static CORE: tokio::sync::OnceCell<Arc<Core>> = tokio::sync::OnceCell::const_new();

/// The core, if it's been started.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn running_core() -> Option<Arc<Core>> {
    CORE.get().cloned()
}

/// Start the core, or return the one already running (whatever `config`
/// says: the first start decides).
pub(crate) async fn start_core(config: NodeConfig) -> Result<Arc<Core>> {
    let core = CORE
        .get_or_try_init(|| async move {
            nudge_core::logging::to_file(&config.data_dir);
            log::info!("nudge {} starting on {}", nudge_core::VERSION, config.platform);
            let (node, mut rx) = runtime()
                .spawn(Node::start(config))
                .await
                .context("Nudge runtime stopped")??;
            let core = Arc::new(Core {
                node: Arc::new(node),
                dart: Mutex::new(None),
                native: Mutex::new(None),
                early: Mutex::new(Vec::new()),
            });
            let delivering = core.clone();
            runtime().spawn(async move {
                let mut timer = crate::timer::Fine::default();
                while let Some(event) = rx.recv().await {
                    match &event {
                        // Pacing the real-time channel needs ~1 ms timers;
                        // Windows gives ~16 ms unless asked.
                        NodeEvent::MirrorStarted { id, peer, role } => {
                            timer.hold(*id);
                            // The phone offered its screen: show it. (One this
                            // device asked for is shown by `mirror_device`.)
                            #[cfg(windows)]
                            if *role == MirrorRole::Viewing
                                && let Some(session) = delivering.node.take_mirror(*id)
                            {
                                crate::mirror::show(session, mirror_title(&delivering.node, peer));
                            }
                            #[cfg(not(windows))]
                            let _ = (peer, role);
                        }
                        NodeEvent::MirrorEnded { id } => timer.release(*id),
                        // Sharing this computer's screen isn't built yet:
                        // say no at once rather than leave it waiting.
                        #[cfg(not(target_os = "android"))]
                        NodeEvent::MirrorAsked { id, .. } => {
                            delivering.node.answer_mirror(*id, false)
                        }
                        _ => {}
                    }
                    delivering.deliver(event.into());
                }
            });
            anyhow::Ok(core)
        })
        .await?;
    Ok(core.clone())
}

impl Core {
    fn deliver(&self, event: AppEvent) {
        let native = self.native.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(listener) = native.as_ref() {
            listener(&event);
        }
        let mut dart = self.dart.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sink) = dart.as_ref()
            && sink.add(event.clone()).is_err()
        {
            *dart = None; // the window closed
        }
        if native.is_none() && dart.is_none() {
            let mut early = self.early.lock().unwrap_or_else(|e| e.into_inner());
            if early.len() < 1000 {
                early.push(event);
            }
        }
    }

    /// Events so far nobody heard.
    fn take_early(&self) -> Vec<AppEvent> {
        std::mem::take(&mut *self.early.lock().unwrap_or_else(|e| e.into_inner()))
    }

    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn set_native(&self, listener: NativeListener) {
        for event in self.take_early() {
            listener(&event);
        }
        *self.native.lock().unwrap_or_else(|e| e.into_inner()) = Some(listener);
    }
}

impl NudgeApp {
    /// Start Nudge: load or create this device's identity in `data_dir`,
    /// listen for connections and start finding nearby devices. If it's
    /// already running in this process (Android's background service
    /// started it), this attaches to it.
    pub async fn start(
        data_dir: String,
        download_dir: String,
        device_name: String,
        platform: String,
    ) -> Result<NudgeApp> {
        let config = NodeConfig::new(
            PathBuf::from(data_dir),
            PathBuf::from(download_dir),
            &device_name,
            &platform,
        );
        let core = start_core(config).await?;
        Ok(NudgeApp {
            node: core.node.clone(),
        })
    }

    /// Deliver events to this window. A later call (a new window) takes
    /// over from an earlier one.
    pub fn events(&self, sink: StreamSink<AppEvent>) {
        let Some(core) = CORE.get() else { return };
        for event in core.take_early() {
            let _ = sink.add(event);
        }
        *core.dart.lock().unwrap_or_else(|e| e.into_inner()) = Some(sink);
    }

    #[frb(sync)]
    pub fn device_id(&self) -> String {
        self.node.fingerprint().to_string()
    }

    #[frb(sync)]
    pub fn device_name(&self) -> String {
        self.node.device_name().to_owned()
    }

    #[frb(sync)]
    pub fn discovery_running(&self) -> bool {
        self.node.discovery_running()
    }

    /// Paired devices first, then unpaired nearby ones.
    #[frb(sync)]
    pub fn devices(&self) -> Vec<DeviceInfo> {
        self.node.peers().into_iter().map(Into::into).collect()
    }

    /// Ask nearby devices to announce themselves now.
    #[frb(sync)]
    pub fn refresh(&self) {
        self.node.refresh();
    }

    #[frb(sync)]
    pub fn download_dir(&self) -> String {
        self.node.download_dir().to_string_lossy().into_owned()
    }

    #[frb(sync)]
    pub fn set_download_dir(&self, dir: String) {
        self.node.set_download_dir(PathBuf::from(dir));
    }

    /// Show a pairing code: returns the text for the QR code. It works once
    /// and expires after five minutes (`AppEvent::PairingClosed`).
    #[frb(sync)]
    pub fn start_pairing(&self) -> String {
        let _guard = runtime().enter();
        self.node.start_pairing()
    }

    #[frb(sync)]
    pub fn cancel_pairing(&self) {
        self.node.cancel_pairing();
    }

    /// Pair with the device showing `code` (scanned or pasted).
    pub async fn pair(&self, code: String) -> Result<DeviceInfo> {
        let node = self.node.clone();
        let peer = runtime()
            .spawn(async move { node.pair(&code).await })
            .await
            .context("Nudge runtime stopped")??;
        Ok(peer.into())
    }

    #[frb(sync)]
    pub fn unpair(&self, device_id: String) -> Result<()> {
        Ok(self.node.unpair(&parse_id(&device_id)?)?)
    }

    /// Answer `AppEvent::OfferReceived`. With `always`, that device's files
    /// are accepted without asking from now on.
    #[frb(sync)]
    pub fn answer_offer(&self, id: i64, accept: bool, always: bool) -> Result<()> {
        // "Always" tells the other device, in the background.
        let _guard = runtime().enter();
        Ok(self.node.answer_offer(id as u64, accept, always)?)
    }

    /// Stop a transfer, and the rest of its send. The other device is told
    /// and deletes what arrived. False if it isn't running any more.
    #[frb(sync)]
    pub fn cancel_transfer(&self, id: i64) -> bool {
        let _guard = runtime().enter();
        self.node.cancel(id as u64)
    }

    /// The code both screens show while pairing with a nearby device.
    #[frb(sync)]
    pub fn pair_code(&self, device_id: String) -> Result<String> {
        Ok(self.node.pair_code(&parse_id(&device_id)?))
    }

    /// Pair with a nearby device without a QR code: it asks its user.
    /// Finishes when they answer.
    pub async fn pair_nearby(&self, device_id: String) -> Result<DeviceInfo> {
        let peer = parse_id(&device_id)?;
        let node = self.node.clone();
        let paired = runtime()
            .spawn(async move { node.pair_nearby(peer).await })
            .await
            .context("Nudge runtime stopped")??;
        Ok(paired.into())
    }

    /// Answer `AppEvent::PairAsked`.
    #[frb(sync)]
    pub fn answer_pair(&self, id: i64, accept: bool) {
        self.node.answer_pair(id as u64, accept);
    }

    /// Accept files from a paired device without asking, or ask again.
    #[frb(sync)]
    pub fn set_auto_accept(&self, device_id: String, on: bool) -> Result<()> {
        // Tells the other device, in the background.
        let _guard = runtime().enter();
        Ok(self.node.set_auto_accept(&parse_id(&device_id)?, on)?)
    }

    /// Open a connection to a paired, nearby device ahead of a likely send,
    /// so the send starts without a handshake. Safe to call often.
    #[frb(sync)]
    pub fn warm(&self, device_id: String) -> Result<()> {
        let peer = parse_id(&device_id)?;
        let _guard = runtime().enter();
        self.node.warm(peer);
        Ok(())
    }

    /// Share with browsers, for people without Nudge: returns the link for
    /// the QR code. Tabs that open it appear in `devices` with platform
    /// "browser" and can be sent to like any device. Already sharing: the
    /// same link. Stops by itself after ten minutes with nobody connected.
    pub async fn start_web_share(&self) -> Result<String> {
        let node = self.node.clone();
        Ok(runtime()
            .spawn(async move { node.start_web_share().await })
            .await
            .context("Nudge runtime stopped")??)
    }

    #[frb(sync)]
    pub fn stop_web_share(&self) {
        self.node.stop_web_share();
    }

    /// The link, while sharing with browsers.
    #[frb(sync)]
    pub fn web_share_url(&self) -> Option<String> {
        self.node.web_share_url()
    }

    /// Put text on a paired device's clipboard. No question on its side.
    pub async fn send_clipboard(&self, device_id: String, text: String) -> Result<()> {
        let peer = parse_id(&device_id)?;
        let node = self.node.clone();
        runtime()
            .spawn(async move { node.send_clipboard(peer, text).await })
            .await
            .context("Nudge runtime stopped")??;
        Ok(())
    }

    /// Put a copied image on a paired device's clipboard (`mime` like
    /// "image/png"). No question on its side.
    pub async fn send_clipboard_image(
        &self,
        device_id: String,
        mime: String,
        data: Vec<u8>,
    ) -> Result<()> {
        let peer = parse_id(&device_id)?;
        let node = self.node.clone();
        runtime()
            .spawn(async move { node.send_clipboard_image(peer, mime, data).await })
            .await
            .context("Nudge runtime stopped")??;
        Ok(())
    }

    /// Sync the clipboard with a paired device, both ways, or stop.
    #[frb(sync)]
    pub fn set_clipboard_sync(&self, device_id: String, on: bool) -> Result<()> {
        Ok(self.node.set_clipboard_sync(&parse_id(&device_id)?, on)?)
    }

    /// Whether big transfers may move to the phone's hotspot. USB cables
    /// and Ethernet are always used when faster.
    #[frb(sync)]
    pub fn direct_link(&self) -> bool {
        self.node.direct_link()
    }

    /// How incoming files are treated from now on: "usual" (auto-accepted
    /// devices send without a question), "ask" (ask about every send) or
    /// "refuse" (decline without asking). The app sets it as the device
    /// locks and unlocks.
    #[frb(sync)]
    pub fn set_receiving(&self, mode: String) {
        self.node.set_receiving(match mode.as_str() {
            "ask" => Receiving::Ask,
            "refuse" => Receiving::Refuse,
            _ => Receiving::Usual,
        });
    }

    /// Windows: see a paired phone's screen in a window of its own. The
    /// phone's user is asked first; returns once they've agreed (or with
    /// why not: "they declined", "the other device is locked"...).
    pub async fn mirror_device(&self, device_id: String) -> Result<()> {
        let peer = parse_id(&device_id)?;
        let node = self.node.clone();
        let (_, session) = runtime()
            .spawn(async move {
                node.start_mirror(peer, MirrorRole::Viewing, nudge_core::realtime::Config::default())
                    .await
            })
            .await
            .context("Nudge runtime stopped")??;
        #[cfg(windows)]
        crate::mirror::show(session, mirror_title(&self.node, &peer));
        #[cfg(not(windows))]
        drop(session);
        Ok(())
    }

    /// Answer `AppEvent::MirrorAsked`.
    #[frb(sync)]
    pub fn answer_mirror(&self, id: i64, accept: bool) {
        self.node.answer_mirror(id as u64, accept);
    }

    /// End a screen-mirroring session.
    #[frb(sync)]
    pub fn stop_mirror(&self, id: i64) {
        self.node.stop_mirror(id as u64);
    }

    /// Whether this device is locked: a locked device's screen is never
    /// shared, and sharing ends when it locks.
    #[frb(sync)]
    pub fn set_screen_locked(&self, locked: bool) {
        self.node.set_screen_locked(locked);
    }

    /// On a phone: plugged into a computer by a cable that carries data,
    /// with USB tethering off. Paired computers nearby are told.
    #[frb(sync)]
    pub fn set_cable_unused(&self, on: bool) {
        let _guard = runtime().enter();
        self.node.set_cable_unused(on);
    }

    #[frb(sync)]
    pub fn set_direct_link(&self, on: bool) -> Result<()> {
        Ok(self.node.set_direct_link(on)?)
    }

    /// Answer `AppEvent::HotspotWanted`: the hotspot's name and passphrase,
    /// or no `ssid` if it couldn't start.
    #[frb(sync)]
    pub fn hotspot_ready(&self, id: i64, ssid: Option<String>, passphrase: String, wpa3: bool) {
        let info = ssid.map(|ssid| HotspotInfo {
            ssid,
            passphrase,
            wpa3,
            mhz: 0,
        });
        self.node.hotspot_ready(id as u64, info);
    }

    /// Answer `AppEvent::JoinWanted`.
    #[frb(sync)]
    pub fn joined(&self, id: i64, ok: bool) {
        self.node.joined(id as u64, ok);
    }

    /// Send files to a paired device in the background. Returns one
    /// transfer ID per file; progress arrives as events.
    #[frb(sync)]
    pub fn send_files(&self, device_id: String, paths: Vec<String>) -> Result<Vec<i64>> {
        let _guard = runtime().enter();
        let paths = paths.into_iter().map(PathBuf::from).collect();
        let ids = self.node.send_files(parse_id(&device_id)?, paths);
        Ok(ids.into_iter().map(|id| id as i64).collect())
    }

    /// Send files the platform has already opened (Android shares arrive
    /// as open descriptors, not paths). Takes ownership of each descriptor
    /// and closes it when its transfer ends. Unix only.
    #[frb(sync)]
    pub fn send_open_files(&self, device_id: String, files: Vec<OpenFile>) -> Result<Vec<i64>> {
        let peer = parse_id(&device_id)?;
        let items = files
            .into_iter()
            .map(OpenFile::into_outgoing)
            .collect::<Result<Vec<_>>>()?;
        let _guard = runtime().enter();
        let ids = self.node.send(peer, items);
        Ok(ids.into_iter().map(|id| id as i64).collect())
    }
}

/// A file the platform opened for us: its descriptor and display name. A
/// name with folders ("Trip/a.jpg") arrives in those folders; a name ending
/// in '/' with no descriptor (-1) is an empty folder.
#[derive(Debug, Clone)]
pub struct OpenFile {
    pub fd: i32,
    pub name: String,
}

impl OpenFile {
    #[cfg(unix)]
    fn into_outgoing(self) -> Result<Outgoing> {
        use std::os::fd::{FromRawFd, OwnedFd};
        // An empty folder from a picked folder ("Trip/empty/"): no file.
        if self.fd < 0 && self.name.ends_with('/') {
            return Ok(Outgoing::EmptyFolder { name: self.name });
        }
        anyhow::ensure!(self.fd >= 0, "invalid file descriptor for {}", self.name);
        // SAFETY: the caller hands over an open descriptor it no longer uses.
        let fd = unsafe { OwnedFd::from_raw_fd(self.fd) };
        Ok(Outgoing::Open {
            file: fd.into(),
            name: self.name,
        })
    }

    #[cfg(not(unix))]
    fn into_outgoing(self) -> Result<Outgoing> {
        anyhow::bail!("sending open files isn't supported on this platform")
    }
}

fn route_name(kind: LinkKind) -> &'static str {
    kind.name()
}

pub(crate) fn parse_id(id: &str) -> Result<Fingerprint> {
    Ok(id.parse()?)
}

/// The mirror window's title: the device's name.
#[cfg(windows)]
fn mirror_title(node: &Node, peer: &Fingerprint) -> String {
    node.peers()
        .into_iter()
        .find(|p| p.fingerprint == *peer)
        .map_or_else(|| "Nudge".into(), |p| format!("{} - Nudge", p.device_name))
}
