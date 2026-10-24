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

    // Send two files: one offer, then one download at a time.
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
    for (expected_id, expected) in ids.iter().zip([&a_data[..], b"second"]) {
        let next = tab.next("download").await;
        assert_eq!(field(&next, "id"), expected_id.to_string());
        let (status, body) = http(
            &addr,
            "GET",
            &format!("{path}/f/{expected_id}?s={session}"),
            b"",
        )
        .await;
        assert_eq!(status, 200);
        assert!(body == expected);
        // The page hears when the whole file has gone.
        assert_eq!(
            field(&tab.next("delivered").await, "id"),
            expected_id.to_string()
        );
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
    let mut rest = Vec::new();
    let closed = tokio::time::timeout(Duration::from_secs(5), tab.tcp.read_to_end(&mut rest)).await;
    assert!(closed.is_ok(), "event stream still open");
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
