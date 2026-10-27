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
use crate::identity::Fingerprint;
use crate::pairing::random_bytes;
use crate::resume::with_suffix;
use crate::transfer::{CancelledBy, TransferStats, sanitize_file_name, unique_path};
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
    /// The tab's event stream, while it's connected.
    events: Option<mpsc::UnboundedSender<String>>,
    /// Which connection `events` belongs to, so an old one closing late
    /// doesn't mark a newer one gone.
    connection: u64,
    gone_since: Option<Instant>,
    /// Offers waiting for an answer, by batch ID.
    offers: HashMap<u64, Offer>,
    /// Accepted files, downloaded one at a time in this order.
    queue: VecDeque<Download>,
    current: Option<Download>,
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
            events: None,
            connection: 0,
            gone_since: None,
            offers: HashMap::new(),
            queue: VecDeque::new(),
            current: None,
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
        let owed: Vec<String> = self
            .current
            .iter()
            .chain(&self.queue)
            .map(Download::summary)
            .collect();
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

    /// Tell the tab to fetch the next accepted file, if any.
    fn advance(&mut self) {
        if self.current.is_none() {
            self.current = self.queue.pop_front();
        }
        if let Some(d) = &self.current {
            self.push("download", &d.event());
        }
    }

    /// Everything still owed to a tab that reconnected.
    fn resend(&self) {
        self.push("sync", &self.sync_event());
        for offer in self.offers.values() {
            self.push("offer", &offer.event);
        }
        if let Some(d) = &self.current {
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

struct Download {
    id: u64,
    /// The send it's part of (its first file's ID).
    batch: u64,
    name: String,
    size: u64,
    /// Kept until it's settled, so a browser that asks again (a retry, or
    /// the page's "Download" button) can be served again.
    item: Outgoing,
    /// Requests so far; each one's number.
    attempts: u64,
    /// The request streaming it now, if any.
    streaming: Option<Streaming>,
}

/// A request streaming a download.
struct Streaming {
    attempt: u64,
    /// Tells it to stop: a newer request for the same file took over.
    stop: Arc<Notify>,
    /// Closes when it has stopped.
    done: watch::Receiver<()>,
}

impl Download {
    fn summary(&self) -> String {
        format!(
            "{{\"id\":{},\"name\":{},\"size\":{}}}",
            self.id,
            json_str(&self.name),
            self.size
        )
    }

    /// "Fetch this now"; `streaming` says if a request already is.
    fn event(&self) -> String {
        format!(
            "{{\"id\":{},\"name\":{},\"size\":{},\"streaming\":{}}}",
            self.id,
            json_str(&self.name),
            self.size,
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
    let mut moving = Vec::new();
    for tab in web.browsers.values() {
        tab.push("stopped", &data);
        moving.extend(tab.current.iter().filter(|d| d.streaming.is_some()).map(|d| d.id));
        moving.extend(tab.uploads.iter().copied());
    }
    fail_leftovers(shared, std::mem::take(&mut web.browsers).into_values());
    // Downloads and uploads under way end as "sharing was stopped" (see
    // `ended_error`).
    for id in moving {
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

/// Offer files to a browser and, once accepted, queue them for download.
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
                let items = jobs.into_iter().map(|(_, item)| item);
                for ((id, name, size), item) in files.iter().zip(items) {
                    tab.queue.push_back(Download {
                        id: *id,
                        batch,
                        name: name.clone(),
                        size: *size,
                        item,
                        attempts: 0,
                        streaming: None,
                    });
                }
                tab.advance();
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

/// Fail what removed tabs still had queued. Unanswered offers fail on their
/// own when their answer channel drops.
fn fail_leftovers(shared: &Shared, gone: impl Iterator<Item = Browser>) {
    for tab in gone {
        for d in tab.current.into_iter().chain(tab.queue) {
            // A download in progress finishes (or fails) by itself.
            if d.streaming.is_none() {
                shared.finish(d.id, Err(Error::BrowserGone));
            }
        }
    }
}

/// Stop the send to a tab that file `id` is part of: still waiting for an
/// answer, queued, or downloading. False if `id` isn't one.
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
            let batch = tab
                .current
                .iter()
                .chain(&tab.queue)
                .find(|d| d.id == id)
                .map(|d| d.batch);
            if let Some(batch) = batch {
                found = Some(stop_downloads(tab, Some(batch), by));
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

/// Downloads stopped: those not started yet, and the one under way.
struct Stopped {
    unstarted: Vec<u64>,
    streaming: Option<u64>,
}

impl Stopped {
    fn finish(self, shared: &Shared, by: CancelledBy) {
        for id in self.unstarted {
            shared.finish(id, Err(by.error()));
        }
        if let Some(id) = self.streaming {
            let progress = shared.state().active.get(&id).map(|a| a.progress.clone());
            if let Some(p) = progress {
                p.cancel().cancel(by);
            }
        }
    }
}

/// Take a tab's downloads of `batch` (or all of them) off its list, and
/// tell it. The one downloading now ends when its stream does.
fn stop_downloads(tab: &mut Browser, batch: Option<u64>, by: CancelledBy) -> Stopped {
    let ours = |d: &Download| batch.is_none_or(|b| d.batch == b);
    let mut stopped = Stopped {
        unstarted: Vec::new(),
        streaming: None,
    };
    let mut ids = Vec::new();
    tab.queue.retain(|d| {
        if ours(d) {
            stopped.unstarted.push(d.id);
            ids.push(d.id);
        }
        !ours(d)
    });
    if let Some(d) = tab.current.take_if(|d| ours(d)) {
        // Off the list either way, so a retry can't pick it up again; the
        // request streaming it ends when its progress is cancelled.
        ids.push(d.id);
        if d.streaming.is_none() {
            stopped.unstarted.push(d.id);
        } else {
            stopped.streaming = Some(d.id);
        }
    }
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
    tab.advance();
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
        ("GET", ["events"]) => events(shared, &mut tcp, session, &req.user_agent).await,
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
    tcp: &mut TcpStream,
    session: Option<Fingerprint>,
    user_agent: &str,
) -> std::io::Result<()> {
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
        web.idle_since = None;
        tab.push(
            "hello",
            &format!(
                "{{\"session\":\"{id}\",\"from\":{},\"platform\":{},\"name\":{}}}",
                json_str(&shared.device_name),
                json_str(&shared.platform),
                json_str(&tab.name)
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
    match offer {
        Some(offer) => offer.answer.send(Some(accept)).is_ok(),
        None => false,
    }
}

/// Stream an accepted file to the tab: only the one it was last told to
/// fetch. Asked for again while it's still that one (the browser retrying a
/// broken download, or the page's Download button), the newer request
/// takes over from the older.
async fn download(
    shared: &Arc<Shared>,
    token: &str,
    tcp: &mut TcpStream,
    session: Fingerprint,
    id: u64,
) -> std::io::Result<()> {
    let claimed = {
        let mut state = shared.state();
        let tab = state
            .web
            .as_mut()
            .and_then(|w| w.browsers.get_mut(&session));
        tab.and_then(|tab| {
            let d = tab.current.as_mut().filter(|d| d.id == id)?;
            let item = d.item.try_clone().ok()?;
            d.attempts += 1;
            let stop = Arc::new(Notify::new());
            let (done, done_rx) = watch::channel(());
            let previous = d.streaming.replace(Streaming {
                attempt: d.attempts,
                stop: stop.clone(),
                done: done_rx,
            });
            let claim = (item, d.name.clone(), d.attempts, previous, stop, done);
            tab.push("started", &format!("{{\"id\":{id}}}"));
            Some(claim)
        })
    };
    let Some((item, name, attempt, previous, stop, _done)) = claimed else {
        return respond(tcp, "404 Not Found", "text/plain", b"Not found").await;
    };
    if let Some(previous) = previous {
        // Wait for the older request to let go of the file (its `done`
        // closes), so the two never read it at once.
        previous.stop.notify_one();
        let mut done = previous.done;
        let gone = async { while done.changed().await.is_ok() {} };
        let _ = tokio::time::timeout(Duration::from_secs(5), gone).await;
    }

    let start = Instant::now();
    let streamed = tokio::select! {
        r = stream_file(shared, tcp, session, id, item, &name) => r,
        // A newer request took over; it settles the file.
        () = stop.notified() => return Ok(()),
    };
    let result = streamed.map(|bytes| {
        (
            PathBuf::from(&name),
            TransferStats {
                bytes,
                resumed_from: 0,
                elapsed: start.elapsed(),
            },
        )
    });
    match result {
        // Broken off (not cancelled, and still sharing): the browser may ask
        // again. Leave it on the list for a moment before failing it.
        Err(e) if !matches!(e, Error::Cancelled | Error::CancelledThere) && live(shared, token) => {
            let wait = {
                let mut state = shared.state();
                current_of(&mut state, session, id).map(|d| {
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
                        settle(&shared, session, id, attempt, Err(e), true);
                    });
                }
                // A newer request has it.
                Some(false) => {}
                // Off the list already (the tab went): just fail it.
                None => shared.finish(id, Err(e)),
            }
        }
        result => {
            let result = result.map_err(|e| ended_error(shared, token, e));
            settle(shared, session, id, attempt, result, false);
        }
    }
    Ok(())
}

/// The download `id`, if it's the one `session` is fetching now.
fn current_of(state: &mut super::State, session: Fingerprint, id: u64) -> Option<&mut Download> {
    state
        .web
        .as_mut()
        .and_then(|w| w.browsers.get_mut(&session))
        .and_then(|tab| tab.current.as_mut())
        .filter(|d| d.id == id)
}

/// A download is over: tell the app and the tab, and move on to the next.
/// `retried_out`: called after `RETRY_WINDOW`, so only if nobody asked
/// for it again meanwhile and it wasn't settled some other way.
fn settle(
    shared: &Shared,
    session: Fingerprint,
    id: u64,
    attempt: u64,
    result: Result<(PathBuf, TransferStats)>,
    retried_out: bool,
) {
    let ok = result.is_ok();
    {
        let mut state = shared.state();
        let tab = state
            .web
            .as_mut()
            .and_then(|w| w.browsers.get_mut(&session))
            .filter(|tab| tab.current.as_ref().is_some_and(|d| d.id == id));
        match tab {
            Some(tab) => {
                let unclaimed = tab
                    .current
                    .as_ref()
                    .is_some_and(|d| d.attempts == attempt && d.streaming.is_none());
                if retried_out && !unclaimed {
                    return;
                }
                let d = tab.current.take().expect("checked above");
                // An older request still going (this one overtook it and
                // won): stop it quietly.
                if let Some(other) = d.streaming.filter(|s| s.attempt != attempt) {
                    other.stop.notify_one();
                }
                tab.remember_down(id, ok);
                let event = if ok { "delivered" } else { "failed" };
                tab.push(event, &format!("{{\"id\":{id}}}"));
                tab.advance();
            }
            // Settled some other way already (cancelled, or the tab went).
            None if retried_out => return,
            None => {}
        }
    }
    shared.finish(id, result);
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

async fn stream_file(
    shared: &Shared,
    tcp: &mut TcpStream,
    browser: Fingerprint,
    id: u64,
    item: Outgoing,
    name: &str,
) -> Result<u64> {
    let file = match item {
        Outgoing::Path(path) | Outgoing::InFolder { path, .. } => std::fs::File::open(path)?,
        Outgoing::Open { file, .. } => file,
        // Never made for a browser (see Node::send).
        Outgoing::Pack(_) | Outgoing::EmptyFolder { .. } => {
            return Err(Error::Protocol("packs don't go to browsers"));
        }
    };
    let size = file.metadata()?.len();
    let progress = shared.track(id, browser, size);
    let file = Arc::new(file);

    let head = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/octet-stream\r\n\
         Content-Length: {size}\r\n\
         Content-Disposition: {}\r\n\
         Cache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Connection: close\r\n\r\n",
        content_disposition(name)
    );
    tcp.write_all(head.as_bytes()).await?;
    let mut buf = vec![0u8; BUFFER];
    let mut sent = 0u64;
    let mut told = Instant::now();
    while sent < size {
        let step = async {
            let (back, n) = read_chunk(file.clone(), std::mem::take(&mut buf), sent).await?;
            buf = back;
            let n = n.min((size - sent) as usize);
            // Written in pieces, so a slow browser isn't mistaken for a
            // stalled one: only no progress at all for `STALL_LIMIT` is.
            let mut off = 0;
            while off < n {
                let wrote = tokio::time::timeout(STALL_LIMIT, tcp.write(&buf[off..n]))
                    .await
                    .map_err(|_| Error::BrowserStalled)??;
                if wrote == 0 {
                    return Err(std::io::Error::from(std::io::ErrorKind::WriteZero).into());
                }
                off += wrote;
                progress.set(sent + off as u64);
            }
            Ok::<usize, Error>(n)
        };
        let n = tokio::select! {
            n = step => n?,
            by = progress.cancel().until() => return Err(by.error()),
        };
        if n == 0 {
            break;
        }
        sent += n as u64;
        // The page shows the same progress as the app.
        if told.elapsed() >= PAGE_PROGRESS || sent == size {
            told = Instant::now();
            if let Some(tab) = shared
                .state()
                .web
                .as_ref()
                .and_then(|w| w.browsers.get(&browser))
            {
                tab.push(
                    "progress",
                    &format!("{{\"id\":{id},\"bytes\":{sent},\"size\":{size}}}"),
                );
            }
        }
    }
    if sent < size {
        return Err(Error::Truncated {
            got: sent,
            expected: size,
        });
    }
    tcp.flush().await?;
    let _ = tcp.shutdown().await;
    Ok(sent)
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
    tcp.shutdown().await
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
