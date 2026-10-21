//! What the Flutter app can call. A thin layer over `nudge_core::Node`:
//! plain types that cross into Dart, and one tokio runtime for the core.
//!
//! flutter_rust_bridge generates the Dart side from this file
//! (`flutter_rust_bridge_codegen generate` in `app/`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use flutter_rust_bridge::frb;
use nudge_core::{Direction, Fingerprint, Node, NodeConfig, NodeEvent, Peer};
use tokio::runtime::Runtime;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::frb_generated::StreamSink;

/// The core's runtime. Separate from flutter_rust_bridge's own, so the
/// core's background tasks never depend on how the bridge schedules calls.
fn runtime() -> &'static Runtime {
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
    flutter_rust_bridge::setup_default_user_utils();
}

/// A device as the app shows it.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// Full fingerprint (64 hex characters): the device's permanent ID.
    pub id: String,
    /// First 8 characters of the fingerprint, to show next to the name.
    pub short_id: String,
    pub name: String,
    /// "windows", "android", ... or empty if not known yet.
    pub platform: String,
    pub paired: bool,
    pub online: bool,
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
        }
    }
}

/// This device, running. Create one with `NudgeApp::start` and keep it for
/// the life of the app.
pub struct NudgeApp {
    node: Arc<Node>,
    events: Mutex<Option<UnboundedReceiver<NodeEvent>>>,
}

impl NudgeApp {
    /// Start Nudge: load or create this device's identity in `data_dir`,
    /// listen for connections and start finding nearby devices.
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
        let (node, events) = runtime()
            .spawn(Node::start(config))
            .await
            .context("Nudge runtime stopped")??;
        Ok(NudgeApp {
            node: Arc::new(node),
            events: Mutex::new(Some(events)),
        })
    }

    /// Deliver events to Dart. Call once; later calls do nothing.
    pub fn events(&self, sink: StreamSink<AppEvent>) {
        let Some(mut rx) = self.events.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return;
        };
        runtime().spawn(async move {
            while let Some(event) = rx.recv().await {
                if sink.add(event.into()).is_err() {
                    break; // Dart stopped listening
                }
            }
        });
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

    /// Send files to a paired device in the background. Returns one
    /// transfer ID per file; progress arrives as events.
    #[frb(sync)]
    pub fn send_files(&self, device_id: String, paths: Vec<String>) -> Result<Vec<i64>> {
        let _guard = runtime().enter();
        let paths = paths.into_iter().map(PathBuf::from).collect();
        let ids = self.node.send_files(parse_id(&device_id)?, paths);
        Ok(ids.into_iter().map(|id| id as i64).collect())
    }
}

fn parse_id(id: &str) -> Result<Fingerprint> {
    Ok(id.parse()?)
}
