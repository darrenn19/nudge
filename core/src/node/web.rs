//! Sharing with a browser, for people without Nudge.
//!
//! While on, the node serves a small web page on the local network. Someone
//! who scans the QR code opens it, and while the tab stays open they appear
//! as a device. Files sent to them wait for them to tap Accept, then
//! download straight from this device. They can send files back from the
//! same page (the user here accepts or declines each batch), and clipboard
//! text both ways. A tab is temporary: it's gone soon after the page closes,
//! and is never paired.
//!
//! Plain HTTP: a browser can't trust Nudge's self-signed certificate
//! without a scary warning. The only protection is the unguessable secret
//! in the link, so the page is meant for a trusted network.
//! See docs/PROTOCOL.md ("Browser sharing").

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use super::{BatchKey, Direction, NodeEvent, Outgoing, Shared, Verdict, local_addrs};
use crate::link::{self, LinkKind};
use crate::archive::{Archive, Entry};
use crate::identity::Fingerprint;
use crate::pairing::random_bytes;
use crate::resume::with_suffix;
use crate::transfer::{Cancel, CancelledBy, Progress, TransferStats, sanitize_file_name, unique_path};
use crate::{Error, Result};

/// Port for the page; any free port if it's taken.
pub const WEB_PORT: u16 = 47_802;

/// Sharing stops after this long with no browser connected.
const IDLE_LIMIT: Duration = Duration::from_secs(10 * 60);

/// A tab that loses its connection (phone locked, tab in the background)
/// keeps its place this long.
const GRACE: Duration = Duration::from_secs(30);

/// How long an offer waits for Accept or Decline.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Keeps the event stream alive, and notices when a tab has gone. It's a
/// real event, not a comment, so the page sees it: a page that hears
/// nothing for a few of these knows its connection is dead.
const PING_INTERVAL: Duration = Duration::from_secs(10);

/// After sharing stops, the old link keeps saying so this long, so a tab
/// that was away (phone locked, out of range) learns that sharing ended
/// instead of looking for this device forever.
const ENDED_LINGER: Duration = Duration::from_secs(15 * 60);

/// A download the browser takes nothing of for this long has stalled.
const STALL_LIMIT: Duration = Duration::from_secs(45);

/// A download that broke waits this long for the browser to ask again
/// (browsers retry a broken download by starting it over) before failing.
const RETRY_WINDOW: Duration = Duration::from_secs(10);

/// How often a tab hears how far its download has got.
const PAGE_PROGRESS: Duration = Duration::from_millis(250);

/// Results kept for a tab that reconnects having missed them.
const RECENT: usize = 64;

const MAX_HEADER: usize = 16 * 1024;
const BUFFER: usize = 1024 * 1024;
const UPLOAD_SUFFIX: &str = ".nudge-upload";

const PAGE: &str = include_str!("web_page.html");

/// Why sharing stopped, as the page is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StopReason {
    /// The user here turned it off.
    User,
    /// Nobody was connected for a long time.
    Idle,
}

impl StopReason {
    fn word(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Idle => "idle",
        }
    }
}

/// The running share: its link, and the browsers that opened it.
pub(super) struct WebShare {
    pub(super) url: String,
    token: Arc<str>,
    pub(super) browsers: HashMap<Fingerprint, Browser>,
    idle_since: Option<Instant>,
    /// Serves the link. Handed to `Ended` when sharing stops.
    listener: Option<JoinHandle<()>>,
    watchdog: JoinHandle<()>,
}

impl Drop for WebShare {
    fn drop(&mut self) {
        if let Some(task) = &self.listener {
            task.abort();
        }
        self.watchdog.abort();
    }
}

/// A stopped share's link, still answered for a while (`ENDED_LINGER`):
/// every request gets "sharing stopped", nothing else. It lets a tab tell
/// "stopped" from "out of range", which otherwise look the same.
pub(super) struct Ended {
    token: Arc<str>,
    why: StopReason,
    until: Instant,
    listener: Option<JoinHandle<()>>,
}

impl Drop for Ended {
    fn drop(&mut self) {
        if let Some(task) = &self.listener {
            task.abort();
        }
    }
}

/// One open tab.
pub(super) struct Browser {
    pub(super) name: String,
    /// Its page reached this device over a phone's cable (USB tethering).
    pub(super) via_cable: bool,
    /// The tab's event stream, while it's connected.
    events: Option<mpsc::UnboundedSender<String>>,
    /// Which connection `events` belongs to, so an old one closing late
    /// doesn't mark a newer one gone.
    connection: u64,
    gone_since: Option<Instant>,
    /// Offers waiting for an answer, by batch ID.
    offers: HashMap<u64, Offer>,
    /// Accepted sends not yet downloaded in full: one download each (a
    /// zip when there's more than one file), fetched in any order.
    downloads: Vec<Download>,
    /// Sends just accepted, about to join `downloads`.
    accepting: HashSet<u64>,
    /// Files this tab may still send: granted when the user accepts its
    /// "can I send you these?" question. Every send is asked about: a tab
    /// is temporary, so there's no "always accept".
    allowance: u32,
    /// Its question to the user here, while open.
    asking: Option<BatchKey>,
    /// Files it's sending now.
    uploads: HashSet<u64>,
    /// How its recent downloads ended (delivered or not), and the keys of
    /// its recent uploads that were saved: what a reconnecting tab may
    /// have missed hearing.
    recent_down: VecDeque<(u64, bool)>,
    recent_up: VecDeque<String>,
}

impl Browser {
    fn new(name: String) -> Self {
        Self {
            name,
            via_cable: false,
            events: None,
            connection: 0,
            gone_since: None,
            offers: HashMap::new(),
            downloads: Vec::new(),
            accepting: HashSet::new(),
            allowance: 0,
            asking: None,
            uploads: HashSet::new(),
            recent_down: VecDeque::new(),
            recent_up: VecDeque::new(),
        }
    }

    fn remember_down(&mut self, id: u64, ok: bool) {
        if self.recent_down.len() == RECENT {
            self.recent_down.pop_front();
        }
        self.recent_down.push_back((id, ok));
    }

    fn remember_up(&mut self, key: String) {
        if self.recent_up.len() == RECENT {
            self.recent_up.pop_front();
        }
        self.recent_up.push_back(key);
    }

    /// Where things stand, for a tab that (re)connects: the files still
    /// coming, how recent ones ended, and which uploads were saved. The
    /// page corrects itself from this, whatever it missed.
    fn sync_event(&self) -> String {
        let owed: Vec<String> = self.downloads.iter().map(Download::summary).collect();
        let done: Vec<String> = self
            .recent_down
            .iter()
            .map(|(id, ok)| format!("{{\"id\":{id},\"ok\":{ok}}}"))
            .collect();
        let up: Vec<String> = self.recent_up.iter().map(|k| json_str(k)).collect();
        format!(
            "{{\"owed\":[{}],\"done\":[{}],\"up\":[{}]}}",
            owed.join(","),
            done.join(","),
            up.join(",")
        )
    }

    pub(super) fn connected(&self) -> bool {
        self.events.is_some()
    }

    fn push(&self, event: &str, data: &str) {
        if let Some(events) = &self.events {
            let _ = events.send(format!("event: {event}\ndata: {data}\n\n"));
        }
    }

    /// Everything still owed to a tab that reconnected.
    fn resend(&self) {
        self.push("sync", &self.sync_event());
        for offer in self.offers.values() {
            self.push("offer", &offer.event);
        }
        for d in self.downloads.iter().filter(|d| d.streaming.is_none()) {
            self.push("download", &d.event());
        }
    }
}

struct Offer {
    event: String,
    /// Its files' transfer IDs.
    ids: Vec<u64>,
    /// Accept or decline; `None`: cancelled here.
    answer: oneshot::Sender<Option<bool>>,
}

/// An accepted send, as the tab downloads it: one download however many
/// files, so the browser only has to allow one (it holds back a second that
/// no tap started). Several files go as one zip.
struct Download {
    /// The send's batch ID (its first file's ID): the page asks for it by
    /// this.
    id: u64,
    /// What it's saved as: the file's name, or "3 files from Laptop.zip".
    name: String,
    /// Bytes on the wire.
    size: u64,
    /// Kept until it's settled, so a browser that asks again (a retry, or
    /// the page's Download button) can be served again.
    files: Vec<WebFile>,
    /// Requests so far; each one's number.
    attempts: u64,
    /// The request streaming it now, if any.
    streaming: Option<Streaming>,
}

/// A file in a send to a tab.
struct WebFile {
    id: u64,
    /// Its name; in a zip, its path there ("Trip/day 1/notes.txt").
    name: String,
    size: u64,
    /// Unix seconds, for the zip.
    modified: u64,
    item: Outgoing,
}

impl WebFile {
    fn new(id: u64, name: String, size: u64, item: Outgoing) -> Self {
        let metadata = match &item {
            Outgoing::Path(path) | Outgoing::InFolder { path, .. } => std::fs::metadata(path).ok(),
            Outgoing::Open { file, .. } => file.metadata().ok(),
            Outgoing::Pack(_) | Outgoing::EmptyFolder { .. } => None,
        };
        let modified = metadata
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        Self {
            id,
            name,
            size,
            modified,
            item,
        }
    }
}

/// The zip for these files (same names, same order, every time).
fn archive_of<'a>(files: impl Iterator<Item = (&'a str, u64, u64)>) -> Archive {
    Archive::new(
        files
            .map(|(name, size, modified)| Entry {
                name: name.to_owned(),
                size,
                modified,
            })
            .collect(),
    )
}

/// A request streaming a download.
struct Streaming {
    attempt: u64,
    /// Stops it for good: cancelled, or sharing stopped.
    cancel: Cancel,
    /// Tells it to let go: a newer request for the same send took over.
    stop: Arc<Notify>,
    /// Closes when it has stopped.
    done: watch::Receiver<()>,
}

impl Download {
    fn new(id: u64, files: Vec<WebFile>, from: &str) -> Self {
        let (name, size) = match files.as_slice() {
            [one] => (one.name.clone(), one.size),
            _ => (
                format!("{} files from {from}.zip", files.len()),
                archive_of(files.iter().map(|f| (f.name.as_str(), f.size, f.modified))).total_size(),
            ),
        };
        Self {
            id,
            name,
            size,
            files,
            attempts: 0,
            streaming: None,
        }
    }

    fn covers(&self, id: u64) -> bool {
        self.files.iter().any(|f| f.id == id)
    }

    fn summary(&self) -> String {
        let ids: Vec<String> = self.files.iter().map(|f| f.id.to_string()).collect();
        format!(
            "{{\"id\":{},\"name\":{},\"size\":{},\"ids\":[{}]}}",
            self.id,
            json_str(&self.name),
            self.size,
            ids.join(",")
        )
    }

    /// "Fetch this"; `streaming` says if a request already is.
    fn event(&self) -> String {
        let summary = self.summary();
        format!(
            "{},\"streaming\":{}}}",
            &summary[..summary.len() - 1],
            self.streaming.is_some()
        )
    }
}

/// Start sharing, or return the link if already sharing.
pub(super) async fn start(shared: &Arc<Shared>) -> Result<String> {
    if let Some(web) = &shared.state().web {
        return Ok(web.url.clone());
    }
    // The last share's link may still hold the port: let it go first, so
    // the new link gets the usual port.
    let ended = shared.state().web_ended.take();
    if let Some(task) = ended.and_then(|mut e| e.listener.take()) {
        task.abort();
        let _ = task.await;
    }
    let bind = shared.bind;
    let listener = match TcpListener::bind((bind, WEB_PORT)).await {
        Ok(l) => l,
        Err(_) => TcpListener::bind((bind, 0))
            .await
            .map_err(|e| Error::WebShare(e.to_string()))?,
    };
    let port = listener.local_addr()?.port();
    let host = if bind.is_loopback() {
        bind
    } else {
        best_lan_ip().ok_or_else(|| Error::WebShare("not connected to a network".into()))?
    };
    let token: Arc<str> = hex(&random_bytes::<16>()).into();
    let url = format!("http://{host}:{port}/t/{token}");

    let mut state = shared.state();
    if let Some(web) = &state.web {
        // Started by another call meanwhile.
        return Ok(web.url.clone());
    }
    state.web = Some(WebShare {
        url: url.clone(),
        token: token.clone(),
        browsers: HashMap::new(),
        idle_since: Some(Instant::now()),
        listener: Some(tokio::spawn(accept_loop(shared.clone(), listener, token))),
        watchdog: tokio::spawn(watchdog(shared.clone())),
    });
    Ok(url)
}

/// Stop sharing. Open tabs are told, and lose their place; what was on its
/// way stops, and unanswered offers fail. The link keeps saying it ended
/// for a while (see `Ended`).
pub(super) fn stop(shared: &Shared, why: StopReason) {
    let Some(mut web) = shared.state().web.take() else {
        return;
    };
    let data = format!("{{\"why\":\"{}\"}}", why.word());
    let mut downloading = Vec::new();
    let mut uploading = Vec::new();
    for tab in web.browsers.values() {
        tab.push("stopped", &data);
        downloading.extend(
            tab.downloads
                .iter()
                .filter_map(|d| d.streaming.as_ref().map(|s| s.cancel.clone())),
        );
        uploading.extend(tab.uploads.iter().copied());
    }
    fail_leftovers(shared, std::mem::take(&mut web.browsers).into_values());
    // Downloads and uploads under way end as "sharing was stopped" (see
    // `ended_error`).
    for cancel in downloading {
        cancel.cancel(CancelledBy::Here);
    }
    for id in uploading {
        let progress = shared.state().active.get(&id).map(|a| a.progress.clone());
        if let Some(p) = progress {
            p.cancel().cancel(CancelledBy::Here);
        }
    }
    shared.state().web_ended = Some(Ended {
        token: web.token.clone(),
        why,
        until: Instant::now() + ENDED_LINGER,
        listener: web.listener.take(),
    });
    drop(web);
    shared.emit(NodeEvent::WebShareStopped);
    shared.emit(NodeEvent::PeersChanged);
}

/// Whether `token` is the link being shared now.
fn live(shared: &Shared, token: &str) -> bool {
    shared
        .state()
        .web
        .as_ref()
        .is_some_and(|w| *w.token == *token)
}

/// A transfer that stopped because sharing did says so.
fn ended_error(shared: &Shared, token: &str, e: Error) -> Error {
    match e {
        Error::Cancelled if !live(shared, token) => Error::WebShareStopped,
        e => e,
    }
}

/// What a link is now.
enum Link {
    Live,
    Ended(StopReason),
}

/// `None`: not a link any more (or never was).
fn link(shared: &Shared, token: &str) -> Option<Link> {
    let mut state = shared.state();
    if state.web.as_ref().is_some_and(|w| *w.token == *token) {
        return Some(Link::Live);
    }
    let ended = state.web_ended.as_ref().filter(|e| *e.token == *token)?;
    if ended.until > Instant::now() {
        return Some(Link::Ended(ended.why));
    }
    // Said it for long enough: close the port.
    state.web_ended = None;
    None
}

/// Longest clipboard text a tab may send (about the same as paired devices).
const MAX_CLIP: u64 = 1 << 20;

/// Show clipboard text in a tab, with a button to copy it. (A page can't
/// write to the clipboard by itself, so the person taps Copy.)
pub(super) fn send_text(shared: &Shared, id: &Fingerprint, text: &str) -> Result<()> {
    let state = shared.state();
    let tab = state
        .web
        .as_ref()
        .and_then(|w| w.browsers.get(id))
        .filter(|tab| tab.connected())
        .ok_or(Error::BrowserGone)?;
    tab.push("clip", &format!("{{\"text\":{}}}", json_str(text)));
    Ok(())
}

pub(super) fn is_browser(shared: &Shared, id: &Fingerprint) -> bool {
    shared
        .state()
        .web
        .as_ref()
        .is_some_and(|w| w.browsers.contains_key(id))
}

/// Offer files to a browser and, once accepted, let it download them.
pub(super) fn send(shared: Arc<Shared>, browser: Fingerprint, jobs: Vec<(u64, Outgoing)>) {
    tokio::spawn(async move {
        let files: Vec<(u64, String, u64)> = jobs
            .iter()
            .map(|(id, item)| (*id, item.name(), item.size()))
            .collect();
        for (id, name, size) in &files {
            shared.emit(NodeEvent::TransferStarted {
                id: *id,
                peer: browser,
                direction: Direction::Outgoing,
                file_name: name.clone(),
                size: *size,
            });
        }
        let Some(&(batch, _, _)) = files.first() else {
            return;
        };
        let mut event = format!("{{\"batch\":{batch},\"files\":[");
        for (i, (id, name, size)) in files.iter().enumerate() {
            let comma = if i > 0 { "," } else { "" };
            let _ = write!(
                event,
                "{comma}{{\"id\":{id},\"name\":{},\"size\":{size}}}",
                json_str(name)
            );
        }
        event.push_str("]}");

        let (answer, answered) = oneshot::channel();
        {
            let mut state = shared.state();
            let Some(tab) = state
                .web
                .as_mut()
                .and_then(|w| w.browsers.get_mut(&browser))
            else {
                drop(state);
                fail_all(&shared, &files, || Error::BrowserGone);
                return;
            };
            tab.push("offer", &event);
            let ids = files.iter().map(|(id, _, _)| *id).collect();
            tab.offers.insert(batch, Offer { event, ids, answer });
        }
        // Delivered to the page; now it's up to them.
        for (id, _, _) in &files {
            shared.emit(NodeEvent::TransferPending { id: *id });
        }

        let result = tokio::time::timeout(ANSWER_TIMEOUT, answered).await;
        let mut state = shared.state();
        let tab = state
            .web
            .as_mut()
            .and_then(|w| w.browsers.get_mut(&browser));
        match (result, tab) {
            (Ok(Ok(Some(true))), Some(tab)) => {
                for (id, _, _) in &files {
                    shared.emit(NodeEvent::TransferAccepted { id: *id });
                }
                tab.accepting.remove(&batch);
                let items = jobs.into_iter().map(|(_, item)| item);
                let web_files = files
                    .iter()
                    .zip(items)
                    .map(|((id, name, size), item)| WebFile::new(*id, name.clone(), *size, item))
                    .collect();
                let d = Download::new(batch, web_files, &shared.device_name);
                // The page usually started it already, with the Accept tap.
                tab.push("download", &d.event());
                tab.downloads.push(d);
            }
            (Ok(Ok(Some(false))), _) => {
                drop(state);
                fail_all(&shared, &files, || Error::Declined);
            }
            (Ok(Ok(None)), tab) => {
                if let Some(tab) = tab {
                    tab.push("withdrawn", &format!("{{\"batch\":{batch}}}"));
                }
                drop(state);
                fail_all(&shared, &files, || Error::Cancelled);
            }
            (Err(_), tab) => {
                if let Some(tab) = tab {
                    tab.offers.remove(&batch);
                    tab.push("withdrawn", &format!("{{\"batch\":{batch}}}"));
                }
                drop(state);
                fail_all(&shared, &files, || Error::NoAnswer);
            }
            // Accepted just as the tab went, or the tab went unanswered.
            (Ok(_), _) => {
                drop(state);
                fail_all(&shared, &files, || Error::BrowserGone);
            }
        }
    });
}

fn fail_all(shared: &Shared, files: &[(u64, String, u64)], error: impl Fn() -> Error) {
    for (id, _, _) in files {
        shared.finish(*id, Err(error()));
    }
}

/// Fail what removed tabs still had waiting. Unanswered offers fail on
/// their own when their answer channel drops.
fn fail_leftovers(shared: &Shared, gone: impl Iterator<Item = Browser>) {
    for tab in gone {
        for d in tab.downloads {
            // A download under way finishes (or fails) by itself.
            if d.streaming.is_none() {
                for f in &d.files {
                    shared.finish(f.id, Err(Error::BrowserGone));
                }
            }
        }
    }
}

/// Stop the send to a tab that file `id` is part of: still waiting for an
/// answer, waiting to be downloaded, or downloading. False if `id` isn't
/// one.
pub(super) fn cancel(shared: &Shared, id: u64, by: CancelledBy) -> bool {
    let found = {
        let mut state = shared.state();
        let Some(web) = state.web.as_mut() else {
            return false;
        };
        let mut found = None;
        for tab in web.browsers.values_mut() {
            let offer = tab
                .offers
                .iter()
                .find(|(_, o)| o.ids.contains(&id))
                .map(|(batch, _)| *batch);
            if let Some(batch) = offer {
                // Its send task fails the files and tells the tab.
                if let Some(offer) = tab.offers.remove(&batch) {
                    let _ = offer.answer.send(None);
                }
                return true;
            }
            let send = tab.downloads.iter().find(|d| d.covers(id)).map(|d| d.id);
            if let Some(send) = send {
                found = Some(stop_downloads(tab, Some(send), by));
                break;
            }
        }
        found
    };
    match found {
        Some(stopped) => {
            stopped.finish(shared, by);
            true
        }
        None => false,
    }
}

/// Downloads stopped: files not started yet, and requests under way.
#[derive(Default)]
struct Stopped {
    unstarted: Vec<u64>,
    streaming: Vec<Cancel>,
}

impl Stopped {
    fn finish(self, shared: &Shared, by: CancelledBy) {
        for id in self.unstarted {
            shared.finish(id, Err(by.error()));
        }
        // Each ends, and fails its files, when its stream does.
        for cancel in self.streaming {
            cancel.cancel(by);
        }
    }
}

/// Take the tab's download `send` (or all of them) off its list, and tell
/// it. One under way ends when it's cancelled (`Stopped::finish`).
fn stop_downloads(tab: &mut Browser, send: Option<u64>, by: CancelledBy) -> Stopped {
    let mut stopped = Stopped::default();
    let mut ids = Vec::new();
    tab.downloads.retain(|d| {
        let ours = send.is_none_or(|s| d.id == s);
        if ours {
            ids.push(d.id);
            ids.extend(d.files.iter().map(|f| f.id).filter(|id| *id != d.id));
            match &d.streaming {
                None => stopped.unstarted.extend(d.files.iter().map(|f| f.id)),
                Some(s) => stopped.streaming.push(s.cancel.clone()),
            }
        }
        !ours
    });
    for id in &ids {
        tab.remember_down(*id, false);
    }
    if !ids.is_empty() {
        let list: Vec<String> = ids.iter().map(u64::to_string).collect();
        tab.push(
            "cancelled",
            &format!(
                "{{\"what\":\"down\",\"by\":\"{}\",\"ids\":[{}]}}",
                by_word(by),
                list.join(",")
            ),
        );
    }
    stopped
}

/// Who stopped it, as the page sees it: "here" is this device, which is
/// "there" for the page.
fn by_word(by: CancelledBy) -> &'static str {
    match by {
        CancelledBy::Here => "them",
        CancelledBy::There => "you",
    }
}

/// The tab's Cancel: stop what it's sending (`what=up`, including its
/// question to the user here) or receiving (`what=down`).
fn cancel_from_tab(shared: &Shared, session: Fingerprint, what: &str) -> bool {
    let (stopped, uploads, question) = {
        let mut state = shared.state();
        let Some(tab) = state
            .web
            .as_mut()
            .and_then(|w| w.browsers.get_mut(&session))
        else {
            return false;
        };
        if what == "down" {
            let stopped = stop_downloads(tab, None, CancelledBy::There);
            (Some(stopped), Vec::new(), None)
        } else {
            tab.allowance = 0;
            let uploads: Vec<u64> = tab.uploads.iter().copied().collect();
            let asking = tab.asking.take();
            let question = asking.and_then(|key| {
                let qid = state
                    .questions
                    .iter()
                    .find(|(_, (k, _))| *k == key)
                    .map(|(qid, _)| *qid)?;
                state.questions.remove(&qid).map(|(_, tx)| (qid, tx))
            });
            (None, uploads, question)
        }
    };
    if let Some(stopped) = stopped {
        stopped.finish(shared, CancelledBy::There);
    }
    for id in uploads {
        let progress = shared.state().active.get(&id).map(|a| a.progress.clone());
        if let Some(p) = progress {
            p.cancel().cancel(CancelledBy::There);
        }
    }
    if let Some((qid, tx)) = question {
        let _ = tx.send(Some(Verdict::SenderCancelled));
        shared.emit(NodeEvent::OfferClosed { id: qid });
    }
    true
}

/// Forget tabs that have been gone too long, and stop sharing after a long
/// spell with nobody connected.
async fn watchdog(shared: Arc<Shared>) {
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        tick.tick().await;
        let mut stop_now = false;
        let gone: Vec<Browser> = {
            let mut state = shared.state();
            let Some(web) = state.web.as_mut() else {
                return;
            };
            let expired: Vec<Fingerprint> = web
                .browsers
                .iter()
                .filter(|(_, b)| b.gone_since.is_some_and(|t| t.elapsed() > GRACE))
                .map(|(id, _)| *id)
                .collect();
            let gone = expired
                .iter()
                .filter_map(|id| web.browsers.remove(id))
                .collect();
            if web.browsers.values().any(Browser::connected) {
                web.idle_since = None;
            } else if web.idle_since.get_or_insert_with(Instant::now).elapsed() > IDLE_LIMIT {
                stop_now = true;
            }
            gone
        };
        if !gone.is_empty() {
            fail_leftovers(&shared, gone.into_iter());
            shared.emit(NodeEvent::PeersChanged);
        }
        if stop_now {
            stop(&shared, StopReason::Idle);
            return;
        }
    }
}

/// Serves the link while it's live, then while it's ended (`Ended`); closes
/// the port once it's neither.
async fn accept_loop(shared: Arc<Shared>, listener: TcpListener, token: Arc<str>) {
    loop {
        let accepted = tokio::select! {
            a = listener.accept() => Some(a),
            () = tokio::time::sleep(Duration::from_secs(30)) => None,
        };
        if link(&shared, &token).is_none() {
            return;
        }
        match accepted {
            Some(Ok((tcp, _))) => {
                let _ = tcp.set_nodelay(true);
                let shared = shared.clone();
                let token = token.clone();
                tokio::spawn(async move {
                    let _ = handle(&shared, &token, tcp).await;
                });
            }
            Some(Err(_)) => tokio::time::sleep(Duration::from_millis(100)).await,
            None => {}
        }
    }
}

struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    content_length: Option<u64>,
    user_agent: String,
    /// Body bytes that arrived with the headers.
    body_start: Vec<u8>,
}

async fn read_request(tcp: &mut TcpStream) -> std::io::Result<Option<Request>> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 2048];
    loop {
        let n = tokio::time::timeout(Duration::from_secs(15), tcp.read(&mut chunk))
            .await
            .map_err(|_| std::io::Error::other("request timed out"))??;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
        let mut headers = [httparse::EMPTY_HEADER; 48];
        let mut req = httparse::Request::new(&mut headers);
        match req.parse(&buf) {
            Ok(httparse::Status::Complete(len)) => {
                let target = req.path.unwrap_or("/");
                let (path, query) = target.split_once('?').unwrap_or((target, ""));
                let header = |name: &str| {
                    req.headers
                        .iter()
                        .find(|h| h.name.eq_ignore_ascii_case(name))
                        .map(|h| String::from_utf8_lossy(h.value).into_owned())
                };
                return Ok(Some(Request {
                    method: req.method.unwrap_or("").to_owned(),
                    path: path.to_owned(),
                    query: parse_query(query),
                    content_length: header("content-length").and_then(|v| v.trim().parse().ok()),
                    user_agent: header("user-agent").unwrap_or_default(),
                    body_start: buf[len..].to_vec(),
                }));
            }
            Ok(httparse::Status::Partial) if buf.len() < MAX_HEADER => {}
            _ => return Ok(None),
        }
    }
}

async fn handle(shared: &Arc<Shared>, token: &str, mut tcp: TcpStream) -> std::io::Result<()> {
    let Some(req) = read_request(&mut tcp).await? else {
        return Ok(());
    };
    let mut parts = req.path.trim_matches('/').split('/');
    let authorized = parts.next() == Some("t") && parts.next().is_some_and(|t| same(t, token));
    let found = if authorized { link(shared, token) } else { None };
    let route: Vec<&str> = parts.collect();
    match found {
        None => return respond(&mut tcp, "404 Not Found", "text/plain", b"Not found").await,
        Some(Link::Ended(why)) => return ended(&mut tcp, &req.method, &route, why).await,
        Some(Link::Live) => {}
    }
    let session = req
        .query
        .get("s")
        .and_then(|s| s.parse::<Fingerprint>().ok());
    match (req.method.as_str(), route.as_slice()) {
        ("GET", []) => page(&mut tcp).await,
        // The page checks this when its connection drops: an answer means
        // still sharing (any other answer, not; no answer, out of reach).
        ("GET", ["ping"]) => respond(&mut tcp, "200 OK", "application/json", b"{}").await,
        ("GET", ["events"]) => events(shared, token, &mut tcp, session, &req.user_agent).await,
        ("POST", ["answer"]) => {
            let ok = answer(shared, session, &req.query);
            let status = if ok { "200 OK" } else { "404 Not Found" };
            respond(&mut tcp, status, "application/json", b"{}").await
        }
        ("GET", ["f", id]) => match (session, id.parse::<u64>()) {
            (Some(s), Ok(id)) => download(shared, token, &mut tcp, s, id).await,
            _ => respond(&mut tcp, "404 Not Found", "text/plain", b"Not found").await,
        },
        ("POST", ["ask"]) => match session {
            Some(s) => ask(shared, &mut tcp, s, &req.query).await,
            None => respond(&mut tcp, "404 Not Found", "text/plain", b"Not found").await,
        },
        ("POST", ["u"]) => match session {
            Some(s) => upload(shared, token, &mut tcp, s, req).await,
            None => respond(&mut tcp, "404 Not Found", "text/plain", b"Not found").await,
        },
        ("POST", ["cancel"]) => {
            let what = req.query.get("what").map_or("up", String::as_str);
            let ok = session.is_some_and(|s| cancel_from_tab(shared, s, what));
            let status = if ok { "200 OK" } else { "404 Not Found" };
            respond(&mut tcp, status, "application/json", b"{}").await
        }
        ("POST", ["clip"]) => match session {
            Some(s) => clip(shared, &mut tcp, s, req).await,
            None => respond(&mut tcp, "404 Not Found", "text/plain", b"Not found").await,
        },
        _ => respond(&mut tcp, "404 Not Found", "text/plain", b"Not found").await,
    }
}

/// "Can I send you these?" from a tab: the user here accepts or declines. The reply,
/// `{"answer":"accepted"|"declined"|"none"}`, comes when they do.
async fn ask(
    shared: &Arc<Shared>,
    tcp: &mut TcpStream,
    session: Fingerprint,
    query: &HashMap<String, String>,
) -> std::io::Result<()> {
    if !is_browser(shared, &session) {
        return respond(tcp, "404 Not Found", "text/plain", b"Not found").await;
    }
    let count: u32 = query
        .get("count")
        .and_then(|c| c.parse().ok())
        .unwrap_or(1)
        .clamp(1, 10_000);
    let size: u64 = query.get("size").and_then(|c| c.parse().ok()).unwrap_or(0);
    let name = query.get("name").cloned().unwrap_or_default();
    let answer = match shared.start_asking(session, 0, &name, count, size) {
        super::Asking::Now(answer) => answer,
        super::Asking::Wait(key, rx) => {
            if let Some(tab) = shared
                .state()
                .web
                .as_mut()
                .and_then(|w| w.browsers.get_mut(&session))
            {
                tab.asking = Some(key);
            }
            let answer = shared.wait_answer(key, rx).await;
            if let Some(tab) = shared
                .state()
                .web
                .as_mut()
                .and_then(|w| w.browsers.get_mut(&session))
            {
                tab.asking = None;
            }
            answer
        }
    };
    if answer == Verdict::Yes
        && let Some(tab) = shared
            .state()
            .web
            .as_mut()
            .and_then(|w| w.browsers.get_mut(&session))
    {
        tab.allowance = tab.allowance.saturating_add(count);
    }
    let word = match answer {
        Verdict::Yes => "accepted",
        Verdict::No => "declined",
        Verdict::Lapsed => "none",
        Verdict::SenderCancelled | Verdict::ReceiverCancelled => "cancelled",
    };
    respond(
        tcp,
        "200 OK",
        "application/json",
        format!("{{\"answer\":\"{word}\"}}").as_bytes(),
    )
    .await
}

/// A stopped share's link: the page still loads (and shows that sharing
/// stopped); everything else is `410 Gone`, with why.
async fn ended(
    tcp: &mut TcpStream,
    method: &str,
    route: &[&str],
    why: StopReason,
) -> std::io::Result<()> {
    if method == "GET" && route.is_empty() {
        return page(tcp).await;
    }
    let body = format!("{{\"stopped\":\"{}\"}}", why.word());
    respond(tcp, "410 Gone", "application/json", body.as_bytes()).await
}

async fn page(tcp: &mut TcpStream) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Referrer-Policy: no-referrer\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Content-Security-Policy: default-src 'self'; img-src data:; script-src 'unsafe-inline'; style-src 'unsafe-inline'\r\n\
         Connection: close\r\n\r\n",
        PAGE.len()
    );
    tcp.write_all(head.as_bytes()).await?;
    tcp.write_all(PAGE.as_bytes()).await?;
    tcp.shutdown().await
}

/// The tab's event stream. While it's open, the tab is a device.
async fn events(
    shared: &Arc<Shared>,
    token: &str,
    tcp: &mut TcpStream,
    session: Option<Fingerprint>,
    user_agent: &str,
) -> std::io::Result<()> {
    // How the tab reached this device. Over Wi-Fi with a phone's cable
    // plugged in too, the page is told the link's address on the cable: if
    // it can reach it, it's on that phone, and it moves over (faster).
    let nets = link::local_nets();
    let here = tcp.local_addr()?;
    let via = link::kind_of_local(&nets, here.ip());
    let cable_url = (via != LinkKind::Usb)
        .then(|| nets.iter().find(|n| n.kind == LinkKind::Usb))
        .flatten()
        .map(|n| format!("http://{}:{}/t/{token}", n.ip, here.port()));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (id, connection) = {
        let mut state = shared.state();
        let Some(web) = state.web.as_mut() else {
            return Ok(());
        };
        // A known tab coming back keeps its place; anything else is new.
        let id = match session.filter(|s| web.browsers.contains_key(s)) {
            Some(id) => id,
            None => {
                let id = Fingerprint::from_bytes(random_bytes());
                let name = unique_name(web, name_from_user_agent(user_agent));
                web.browsers.insert(id, Browser::new(name));
                id
            }
        };
        let tab = web.browsers.get_mut(&id).expect("just found or added");
        tab.connection += 1;
        tab.events = Some(tx);
        tab.gone_since = None;
        tab.via_cable = via == LinkKind::Usb;
        web.idle_since = None;
        tab.push(
            "hello",
            &format!(
                "{{\"session\":\"{id}\",\"from\":{},\"platform\":{},\"name\":{},\"via\":\"{}\",\"cable\":{}}}",
                json_str(&shared.device_name),
                json_str(&shared.platform),
                json_str(&tab.name),
                if tab.via_cable { "cable" } else { via.name() },
                cable_url.as_deref().map_or_else(|| "null".to_owned(), json_str)
            ),
        );
        tab.resend();
        (id, tab.connection)
    };
    shared.emit(NodeEvent::PeersChanged);

    let result = async {
        tcp.write_all(
            b"HTTP/1.1 200 OK\r\n\
              Content-Type: text/event-stream\r\n\
              Cache-Control: no-store\r\n\
              X-Accel-Buffering: no\r\n\
              Connection: close\r\n\r\n",
        )
        .await?;
        let mut ping = tokio::time::interval(PING_INTERVAL);
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Some(msg) => tcp.write_all(msg.as_bytes()).await?,
                    None => return Ok(()), // sharing stopped, or the tab was replaced
                },
                _ = ping.tick() => tcp.write_all(b"event: ping\ndata: {}\n\n").await?,
            }
        }
    }
    .await;

    let changed = {
        let mut state = shared.state();
        match state.web.as_mut().and_then(|w| w.browsers.get_mut(&id)) {
            Some(tab) if tab.connection == connection => {
                tab.events = None;
                tab.gone_since = Some(Instant::now());
                true
            }
            _ => false,
        }
    };
    if changed {
        shared.emit(NodeEvent::PeersChanged);
    }
    result
}

fn answer(shared: &Shared, session: Option<Fingerprint>, query: &HashMap<String, String>) -> bool {
    let (Some(session), Some(batch)) = (session, query.get("batch").and_then(|b| b.parse().ok()))
    else {
        return false;
    };
    let accept = query.get("accept").is_some_and(|a| a == "1");
    let mut state = shared.state();
    let offer = state
        .web
        .as_mut()
        .and_then(|w| w.browsers.get_mut(&session))
        .and_then(|tab| tab.offers.remove(&batch));
    let Some(offer) = offer else {
        return false;
    };
    if accept && let Some(tab) = state.web.as_mut().and_then(|w| w.browsers.get_mut(&session)) {
        tab.accepting.insert(batch);
    }
    offer.answer.send(Some(accept)).is_ok()
}

/// How long a request for a send waits for its Accept to land. The page
/// starts the download in the same tap as Accept (so the browser counts it
/// as the person's own doing), and it can get here first.
const CLAIM_WAIT: Duration = Duration::from_secs(15);

/// A request's hold on a download.
struct Claim {
    files: Vec<(u64, String, u64, u64, Outgoing)>,
    name: String,
    size: u64,
    attempt: u64,
    previous: Option<Streaming>,
    cancel: Cancel,
    stop: Arc<Notify>,
    /// Dropped when this request is over (see `Streaming::done`).
    _done: watch::Sender<()>,
}

enum Claimed {
    Got(Claim),
    /// Accepted (or about to be) but not ready yet.
    Wait,
    No,
}

fn claim(shared: &Shared, session: Fingerprint, id: u64) -> Claimed {
    let mut state = shared.state();
    let Some(tab) = state
        .web
        .as_mut()
        .and_then(|w| w.browsers.get_mut(&session))
    else {
        return Claimed::No;
    };
    let Some(d) = tab.downloads.iter_mut().find(|d| d.id == id) else {
        let soon = tab.offers.contains_key(&id) || tab.accepting.contains(&id);
        return if soon { Claimed::Wait } else { Claimed::No };
    };
    let files: std::io::Result<Vec<_>> = d
        .files
        .iter()
        .map(|f| {
            let item = f.item.try_clone()?;
            Ok((f.id, f.name.clone(), f.size, f.modified, item))
        })
        .collect();
    let Ok(files) = files else {
        return Claimed::No;
    };
    d.attempts += 1;
    let cancel = Cancel::new();
    let stop = Arc::new(Notify::new());
    let (done, done_rx) = watch::channel(());
    let previous = d.streaming.replace(Streaming {
        attempt: d.attempts,
        cancel: cancel.clone(),
        stop: stop.clone(),
        done: done_rx,
    });
    let claim = Claim {
        files,
        name: d.name.clone(),
        size: d.size,
        attempt: d.attempts,
        previous,
        cancel,
        stop,
        _done: done,
    };
    tab.push("started", &format!("{{\"id\":{id}}}"));
    Claimed::Got(claim)
}

/// Stream an accepted send to the tab, by its batch ID. Asked for again
/// while it's still on the list (the browser retrying a broken download,
/// or the page's Download button), the newer request takes over.
async fn download(
    shared: &Arc<Shared>,
    token: &str,
    tcp: &mut TcpStream,
    session: Fingerprint,
    id: u64,
) -> std::io::Result<()> {
    let deadline = Instant::now() + CLAIM_WAIT;
    let claim = loop {
        match claim(shared, session, id) {
            Claimed::Got(c) => break c,
            Claimed::Wait if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            _ => return respond(tcp, "404 Not Found", "text/plain", b"Not found").await,
        }
    };
    let Claim {
        files,
        name,
        size,
        attempt,
        previous,
        cancel,
        stop,
        _done,
    } = claim;
    if let Some(previous) = previous {
        // The older request lets go of the files (its `done` closes) before
        // this one reads them, so the two never read at once.
        previous.stop.notify_one();
        let mut done = previous.done;
        let gone = async { while done.changed().await.is_ok() {} };
        let _ = tokio::time::timeout(Duration::from_secs(5), gone).await;
    }

    let about: Vec<(u64, String, u64)> = files
        .iter()
        .map(|(id, name, size, _, _)| (*id, name.clone(), *size))
        .collect();
    let start = Instant::now();
    let streamed = tokio::select! {
        r = stream_download(shared, tcp, session, id, &name, size, files, &cancel) => r,
        by = cancel.until() => Err(by.error()),
        // A newer request took over; it settles the send.
        () = stop.notified() => return Ok(()),
    };
    match streamed {
        // Broken off (not cancelled, and still sharing): the browser may ask
        // again. Leave it on the list for a moment before failing it.
        Err(e) if !matches!(e, Error::Cancelled | Error::CancelledThere) && live(shared, token) => {
            let wait = {
                let mut state = shared.state();
                let d = state
                    .web
                    .as_mut()
                    .and_then(|w| w.browsers.get_mut(&session))
                    .and_then(|tab| tab.downloads.iter_mut().find(|d| d.id == id));
                d.map(|d| {
                    let ours = d.streaming.as_ref().is_some_and(|s| s.attempt == attempt);
                    if ours {
                        d.streaming = None;
                    }
                    ours
                })
            };
            match wait {
                Some(true) => {
                    let shared = shared.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(RETRY_WINDOW).await;
                        settle(&shared, session, id, attempt, &about, Err(e), true);
                    });
                }
                // A newer request has it.
                Some(false) => {}
                // Off the list already (the tab went): just fail it.
                None => settle(shared, session, id, attempt, &about, Err(e), false),
            }
        }
        result => {
            let result = result
                .map(|()| start.elapsed())
                .map_err(|e| ended_error(shared, token, e));
            settle(shared, session, id, attempt, &about, result, false);
        }
    }
    Ok(())
}

/// A send's download is over: tell the tab and the app.
/// `retried_out`: called after `RETRY_WINDOW`, so only if nobody asked for
/// it again meanwhile and it wasn't settled some other way.
fn settle(
    shared: &Shared,
    session: Fingerprint,
    id: u64,
    attempt: u64,
    files: &[(u64, String, u64)],
    result: Result<Duration>,
    retried_out: bool,
) {
    let ok = result.is_ok();
    {
        let mut state = shared.state();
        let tab = state
            .web
            .as_mut()
            .and_then(|w| w.browsers.get_mut(&session));
        let at = tab
            .as_ref()
            .and_then(|tab| tab.downloads.iter().position(|d| d.id == id));
        match (tab, at) {
            (Some(tab), Some(at)) => {
                let d = &tab.downloads[at];
                let unclaimed = d.attempts == attempt && d.streaming.is_none();
                if retried_out && !unclaimed {
                    return;
                }
                let d = tab.downloads.remove(at);
                // An older request still going (this one overtook it and
                // won): stop it quietly.
                if let Some(other) = d.streaming.filter(|s| s.attempt != attempt) {
                    other.stop.notify_one();
                }
                tab.remember_down(id, ok);
                for (file, _, _) in files.iter().filter(|(f, _, _)| *f != id) {
                    tab.remember_down(*file, ok);
                }
                let event = if ok { "delivered" } else { "failed" };
                tab.push(event, &format!("{{\"id\":{id}}}"));
            }
            // Settled some other way already (cancelled, or the tab went).
            _ if retried_out => return,
            _ => {}
        }
    }
    match result {
        Ok(elapsed) => {
            for (file, name, size) in files {
                let stats = TransferStats {
                    bytes: *size,
                    resumed_from: 0,
                    elapsed,
                };
                shared.finish(*file, Ok((PathBuf::from(name), stats)));
            }
        }
        Err(e) => {
            for (file, _, _) in files {
                shared.finish(*file, Err(copy_error(&e)));
            }
        }
    }
}

/// The same error again, for the next file of a send that failed as one.
fn copy_error(e: &Error) -> Error {
    match e {
        Error::Cancelled => Error::Cancelled,
        Error::CancelledThere => Error::CancelledThere,
        Error::WebShareStopped => Error::WebShareStopped,
        Error::BrowserStalled => Error::BrowserStalled,
        Error::BrowserGone => Error::BrowserGone,
        Error::Truncated { got, expected } => Error::Truncated {
            got: *got,
            expected: *expected,
        },
        Error::Io(io) => Error::Io(std::io::Error::new(io.kind(), io.to_string())),
        other => Error::Io(std::io::Error::other(other.to_string())),
    }
}

/// Read up to `buf.len()` bytes at `at`, without moving the file's position:
/// a retry's handle to an already-open file shares that position.
async fn read_chunk(
    file: Arc<std::fs::File>,
    mut buf: Vec<u8>,
    at: u64,
) -> std::io::Result<(Vec<u8>, usize)> {
    tokio::task::spawn_blocking(move || {
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::read_at(&*file, &mut buf, at)?;
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_read(&*file, &mut buf, at)?;
        Ok((buf, n))
    })
    .await
    .map_err(std::io::Error::other)?
}

/// The connection a download goes out on: counts what's sent, gives up on a
/// browser that takes nothing for `STALL_LIMIT`, and tells the page how far
/// it's got.
struct Wire<'a> {
    tcp: &'a mut TcpStream,
    shared: &'a Shared,
    browser: Fingerprint,
    id: u64,
    sent: u64,
    size: u64,
    told: Instant,
}

impl Wire<'_> {
    async fn write(&mut self, mut bytes: &[u8]) -> Result<()> {
        // In pieces, so a slow browser isn't mistaken for a stalled one:
        // only no progress at all for `STALL_LIMIT` is.
        while !bytes.is_empty() {
            let wrote = tokio::time::timeout(STALL_LIMIT, self.tcp.write(bytes))
                .await
                .map_err(|_| Error::BrowserStalled)??;
            if wrote == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::WriteZero).into());
            }
            bytes = &bytes[wrote..];
            self.sent += wrote as u64;
        }
        // The page shows the same progress as the app.
        if self.told.elapsed() >= PAGE_PROGRESS || self.sent == self.size {
            self.told = Instant::now();
            if let Some(tab) = self
                .shared
                .state()
                .web
                .as_ref()
                .and_then(|w| w.browsers.get(&self.browser))
            {
                tab.push(
                    "progress",
                    &format!(
                        "{{\"id\":{},\"bytes\":{},\"size\":{}}}",
                        self.id, self.sent, self.size
                    ),
                );
            }
        }
        Ok(())
    }

    /// `size` bytes of `file`, onto the wire.
    async fn file(
        &mut self,
        file: std::fs::File,
        size: u64,
        progress: &Progress,
        mut crc: Option<&mut crc32fast::Hasher>,
    ) -> Result<()> {
        let file = Arc::new(file);
        let mut buf = vec![0u8; BUFFER];
        let mut done = 0u64;
        while done < size {
            let (back, n) = read_chunk(file.clone(), std::mem::take(&mut buf), done).await?;
            buf = back;
            let n = n.min((size - done) as usize);
            if n == 0 {
                return Err(Error::Truncated {
                    got: done,
                    expected: size,
                });
            }
            if let Some(crc) = crc.as_deref_mut() {
                crc.update(&buf[..n]);
            }
            self.write(&buf[..n]).await?;
            done += n as u64;
            progress.set(done);
        }
        Ok(())
    }
}

fn open(item: Outgoing) -> Result<std::fs::File> {
    match item {
        Outgoing::Path(path) | Outgoing::InFolder { path, .. } => Ok(std::fs::File::open(path)?),
        Outgoing::Open { file, .. } => Ok(file),
        // Never made for a browser (see Node::send).
        Outgoing::Pack(_) | Outgoing::EmptyFolder { .. } => {
            Err(Error::Protocol("packs don't go to browsers"))
        }
    }
}

/// Send `files`: the one file as it is, or several as a zip.
#[allow(clippy::too_many_arguments)]
async fn stream_download(
    shared: &Shared,
    tcp: &mut TcpStream,
    browser: Fingerprint,
    id: u64,
    name: &str,
    size: u64,
    files: Vec<(u64, String, u64, u64, Outgoing)>,
    cancel: &Cancel,
) -> Result<()> {
    // The app follows each file; cancelling any stops them all.
    let progress: Vec<Progress> = files
        .iter()
        .map(|(file, _, size, _, _)| {
            shared.track_with(*file, browser, *size, Progress::with_cancel(cancel.clone()))
        })
        .collect();
    let kind = if files.len() > 1 {
        "application/zip"
    } else {
        "application/octet-stream"
    };
    let head = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: {kind}\r\n\
         Content-Length: {size}\r\n\
         Content-Disposition: {}\r\n\
         Cache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Connection: close\r\n\r\n",
        content_disposition(name)
    );
    tcp.write_all(head.as_bytes()).await?;
    let mut wire = Wire {
        tcp,
        shared,
        browser,
        id,
        sent: 0,
        size,
        told: Instant::now(),
    };
    if files.len() == 1 {
        let (_, _, size, _, item) = files.into_iter().next().expect("one file");
        wire.file(open(item)?, size, &progress[0], None).await?;
    } else {
        let mut zip = archive_of(files.iter().map(|(_, n, s, m, _)| (n.as_str(), *s, *m)));
        for (i, (_, _, size, _, item)) in files.into_iter().enumerate() {
            wire.write(&zip.local_header(i)).await?;
            let mut crc = crc32fast::Hasher::new();
            wire.file(open(item)?, size, &progress[i], Some(&mut crc))
                .await?;
            zip.data(size);
            wire.write(&zip.descriptor(i, crc.finalize())).await?;
        }
        wire.write(&zip.finish()).await?;
    }
    if wire.sent != size {
        return Err(Error::Truncated {
            got: wire.sent,
            expected: size,
        });
    }
    wire.tcp.flush().await?;
    let _ = wire.tcp.shutdown().await;
    Ok(())
}

/// Save a file the tab sends, once the user here has accepted it (`ask`).
async fn upload(
    shared: &Arc<Shared>,
    token: &str,
    tcp: &mut TcpStream,
    session: Fingerprint,
    req: Request,
) -> std::io::Result<()> {
    if !is_browser(shared, &session) {
        return respond(tcp, "404 Not Found", "text/plain", b"Not found").await;
    }
    // Only what the user here accepted (see `ask`).
    let allowed = {
        let mut state = shared.state();
        match state
            .web
            .as_mut()
            .and_then(|w| w.browsers.get_mut(&session))
        {
            Some(tab) if tab.allowance > 0 => {
                tab.allowance -= 1;
                true
            }
            _ => false,
        }
    };
    if !allowed {
        return respond(tcp, "403 Forbidden", "text/plain", b"Not accepted").await;
    }
    let Some(size) = req.content_length else {
        return respond(tcp, "411 Length Required", "text/plain", b"Length required").await;
    };
    let Ok(name) = sanitize_file_name(req.query.get("name").map_or("", String::as_str)) else {
        return respond(tcp, "400 Bad Request", "text/plain", b"Bad file name").await;
    };
    // The page's name for this upload: it's told over the event stream once
    // the file is saved, in case this reply never reaches it.
    let key = req
        .query
        .get("k")
        .filter(|k| k.len() <= 32 && k.bytes().all(|b| b.is_ascii_alphanumeric()))
        .cloned();

    let id = shared
        .next_id
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    shared.emit(NodeEvent::TransferStarted {
        id,
        peer: session,
        direction: Direction::Incoming,
        file_name: name.clone(),
        size,
    });
    let progress = shared.track(id, session, size);
    let start = Instant::now();
    let dir = shared.download_dir();
    if let Some(tab) = shared
        .state()
        .web
        .as_mut()
        .and_then(|w| w.browsers.get_mut(&session))
    {
        tab.uploads.insert(id);
    }
    let result = async {
        tokio::fs::create_dir_all(&dir).await?;
        let part = with_suffix(&dir.join(&name), UPLOAD_SUFFIX);
        let written = receive_body(tcp, &part, req.body_start, size, &progress).await;
        if let Err(e) = written {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(e);
        }
        let path = unique_path(&dir, &name);
        tokio::fs::rename(&part, &path).await?;
        Ok(path)
    }
    .await;

    let cancelled = progress.cancel().by();
    let sharing = live(shared, token);
    if let Some(tab) = shared
        .state()
        .web
        .as_mut()
        .and_then(|w| w.browsers.get_mut(&session))
    {
        tab.uploads.remove(&id);
        if cancelled.is_some() {
            // The rest of its files aren't wanted either.
            tab.allowance = 0;
        }
        if cancelled == Some(CancelledBy::Here) && sharing {
            tab.push("cancelled", "{\"what\":\"up\",\"by\":\"them\"}");
        }
        if let (Ok(_), Some(key)) = (&result, key) {
            tab.push("received", &format!("{{\"k\":{}}}", json_str(&key)));
            tab.remember_up(key);
        }
    }
    let result = result.map_err(|e| ended_error(shared, token, e));
    let ok = result.is_ok();
    shared.finish(
        id,
        result.map(|path| {
            (
                path,
                TransferStats {
                    bytes: size,
                    resumed_from: 0,
                    elapsed: start.elapsed(),
                },
            )
        }),
    );
    if ok {
        respond(tcp, "200 OK", "application/json", b"{}").await
    } else if cancelled.is_some() {
        respond(tcp, "409 Conflict", "text/plain", b"Cancelled").await
    } else {
        respond(tcp, "500 Internal Server Error", "text/plain", b"Not saved").await
    }
}

/// Text the tab sends: onto this device's clipboard, as from a paired
/// device. The person here invited the tab by showing the code.
async fn clip(
    shared: &Arc<Shared>,
    tcp: &mut TcpStream,
    session: Fingerprint,
    req: Request,
) -> std::io::Result<()> {
    if !is_browser(shared, &session) {
        return respond(tcp, "404 Not Found", "text/plain", b"Not found").await;
    }
    let size = match req.content_length {
        Some(n) if n <= MAX_CLIP => n as usize,
        Some(_) => return respond(tcp, "413 Payload Too Large", "text/plain", b"Too long").await,
        None => return respond(tcp, "411 Length Required", "text/plain", b"Length required").await,
    };
    let mut body = req.body_start;
    body.truncate(size);
    let mut chunk = vec![0u8; 16 * 1024];
    while body.len() < size {
        let want = chunk.len().min(size - body.len());
        let n = tokio::time::timeout(Duration::from_secs(15), tcp.read(&mut chunk[..want]))
            .await
            .map_err(|_| std::io::Error::other("body timed out"))??;
        if n == 0 {
            return Ok(());
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let text = String::from_utf8_lossy(&body).into_owned();
    if text.is_empty() {
        return respond(tcp, "400 Bad Request", "text/plain", b"Empty").await;
    }
    shared.emit(NodeEvent::ClipboardReceived {
        peer: session,
        text,
    });
    respond(tcp, "200 OK", "application/json", b"{}").await
}

async fn receive_body(
    tcp: &mut TcpStream,
    part: &std::path::Path,
    first: Vec<u8>,
    size: u64,
    progress: &crate::transfer::Progress,
) -> Result<()> {
    let mut file = tokio::fs::File::create(part).await?;
    let mut got = first.len().min(size as usize) as u64;
    file.write_all(&first[..got as usize]).await?;
    progress.set(got);
    let mut buf = vec![0u8; BUFFER];
    while got < size {
        let want = buf.len().min((size - got) as usize);
        let n = tokio::select! {
            n = tcp.read(&mut buf[..want]) => n?,
            by = progress.cancel().until() => return Err(by.error()),
        };
        if n == 0 {
            return Err(Error::Truncated {
                got,
                expected: size,
            });
        }
        file.write_all(&buf[..n]).await?;
        got += n as u64;
        progress.set(got);
    }
    file.sync_all().await?;
    Ok(())
}

async fn respond(
    tcp: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    tcp.write_all(head.as_bytes()).await?;
    tcp.write_all(body).await?;
    tcp.shutdown().await?;
    linger(tcp).await;
    Ok(())
}

/// Before closing, read (and drop) what the browser is still sending, for a
/// moment. Closing with a request body unread (a refused upload, say) makes
/// the system reset the connection, and a reset can wipe out the reply
/// before the browser reads it.
async fn linger(tcp: &mut TcpStream) {
    let drain = async {
        let mut sink = [0u8; 16 * 1024];
        let mut left = 1usize << 20;
        while left > 0 {
            match tcp.read(&mut sink).await {
                Ok(0) | Err(_) => break,
                Ok(n) => left = left.saturating_sub(n),
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_millis(500), drain).await;
}

/// This device's address on the local network, as people nearby reach it.
/// Home networks first: on Windows, virtual adapters (Hyper-V, WSL) also
/// have private addresses, usually 172.16/12.
fn best_lan_ip() -> Option<IpAddr> {
    let rank = |ip: &IpAddr| match ip {
        IpAddr::V4(v4) => match v4.octets() {
            [192, 168, ..] => 0,
            [10, ..] => 1,
            [172, b, ..] if (16..32).contains(&b) => 2,
            _ if v4.is_loopback() => 9,
            _ => 3,
        },
        IpAddr::V6(_) => 9,
    };
    local_addrs()
        .into_iter()
        .filter(|ip| rank(ip) < 9)
        .min_by_key(rank)
}

/// What the tab shows up as: the kind of device, from its browser.
fn name_from_user_agent(ua: &str) -> &'static str {
    if ua.contains("iPhone") {
        "iPhone"
    } else if ua.contains("iPad") {
        "iPad"
    } else if ua.contains("Android") {
        if ua.contains("Mobile") {
            "Android phone"
        } else {
            "Android tablet"
        }
    } else if ua.contains("CrOS") {
        "Chromebook"
    } else if ua.contains("Windows") {
        "Windows PC"
    } else if ua.contains("Macintosh") {
        "Mac"
    } else if ua.contains("Linux") {
        "Linux PC"
    } else {
        "Browser"
    }
}

/// "iPhone", then "iPhone 2", ... so two of a kind can be told apart.
fn unique_name(web: &WebShare, base: &str) -> String {
    let taken = |n: &str| web.browsers.values().any(|b| b.name == n);
    if !taken(base) {
        return base.to_owned();
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|n| !taken(n))
        .expect("unbounded search always finds a free name")
}

/// Constant-time, so the secret can't be guessed a character at a time.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let digit = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match (digit(bytes[i + 1]), digit(bytes[i + 2])) {
                (Some(hi), Some(lo)) => {
                    out.push(hi * 16 + lo);
                    i += 3;
                    continue;
                }
                _ => out.push(b'%'),
            },
            b'+' => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `attachment` with the name in both forms browsers understand: plain
/// ASCII, and UTF-8 for everything else.
fn content_disposition(name: &str) -> String {
    let ascii: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut encoded = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            encoded.push(b as char);
        } else {
            let _ = write!(encoded, "%{b:02X}");
        }
    }
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Also escapes < and > so a name can never close a script tag.
            c if (c as u32) < 0x20 || c == '<' || c == '>' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_queries() {
        let q = parse_query("s=abc&name=caf%C3%A9+menu.pdf&x");
        assert_eq!(q["s"], "abc");
        assert_eq!(q["name"], "café menu.pdf");
        assert_eq!(q["x"], "");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn names_tabs_by_device() {
        let iphone = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) Safari/604.1";
        let pixel = "Mozilla/5.0 (Linux; Android 14; Pixel 8) Chrome/120 Mobile Safari/537.36";
        assert_eq!(name_from_user_agent(iphone), "iPhone");
        assert_eq!(name_from_user_agent(pixel), "Android phone");
        assert_eq!(name_from_user_agent(""), "Browser");
    }

    #[test]
    fn escapes_json_and_headers() {
        assert_eq!(
            json_str("a\"b\\c</script>"),
            "\"a\\\"b\\\\c\\u003c/script\\u003e\""
        );
        assert_eq!(
            content_disposition("café \"1\".jpg"),
            "attachment; filename=\"caf_ _1_.jpg\"; filename*=UTF-8''caf%C3%A9%20%221%22.jpg"
        );
    }

    #[test]
    fn compares_secrets() {
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "ab"));
    }
}
