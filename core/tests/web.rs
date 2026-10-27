//! Sharing with a browser, driven over plain HTTP the way the page does it.

use std::net::Ipv4Addr;
use std::time::Duration;

use nudge_core::discovery::DiscoveryConfig;
use nudge_core::{Error, Fingerprint, Node, NodeConfig, NodeEvent};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::UnboundedReceiver;

const IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) Safari/604.1";

async fn start() -> (Node, UnboundedReceiver<NodeEvent>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = NodeConfig::new(
        dir.path().join("id"),
        dir.path().join("downloads"),
        "Laptop",
        "windows",
    );
    config.port = Some(0);
    config.bind = Ipv4Addr::LOCALHOST.into();
    config.discovery = DiscoveryConfig {
        mdns: false,
        broadcast: false,
        ..DiscoveryConfig::default()
    };
    let (node, events) = Node::start(config).await.unwrap();
    (node, events, dir)
}

/// "http://127.0.0.1:1234/t/abc" -> ("127.0.0.1:1234", "/t/abc")
fn split(url: &str) -> (String, String) {
    let rest = url.strip_prefix("http://").unwrap();
    let slash = rest.find('/').unwrap();
    (rest[..slash].to_owned(), rest[slash..].to_owned())
}

/// One request; returns the status code and body.
async fn http(addr: &str, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let mut tcp = TcpStream::connect(addr).await.unwrap();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nUser-Agent: {IPHONE}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    tcp.write_all(head.as_bytes()).await.unwrap();
    tcp.write_all(body).await.unwrap();
    let mut response = Vec::new();
    tcp.read_to_end(&mut response).await.unwrap();
    let end = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let status = std::str::from_utf8(&response[9..12])
        .unwrap()
        .parse()
        .unwrap();
    (status, response[end + 4..].to_vec())
}

/// The page's event stream.
struct Events {
    tcp: TcpStream,
    buf: String,
}

impl Events {
    /// Like the page: `session` is the one it was given before, if any.
    async fn open(addr: &str, path: &str, session: Option<&str>) -> Self {
        let mut tcp = TcpStream::connect(addr).await.unwrap();
        let query = session.map(|s| format!("?s={s}")).unwrap_or_default();
        let head = format!(
            "GET {path}/events{query} HTTP/1.1\r\nHost: {addr}\r\nUser-Agent: {IPHONE}\r\n\r\n"
        );
        tcp.write_all(head.as_bytes()).await.unwrap();
        Self {
            tcp,
            buf: String::new(),
        }
    }

    /// The data of the next event called `name`.
    async fn next(&mut self, name: &str) -> String {
        let marker = format!("event: {name}\ndata: ");
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(start) = self.buf.find(&marker) {
                    let data_start = start + marker.len();
                    if let Some(len) = self.buf[data_start..].find("\n\n") {
                        let data = self.buf[data_start..data_start + len].to_owned();
                        self.buf.drain(..data_start + len + 2);
                        return data;
                    }
                }
                let mut chunk = [0u8; 4096];
                let n = self.tcp.read(&mut chunk).await.unwrap();
                assert!(n > 0, "event stream closed");
                self.buf.push_str(std::str::from_utf8(&chunk[..n]).unwrap());
            }
        })
        .await
        .unwrap_or_else(|_| panic!("no {name} event"))
    }
}

/// The value of `"key":` in a flat bit of JSON.
fn field(json: &str, key: &str) -> String {
    let start = json.find(&format!("\"{key}\":")).unwrap() + key.len() + 3;
    json[start..]
        .trim_start_matches('"')
        .split(['"', ',', '}', ']'])
        .next()
        .unwrap()
        .to_owned()
}

async fn wait_for<T>(
    events: &mut UnboundedReceiver<NodeEvent>,
    mut pick: impl FnMut(&NodeEvent) -> Option<T>,
) -> T {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(t) = pick(&events.recv().await.unwrap()) {
                return t;
            }
        }
    })
    .await
    .expect("timed out waiting for event")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn browser_receives_and_sends() {
    let (node, mut events, dir) = start().await;
    let url = node.start_web_share().await.unwrap();
    assert_eq!(
        node.start_web_share().await.unwrap(),
        url,
        "same link while on"
    );
    assert_eq!(node.web_share_url(), Some(url.clone()));
    let (addr, path) = split(&url);

    // The page loads only with the secret.
    let (status, page) = http(&addr, "GET", &path, b"").await;
    assert_eq!(status, 200);
    assert!(String::from_utf8_lossy(&page).contains("Nudge"));
    let wrong = format!("/t/{}", "0".repeat(32));
    assert_eq!(http(&addr, "GET", &wrong, b"").await.0, 404);
    assert_eq!(
        http(&addr, "GET", &format!("{wrong}/events"), b"").await.0,
        404
    );

    // Opening it makes the tab a device.
    let mut tab = Events::open(&addr, &path, None).await;
    let hello = tab.next("hello").await;
    let session = field(&hello, "session");
    assert_eq!(field(&hello, "from"), "Laptop");
    assert_eq!(field(&hello, "name"), "iPhone");
    assert_eq!(field(&hello, "platform"), "windows");
    let id: Fingerprint = session.parse().unwrap();
    let peer = node
        .peers()
        .into_iter()
        .find(|p| p.fingerprint == id)
        .unwrap();
    assert_eq!(
        (peer.device_name.as_str(), peer.platform.as_str()),
        ("iPhone", "browser")
    );
    assert!(peer.online && !peer.paired);

    // Send two files: one offer, then one download, a zip.
    let a = dir.path().join("a.bin");
    let b = dir.path().join("b é.txt");
    let a_data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&a, &a_data).unwrap();
    std::fs::write(&b, b"second").unwrap();
    let ids = node.send_files(id, vec![a, b]);
    let offer = tab.next("offer").await;
    assert!(
        offer.contains("a.bin") && offer.contains("b é.txt"),
        "{offer}"
    );
    let batch = field(&offer, "batch");
    let answer = format!("{path}/answer?s={session}&batch={batch}&accept=1");
    assert_eq!(http(&addr, "POST", &answer, b"").await.0, 200);
    let next = tab.next("download").await;
    assert_eq!(field(&next, "id"), ids[0].to_string());
    assert_eq!(field(&next, "name"), "2 files from Laptop.zip");
    let (status, body) = http(
        &addr,
        "GET",
        &format!("{path}/f/{}?s={session}", ids[0]),
        b"",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body.len().to_string(), field(&next, "size"));
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(body)).unwrap();
    for (i, (name, expected)) in [("a.bin", &a_data[..]), ("b é.txt", b"second")]
        .into_iter()
        .enumerate()
    {
        let mut entry = zip.by_index(i).unwrap();
        assert_eq!(entry.name(), name);
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut data).unwrap(); // checks the CRC
        assert!(data == expected);
    }
    // The page hears when all of it has gone; the app, for each file.
    assert_eq!(field(&tab.next("delivered").await, "id"), ids[0].to_string());
    for expected_id in &ids {
        wait_for(&mut events, |e| match e {
            NodeEvent::TransferDone { id, .. } if id == expected_id => Some(()),
            NodeEvent::TransferFailed { error, .. } => panic!("send failed: {error}"),
            _ => None,
        })
        .await;
    }
    // A file can't be fetched twice.
    let again = format!("{path}/f/{}?s={session}", ids[0]);
    assert_eq!(http(&addr, "GET", &again, b"").await.0, 404);

    // Declining fails the transfer.
    let c = dir.path().join("c.txt");
    std::fs::write(&c, b"no thanks").unwrap();
    node.send_files(id, vec![c]);
    let batch = field(&tab.next("offer").await, "batch");
    let answer = format!("{path}/answer?s={session}&batch={batch}&accept=0");
    assert_eq!(http(&addr, "POST", &answer, b"").await.0, 200);
    let error = wait_for(&mut events, |e| match e {
        NodeEvent::TransferFailed { error, .. } => Some(error.clone()),
        _ => None,
    })
    .await;
    assert_eq!(error, Error::Declined.to_string());

    // The tab sends a file back. Not without asking first.
    let upload = format!("{path}/u?s={session}&name=from%20phone%2F..%2Fx.txt");
    assert_eq!(http(&addr, "POST", &upload, b"hello laptop").await.0, 403);
    // It asks; the user here is asked; once they accept, it goes.
    let ask_path = format!("{path}/ask?s={session}&count=1&size=12&name=x.txt");
    let (ask_addr, ask) = (addr.clone(), ask_path.clone());
    let asking = tokio::spawn(async move { http(&ask_addr, "POST", &ask, b"").await });
    let (question, count, size) = wait_for(&mut events, |e| match e {
        NodeEvent::OfferReceived {
            id, count, size, ..
        } => Some((*id, *count, *size)),
        _ => None,
    })
    .await;
    assert_eq!((count, size), (1, 12));
    node.answer_offer(question, true, false).unwrap();
    let (status, body) = asking.await.unwrap();
    assert_eq!(status, 200);
    assert!(String::from_utf8_lossy(&body).contains("accepted"));
    assert_eq!(http(&addr, "POST", &upload, b"hello laptop").await.0, 200);
    let saved = wait_for(&mut events, |e| match e {
        NodeEvent::TransferDone { path, .. } => Some(path.clone()),
        NodeEvent::TransferFailed { error, .. } => panic!("upload failed: {error}"),
        _ => None,
    })
    .await;
    assert_eq!(
        saved,
        dir.path().join("downloads").join("from phone_.._x.txt")
    );
    assert_eq!(std::fs::read(&saved).unwrap(), b"hello laptop");
    // One accepted file is one upload.
    assert_eq!(http(&addr, "POST", &upload, b"again").await.0, 403);
    // Declined: nothing may be sent.
    let (ask_addr, ask) = (addr.clone(), ask_path.clone());
    let asking = tokio::spawn(async move { http(&ask_addr, "POST", &ask, b"").await });
    let question = wait_for(&mut events, |e| match e {
        NodeEvent::OfferReceived { id, .. } => Some(*id),
        _ => None,
    })
    .await;
    node.answer_offer(question, false, false).unwrap();
    assert!(String::from_utf8_lossy(&asking.await.unwrap().1).contains("declined"));
    assert_eq!(http(&addr, "POST", &upload, b"x").await.0, 403);
    // Not without a session.
    let stranger = format!("{path}/u?s={}&name=x.txt", "1".repeat(64));
    assert_eq!(http(&addr, "POST", &stranger, b"x").await.0, 404);

    // Stopping closes the tab's stream and the page.
    node.stop_web_share();
    wait_for(&mut events, |e| {
        matches!(e, NodeEvent::WebShareStopped).then_some(())
    })
    .await;
    assert!(node.web_share_url().is_none());
    assert!(node.peers().iter().all(|p| p.fingerprint != id));
    // The tab is told why, then its stream closes.
    assert_eq!(field(&tab.next("stopped").await, "why"), "user");
    let mut rest = Vec::new();
    let closed = tokio::time::timeout(Duration::from_secs(5), tab.tcp.read_to_end(&mut rest)).await;
    assert!(closed.is_ok(), "event stream still open");
}

/// A stopped link says so (so a tab can tell "stopped" from "out of
/// reach"), until sharing starts again with a new link.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stopped_link_says_it_stopped() {
    let (node, _events, _dir) = start().await;
    let url = node.start_web_share().await.unwrap();
    let (addr, path) = split(&url);
    assert_eq!(
        http(&addr, "GET", &format!("{path}/ping"), b"").await.0,
        200
    );

    node.stop_web_share();
    let (status, body) = http(&addr, "GET", &format!("{path}/ping"), b"").await;
    assert_eq!(status, 410);
    assert!(String::from_utf8_lossy(&body).contains("\"stopped\":\"user\""));
    assert_eq!(
        http(&addr, "GET", &format!("{path}/events"), b"").await.0,
        410
    );
    // The page still loads, to show that sharing stopped.
    assert_eq!(http(&addr, "GET", &path, b"").await.0, 200);
    // Other links are still nothing.
    let wrong = format!("/t/{}/ping", "0".repeat(32));
    assert_eq!(http(&addr, "GET", &wrong, b"").await.0, 404);

    // Sharing again: a new link; the old one is unknown there. (Tests run
    // side by side and share the usual port, so the new link may be on
    // another.)
    let again = node.start_web_share().await.unwrap();
    let (new_addr, new_path) = split(&again);
    assert_ne!(new_path, path);
    assert_eq!(
        http(&new_addr, "GET", &format!("{new_path}/ping"), b"").await.0,
        200
    );
    if new_addr == addr {
        assert_eq!(
            http(&addr, "GET", &format!("{path}/ping"), b"").await.0,
            404
        );
    }
}

/// A browser that asks for a file again mid-download (a retry) gets all of
/// it, and the app hears it finish once, so it never sticks at "sending".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retried_download_takes_over() {
    let (node, mut events, dir) = start().await;
    let url = node.start_web_share().await.unwrap();
    let (addr, path) = split(&url);
    let mut tab = Events::open(&addr, &path, None).await;
    let session = field(&tab.next("hello").await, "session");
    let id: Fingerprint = session.parse().unwrap();
    let big = dir.path().join("big.bin");
    let data: Vec<u8> = (0..(64u32 << 20)).map(|i| (i % 253) as u8).collect();
    std::fs::write(&big, &data).unwrap();

    let ids = node.send_files(id, vec![big]);
    let batch = field(&tab.next("offer").await, "batch");
    let answer = format!("{path}/answer?s={session}&batch={batch}&accept=1");
    assert_eq!(http(&addr, "POST", &answer, b"").await.0, 200);
    assert!(tab.next("download").await.contains("\"streaming\":false"));

    // The first request takes a little, then stops reading.
    let mut first = TcpStream::connect(&addr).await.unwrap();
    let head = format!(
        "GET {path}/f/{}?s={session} HTTP/1.1\r\nHost: {addr}\r\n\r\n",
        ids[0]
    );
    first.write_all(head.as_bytes()).await.unwrap();
    let mut some = vec![0u8; 1 << 20];
    first.read_exact(&mut some).await.unwrap();
    tab.next("started").await;

    // The retry gets the whole file.
    let (status, body) = http(&addr, "GET", &format!("{path}/f/{}?s={session}", ids[0]), b"").await;
    assert_eq!(status, 200);
    assert!(body == data, "retry got {} bytes", body.len());
    assert_eq!(field(&tab.next("delivered").await, "id"), ids[0].to_string());
    wait_for(&mut events, |e| match e {
        NodeEvent::TransferDone { id, .. } if *id == ids[0] => Some(()),
        NodeEvent::TransferFailed { error, .. } => panic!("send failed: {error}"),
        _ => None,
    })
    .await;
    // The first request was dropped, not left hanging.
    let mut rest = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(5), first.read_to_end(&mut rest)).await;
    assert!(read.is_ok(), "first request still open");
    // Settled for good: no third time, and no second result.
    assert_eq!(
        http(&addr, "GET", &format!("{path}/f/{}?s={session}", ids[0]), b"").await.0,
        404
    );
    let late = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match events.recv().await {
                Some(NodeEvent::TransferDone { id, .. } | NodeEvent::TransferFailed { id, .. })
                    if id == ids[0] =>
                {
                    return;
                }
                Some(_) => {}
                None => std::future::pending().await,
            }
        }
    })
    .await;
    assert!(late.is_err(), "finished twice");
}

/// A tab that reconnects hears where things stand: what's still coming and
/// how recent files ended, even if it missed those events.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reconnecting_tab_is_brought_up_to_date() {
    let (node, mut events, dir) = start().await;
    let url = node.start_web_share().await.unwrap();
    let (addr, path) = split(&url);
    let mut tab = Events::open(&addr, &path, None).await;
    let session = field(&tab.next("hello").await, "session");
    let id: Fingerprint = session.parse().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    std::fs::write(&a, b"first").unwrap();
    std::fs::write(&b, b"second").unwrap();

    let mut ids = Vec::new();
    for file in [a, b] {
        ids.extend(node.send_files(id, vec![file]));
        let batch = field(&tab.next("offer").await, "batch");
        let answer = format!("{path}/answer?s={session}&batch={batch}&accept=1");
        assert_eq!(http(&addr, "POST", &answer, b"").await.0, 200);
        tab.next("download").await;
    }
    // The tab drops off; its first file arrives meanwhile.
    drop(tab);
    let (status, _) = http(&addr, "GET", &format!("{path}/f/{}?s={session}", ids[0]), b"").await;
    assert_eq!(status, 200);
    wait_for(&mut events, |e| match e {
        NodeEvent::TransferDone { id, .. } if *id == ids[0] => Some(()),
        _ => None,
    })
    .await;

    let mut tab = Events::open(&addr, &path, Some(&session)).await;
    tab.next("hello").await;
    let sync = tab.next("sync").await;
    assert!(
        sync.contains(&format!("\"done\":[{{\"id\":{},\"ok\":true}}]", ids[0])),
        "{sync}"
    );
    assert!(
        sync.contains(&format!("\"owed\":[{{\"id\":{},\"name\":\"b.txt\"", ids[1])),
        "{sync}"
    );
    assert_eq!(field(&tab.next("download").await, "id"), ids[1].to_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clipboard_goes_both_ways_and_every_send_is_asked() {
    let (node, mut events, _dir) = start().await;
    let url = node.start_web_share().await.unwrap();
    let (addr, path) = split(&url);
    let mut tab = Events::open(&addr, &path, None).await;
    let session = field(&tab.next("hello").await, "session");
    let id: Fingerprint = session.parse().unwrap();

    // App -> tab: shown on the page.
    node.send_clipboard(id, "https://example.com/\"x\"".into())
        .await
        .unwrap();
    let clip = tab.next("clip").await;
    assert!(clip.contains("https://example.com/\\\"x\\\""), "{clip}");

    // Tab -> app: onto the clipboard, from the tab.
    let clip_path = format!("{path}/clip?s={session}");
    assert_eq!(
        http(&addr, "POST", &clip_path, "héllo".as_bytes()).await.0,
        200
    );
    let (from, text) = wait_for(&mut events, |e| match e {
        NodeEvent::ClipboardReceived { peer, text } => Some((*peer, text.clone())),
        _ => None,
    })
    .await;
    assert_eq!((from, text.as_str()), (id, "héllo"));
    // Not empty, and not without a session.
    assert_eq!(http(&addr, "POST", &clip_path, b"").await.0, 400);
    let stranger = format!("{path}/clip?s={}", "1".repeat(64));
    assert_eq!(http(&addr, "POST", &stranger, b"x").await.0, 404);

    // "Always accept" doesn't stick for a tab: it's temporary.
    let ask_path = format!("{path}/ask?s={session}&count=1&size=1&name=x.txt");
    for round in 0..2 {
        let (ask_addr, ask) = (addr.clone(), ask_path.clone());
        let asking = tokio::spawn(async move { http(&ask_addr, "POST", &ask, b"").await });
        let question = wait_for(&mut events, |e| match e {
            NodeEvent::OfferReceived { id, .. } => Some(*id),
            _ => None,
        })
        .await;
        node.answer_offer(question, true, round == 0).unwrap();
        assert!(String::from_utf8_lossy(&asking.await.unwrap().1).contains("accepted"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tab_keeps_its_place_when_it_reconnects() {
    let (node, _events, _dir) = start().await;
    let url = node.start_web_share().await.unwrap();
    let (addr, path) = split(&url);

    let mut tab = Events::open(&addr, &path, None).await;
    let session = field(&tab.next("hello").await, "session");
    drop(tab);

    // Back within the grace period with its session (a phone unlocked):
    // the same device, not a second one.
    let mut tab = Events::open(&addr, &path, Some(&session)).await;
    assert_eq!(field(&tab.next("hello").await, "session"), session);
    let browsers = node
        .peers()
        .into_iter()
        .filter(|p| p.platform == "browser")
        .count();
    assert_eq!(browsers, 1);

    // An unknown session is just a new tab.
    let mut other = Events::open(&addr, &path, Some(&"2".repeat(64))).await;
    let hello = other.next("hello").await;
    assert_ne!(field(&hello, "session"), session);
    assert_eq!(field(&hello, "name"), "iPhone 2");
}

/// The next transfer to fail: its ID and error.
async fn failed(events: &mut UnboundedReceiver<NodeEvent>) -> (u64, String) {
    wait_for(events, |e| match e {
        NodeEvent::TransferFailed { id, error } => Some((*id, error.clone())),
        NodeEvent::TransferDone { .. } => panic!("shouldn't finish"),
        _ => None,
    })
    .await
}

/// Every way a send to or from a tab can be cancelled, from either end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sends_can_be_cancelled_from_either_end() {
    let (node, mut events, dir) = start().await;
    let url = node.start_web_share().await.unwrap();
    let (addr, path) = split(&url);
    let mut tab = Events::open(&addr, &path, None).await;
    let session = field(&tab.next("hello").await, "session");
    let id: Fingerprint = session.parse().unwrap();
    let small = dir.path().join("small.txt");
    std::fs::write(&small, b"hello").unwrap();
    let big = dir.path().join("big.bin");
    std::fs::File::create(&big)
        .unwrap()
        .set_len(512 << 20)
        .unwrap();

    // Cancelled here while the page is still deciding: the offer goes.
    let ids = node.send_files(id, vec![small.clone()]);
    tab.next("offer").await;
    assert!(node.cancel(ids[0]));
    tab.next("withdrawn").await;
    assert_eq!(failed(&mut events).await, (ids[0], "cancelled".into()));

    // The page cancels what it accepted, before downloading it.
    let ids = node.send_files(id, vec![small.clone(), big.clone()]);
    let batch = field(&tab.next("offer").await, "batch");
    let answer = format!("{path}/answer?s={session}&batch={batch}&accept=1");
    assert_eq!(http(&addr, "POST", &answer, b"").await.0, 200);
    tab.next("download").await;
    let cancel_down = format!("{path}/cancel?s={session}&what=down");
    assert_eq!(http(&addr, "POST", &cancel_down, b"").await.0, 200);
    assert!(tab.next("cancelled").await.contains("\"by\":\"you\""));
    for _ in 0..2 {
        let (gone, error) = failed(&mut events).await;
        assert!(ids.contains(&gone));
        assert_eq!(error, "they cancelled");
    }

    // Cancelled here mid-download: the stream stops.
    let ids = node.send_files(id, vec![big.clone()]);
    let batch = field(&tab.next("offer").await, "batch");
    let answer = format!("{path}/answer?s={session}&batch={batch}&accept=1");
    assert_eq!(http(&addr, "POST", &answer, b"").await.0, 200);
    tab.next("download").await;
    let mut download = TcpStream::connect(&addr).await.unwrap();
    let head = format!(
        "GET {path}/f/{}?s={session} HTTP/1.1\r\nHost: {addr}\r\n\r\n",
        ids[0]
    );
    download.write_all(head.as_bytes()).await.unwrap();
    let mut some = vec![0u8; 1 << 20];
    download.read_exact(&mut some).await.unwrap();
    assert!(node.cancel(ids[0]));
    assert!(tab.next("cancelled").await.contains("\"by\":\"them\""));
    assert_eq!(failed(&mut events).await, (ids[0], "cancelled".into()));
    let mut rest = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(5), download.read_to_end(&mut rest)).await;
    assert!(read.is_ok(), "download still open");
    assert!((rest.len() as u64) < 512 << 20);

    // The page cancels its question before it's answered.
    let ask = format!("{path}/ask?s={session}&count=1&size=5&name=x.txt");
    let (ask_addr, ask_path) = (addr.clone(), ask.clone());
    let asking = tokio::spawn(async move { http(&ask_addr, "POST", &ask_path, b"").await });
    let question = wait_for(&mut events, |e| match e {
        NodeEvent::OfferReceived { id, .. } => Some(*id),
        _ => None,
    })
    .await;
    let cancel_up = format!("{path}/cancel?s={session}&what=up");
    assert_eq!(http(&addr, "POST", &cancel_up, b"").await.0, 200);
    wait_for(&mut events, |e| match e {
        NodeEvent::OfferClosed { id } if *id == question => Some(()),
        _ => None,
    })
    .await;
    assert!(String::from_utf8_lossy(&asking.await.unwrap().1).contains("cancelled"));

    // Uploads, cancelled from either end.
    for here in [false, true] {
        let (ask_addr, ask_path) = (addr.clone(), ask.clone());
        let asking = tokio::spawn(async move { http(&ask_addr, "POST", &ask_path, b"").await });
        let question = wait_for(&mut events, |e| match e {
            NodeEvent::OfferReceived { id, .. } => Some(*id),
            _ => None,
        })
        .await;
        node.answer_offer(question, true, false).unwrap();
        asking.await.unwrap();
        let mut up = TcpStream::connect(&addr).await.unwrap();
        let size = 512u64 << 20;
        let head = format!(
            "POST {path}/u?s={session}&name=up.bin HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {size}\r\n\r\n"
        );
        up.write_all(head.as_bytes()).await.unwrap();
        up.write_all(&vec![1u8; 1 << 20]).await.unwrap();
        let started = wait_for(&mut events, |e| match e {
            NodeEvent::TransferStarted { id, .. } => Some(*id),
            _ => None,
        })
        .await;
        if here {
            assert!(node.cancel(started));
            assert!(tab.next("cancelled").await.contains("\"what\":\"up\""));
            assert_eq!(failed(&mut events).await, (started, "cancelled".into()));
        } else {
            assert_eq!(http(&addr, "POST", &cancel_up, b"").await.0, 200);
            assert_eq!(
                failed(&mut events).await,
                (started, "they cancelled".into())
            );
        }
        // Nothing kept, and no more uploads from that send.
        let downloads = dir.path().join("downloads");
        let kept: Vec<_> = std::fs::read_dir(&downloads)
            .map(|d| d.flatten().map(|e| e.file_name()).collect())
            .unwrap_or_default();
        assert!(kept.is_empty(), "{kept:?}");
        let again = format!("{path}/u?s={session}&name=x.txt");
        assert_eq!(http(&addr, "POST", &again, b"x").await.0, 403);
    }
}

/// The page starts the download in the Accept tap (so the browser counts it
/// as the person's), which can arrive before the answer: it waits for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_download_can_start_before_its_accept_lands() {
    let (node, mut events, dir) = start().await;
    let url = node.start_web_share().await.unwrap();
    let (addr, path) = split(&url);
    let mut tab = Events::open(&addr, &path, None).await;
    let session = field(&tab.next("hello").await, "session");
    let id: Fingerprint = session.parse().unwrap();
    let a = dir.path().join("a.txt");
    std::fs::write(&a, b"early bird").unwrap();

    let ids = node.send_files(id, vec![a]);
    let batch = field(&tab.next("offer").await, "batch");
    let get = format!("{path}/f/{batch}?s={session}");
    let (get_addr, get_path) = (addr.clone(), get.clone());
    let fetching = tokio::spawn(async move { http(&get_addr, "GET", &get_path, b"").await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let answer = format!("{path}/answer?s={session}&batch={batch}&accept=1");
    assert_eq!(http(&addr, "POST", &answer, b"").await.0, 200);
    let (status, body) = fetching.await.unwrap();
    assert_eq!((status, body.as_slice()), (200, &b"early bird"[..]));
    wait_for(&mut events, |e| match e {
        NodeEvent::TransferDone { id, .. } if *id == ids[0] => Some(()),
        _ => None,
    })
    .await;
    // Something never offered isn't waited for.
    let started = std::time::Instant::now();
    assert_eq!(http(&addr, "GET", &format!("{path}/f/12345?s={session}"), b"").await.0, 404);
    assert!(started.elapsed() < Duration::from_secs(2));
}
