//! Sharing with a browser, for people without Nudge.
//!
//! While on, the node serves a small web page on the local network. Someone
//! who scans the QR code opens it, and while the tab stays open they appear
//! as a device. Files sent to them wait for them to tap Accept, then
//! download straight from this device. They can send files back from the
//! same page; those are accepted without asking, since the user invited
//! them by showing the code.
//!
//! Plain HTTP: a browser can't trust Nudge's self-signed certificate
//! without a scary warning. The only protection is the unguessable secret
//! in the link, so the page is meant for a trusted network.
//! See docs/PROTOCOL.md ("Browser sharing").

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::io::SeekFrom;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use super::{Direction, NodeEvent, Outgoing, Shared, Verdict, local_addrs};
use crate::identity::Fingerprint;
use crate::pairing::random_bytes;
use crate::resume::with_suffix;
use crate::transfer::{TransferStats, sanitize_file_name, unique_path};
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

/// Keeps the event stream alive, and notices when a tab has gone.
const PING_INTERVAL: Duration = Duration::from_secs(15);

const MAX_HEADER: usize = 16 * 1024;
const BUFFER: usize = 1024 * 1024;
const UPLOAD_SUFFIX: &str = ".nudge-upload";

const PAGE: &str = include_str!("web_page.html");

/// The running share: its link, and the browsers that opened it.
pub(super) struct WebShare {
    pub(super) url: String,
    pub(super) browsers: HashMap<Fingerprint, Browser>,
    idle_since: Option<Instant>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for WebShare {
    fn drop(&mut self) {
        self.tasks.iter().for_each(JoinHandle::abort);
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
    /// "can I send you these?" question.
    allowance: u32,
    /// The user chose "always accept" for this tab.
    always: bool,
}

impl Browser {
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
    answer: oneshot::Sender<bool>,
}

struct Download {
    id: u64,
    name: String,
    /// Taken when the tab starts downloading.
    item: Option<Outgoing>,
}

impl Download {
    fn event(&self) -> String {
        format!("{{\"id\":{},\"name\":{}}}", self.id, json_str(&self.name))
    }
}

/// Start sharing, or return the link if already sharing.
pub(super) async fn start(shared: &Arc<Shared>) -> Result<String> {
    if let Some(web) = &shared.state().web {
        return Ok(web.url.clone());
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
    let token = hex(&random_bytes::<16>());
    let url = format!("http://{host}:{port}/t/{token}");

    let mut state = shared.state();
    if let Some(web) = &state.web {
        // Started by another call meanwhile.
        return Ok(web.url.clone());
    }
    state.web = Some(WebShare {
        url: url.clone(),
        browsers: HashMap::new(),
        idle_since: Some(Instant::now()),
        tasks: vec![
            tokio::spawn(accept_loop(shared.clone(), listener, token.into())),
            tokio::spawn(watchdog(shared.clone())),
        ],
    });
    Ok(url)
}

/// Stop sharing. Open tabs lose their place; unanswered offers fail.
pub(super) fn stop(shared: &Shared) {
    let web = shared.state().web.take();
    if let Some(web) = web {
        let mut web = web;
        fail_leftovers(shared, std::mem::take(&mut web.browsers).into_values());
        shared.emit(NodeEvent::WebShareStopped);
        shared.emit(NodeEvent::PeersChanged);
    }
}

/// "Always accept" for a tab: for as long as it's open.
pub(super) fn always_accept(shared: &Shared, id: &Fingerprint) {
    if let Some(tab) = shared
        .state()
        .web
        .as_mut()
        .and_then(|w| w.browsers.get_mut(id))
    {
        tab.always = true;
    }
}

pub(super) fn always_accepts(shared: &Shared, id: &Fingerprint) -> bool {
    shared
        .state()
        .web
        .as_ref()
        .and_then(|w| w.browsers.get(id))
        .is_some_and(|tab| tab.always)
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
            tab.offers.insert(batch, Offer { event, answer });
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
            (Ok(Ok(true)), Some(tab)) => {
                for (id, _, _) in &files {
                    shared.emit(NodeEvent::TransferAccepted { id: *id });
                }
                let mut items = jobs.into_iter().map(|(_, item)| item);
                for (id, name, _) in &files {
                    tab.queue.push_back(Download {
                        id: *id,
                        name: name.clone(),
                        item: items.next(),
                    });
                }
                tab.advance();
            }
            (Ok(Ok(false)), _) => {
                drop(state);
                fail_all(&shared, &files, || Error::Declined);
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
            if d.item.is_some() {
                shared.finish(d.id, Err(Error::BrowserGone));
            }
        }
    }
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
            stop(&shared);
            return;
        }
    }
}

async fn accept_loop(shared: Arc<Shared>, listener: TcpListener, token: Arc<str>) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let _ = tcp.set_nodelay(true);
        let shared = shared.clone();
        let token = token.clone();
        tokio::spawn(async move {
            let _ = handle(&shared, &token, tcp).await;
        });
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
    if !authorized {
        return respond(&mut tcp, "404 Not Found", "text/plain", b"Not found").await;
    }
    let route: Vec<&str> = parts.collect();
    let session = req
        .query
        .get("s")
        .and_then(|s| s.parse::<Fingerprint>().ok());
    match (req.method.as_str(), route.as_slice()) {
        ("GET", []) => page(&mut tcp).await,
        ("GET", ["events"]) => events(shared, &mut tcp, session, &req.user_agent).await,
        ("POST", ["answer"]) => {
            let ok = answer(shared, session, &req.query);
            let status = if ok { "200 OK" } else { "404 Not Found" };
            respond(&mut tcp, status, "application/json", b"{}").await
        }
        ("GET", ["f", id]) => match (session, id.parse::<u64>()) {
            (Some(s), Ok(id)) => download(shared, &mut tcp, s, id).await,
            _ => respond(&mut tcp, "404 Not Found", "text/plain", b"Not found").await,
        },
        ("POST", ["ask"]) => match session {
            Some(s) => ask(shared, &mut tcp, s, &req.query).await,
            None => respond(&mut tcp, "404 Not Found", "text/plain", b"Not found").await,
        },
        ("POST", ["u"]) => match session {
            Some(s) => upload(shared, &mut tcp, s, req).await,
            None => respond(&mut tcp, "404 Not Found", "text/plain", b"Not found").await,
        },
        _ => respond(&mut tcp, "404 Not Found", "text/plain", b"Not found").await,
    }
}

/// "Can I send you these?" from a tab: the user here accepts or declines
/// (or it's accepted at once, for a tab set to always accept). The reply,
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
        super::Asking::Wait(key, rx) => shared.wait_answer(key, rx).await,
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
    };
    respond(
        tcp,
        "200 OK",
        "application/json",
        format!("{{\"answer\":\"{word}\"}}").as_bytes(),
    )
    .await
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
                web.browsers.insert(
                    id,
                    Browser {
                        name,
                        events: None,
                        connection: 0,
                        gone_since: None,
                        offers: HashMap::new(),
                        queue: VecDeque::new(),
                        current: None,
                        allowance: 0,
                        always: false,
                    },
                );
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
                _ = ping.tick() => tcp.write_all(b": ping\n\n").await?,
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
        Some(offer) => offer.answer.send(accept).is_ok(),
        None => false,
    }
}

/// Stream an accepted file to the tab.
async fn download(
    shared: &Arc<Shared>,
    tcp: &mut TcpStream,
    session: Fingerprint,
    id: u64,
) -> std::io::Result<()> {
    let taken = {
        let mut state = shared.state();
        state
            .web
            .as_mut()
            .and_then(|w| w.browsers.get_mut(&session))
            .and_then(|tab| tab.current.as_mut())
            .filter(|d| d.id == id)
            .and_then(|d| d.item.take().map(|item| (item, d.name.clone())))
    };
    let Some((item, name)) = taken else {
        return respond(tcp, "404 Not Found", "text/plain", b"Not found").await;
    };

    let start = Instant::now();
    let result = stream_file(shared, tcp, session, id, item, &name).await;
    let delivered = result.is_ok();
    shared.finish(
        id,
        result.map(|bytes| {
            (
                PathBuf::from(&name),
                TransferStats {
                    bytes,
                    resumed_from: 0,
                    elapsed: start.elapsed(),
                },
            )
        }),
    );

    let mut state = shared.state();
    if let Some(tab) = state
        .web
        .as_mut()
        .and_then(|w| w.browsers.get_mut(&session))
        && tab.current.as_ref().is_some_and(|d| d.id == id)
    {
        if delivered {
            tab.push("delivered", &format!("{{\"id\":{id}}}"));
        }
        tab.current = None;
        tab.advance();
    }
    Ok(())
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
    let mut file = tokio::fs::File::from_std(file);
    file.seek(SeekFrom::Start(0)).await?;

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
    while sent < size {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let n = n.min((size - sent) as usize);
        tcp.write_all(&buf[..n]).await?;
        sent += n as u64;
        progress.set(sent);
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
    } else {
        respond(tcp, "500 Internal Server Error", "text/plain", b"Not saved").await
    }
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
        let n = tcp.read(&mut buf[..want]).await?;
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
