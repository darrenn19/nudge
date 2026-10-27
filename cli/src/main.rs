//! `nudge` command-line tool for testing the core between two processes.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use nudge_core::bench::{self, Discard};
use nudge_core::transfer::receive_into;
use nudge_core::{
    DEFAULT_PORT, Direction, Error, Fingerprint, Identity, Node, NodeConfig, NodeEvent, Tls,
    TransferStats, TrustedPeers, receive_file, send_file,
};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::UnboundedReceiver;

#[derive(Parser)]
#[command(name = "nudge", version, about = "Fast, private file transfer")]
struct Cli {
    /// Folder holding this device's identity and trusted devices.
    /// Defaults to the user config folder (e.g. %APPDATA%\Nudge).
    #[arg(long, global = true)]
    identity: Option<PathBuf>,

    /// Skip encryption. Only for measuring TLS overhead.
    #[arg(long, global = true, hide = true)]
    no_tls: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show this device's fingerprint (creates an identity if needed).
    Id,
    /// Trust another device, so it can send to and receive from this one.
    Trust {
        /// The other device's fingerprint, from `nudge id` on that device.
        fingerprint: String,
    },
    /// Pair with another device by QR code.
    ///
    /// Without a code: show this device's code (as a QR in the terminal) and
    /// wait for the other device to scan it. With a code: pair with the
    /// device that is showing it.
    Pair {
        /// Pairing code text (starts with `nudge:pair?`), from the other device.
        code: Option<String>,
    },
    /// List nearby and paired devices.
    Devices {
        /// Seconds to listen for nearby devices.
        #[arg(long, default_value_t = 3)]
        wait: u64,
    },
    /// Run as a full device: be discoverable, accept pairing, and save files
    /// from paired devices without asking. What the apps do, minus the UI.
    Serve {
        /// Folder to save files into.
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,
        /// Also show a pairing code at startup.
        #[arg(long)]
        pair: bool,
    },
    /// Share with browsers (for people without Nudge), driven from the
    /// terminal: for trying the web page on a real phone.
    ///
    /// Prints the link, then reads commands: `send <file>...` (to every open
    /// tab), `stop`, `start`, `quit`. Files the page sends are accepted and
    /// saved in --dir.
    WebShare {
        /// Folder to save files from the page into.
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,
    },
    /// Wait for incoming files and save them.
    Receive {
        /// Folder to save files into.
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,
        /// Port to listen on.
        #[arg(short, long, default_value_t = DEFAULT_PORT)]
        port: u16,
        /// Address to listen on. Use 0.0.0.0 to accept from other devices.
        #[arg(long, default_value = "127.0.0.1")]
        bind: IpAddr,
        /// Exit after receiving one file.
        #[arg(long)]
        once: bool,
        /// Receive and check data but don't save it. Pair with `nudge bench --to`
        /// to measure a network path without either disk.
        #[arg(long)]
        discard: bool,
    },
    /// Send a file to a receiver.
    Send {
        /// File to send.
        file: PathBuf,
        /// Receiver address, as host or host:port.
        #[arg(default_value = "127.0.0.1")]
        to: String,
    },
    /// Measure transfer speed and CPU use.
    ///
    /// Without --to: runs both ends on this machine and reports each layer
    /// (TCP, TLS, hashing, full protocol) separately.
    /// With --to: sends generated data to a `nudge receive --discard` on
    /// another device, measuring the real network path.
    Bench {
        /// Bytes per run, e.g. 500M, 2G.
        #[arg(long, default_value = "2G", value_parser = parse_size)]
        size: u64,
        /// Also time a real file transfer through this machine's temp folder.
        #[arg(long)]
        disk: bool,
        /// Receiver address (host or host:port) for a remote run.
        #[arg(long)]
        to: Option<String>,
    },
}

/// What a `send` puts on the wire.
enum Payload {
    File(PathBuf),
    Generated(u64),
}

impl Payload {
    fn describe(&self) -> String {
        match self {
            Self::File(path) => path.display().to_string(),
            Self::Generated(size) => format!("{} of generated data", human_size(*size)),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let identity_dir = match cli.identity {
        Some(dir) => dir,
        None => dirs::config_dir()
            .context("couldn't find a config folder; pass --identity")?
            .join("Nudge"),
    };
    let tls = || (!cli.no_tls).then(|| load_tls(&identity_dir)).transpose();

    match cli.command {
        Command::Id => show_id(&identity_dir),
        Command::Trust { fingerprint } => trust(&identity_dir, &fingerprint),
        Command::Pair { code } => pair(identity_dir, code).await,
        Command::Devices { wait } => devices(identity_dir, wait).await,
        Command::Serve { dir, pair } => serve(identity_dir, dir, pair).await,
        Command::WebShare { dir } => web_share(identity_dir, dir).await,
        Command::Receive {
            dir,
            port,
            bind,
            once,
            discard,
        } => receive(tls()?, dir, bind, port, once, discard).await,
        Command::Send { file, to } => send(tls()?, Payload::File(file), to).await,
        Command::Bench { size, disk, to } => match to {
            Some(to) => send(tls()?, Payload::Generated(size), to).await,
            None => bench_local(size, disk).await,
        },
    }
}

fn show_id(identity_dir: &Path) -> Result<()> {
    let identity = Identity::load_or_create(identity_dir)?;
    println!("{}", identity.fingerprint());
    eprintln!("(identity stored in {})", identity_dir.display());
    Ok(())
}

fn trust(identity_dir: &Path, fingerprint: &str) -> Result<()> {
    let fp: Fingerprint = fingerprint.parse()?;
    let peers = TrustedPeers::load(identity_dir)?;
    if peers.add(fp) {
        peers.save(identity_dir)?;
        println!("Now trusting {}", fp.short());
    } else {
        println!("{} was already trusted", fp.short());
    }
    Ok(())
}

/// This computer's name, as other devices will see it.
fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "Nudge CLI".to_owned())
}

async fn start_node(
    identity_dir: PathBuf,
    download_dir: PathBuf,
) -> Result<(Node, UnboundedReceiver<NodeEvent>)> {
    let mut config = NodeConfig::new(
        identity_dir,
        download_dir,
        &device_name(),
        std::env::consts::OS,
    );
    if std::env::var_os("NUDGE_LOG").is_some() {
        nudge_core::logging::to_stderr();
    }
    // No one to ask at a terminal: take whatever paired devices send.
    config.ask_before_receiving = false;
    let (node, events) = Node::start(config).await?;
    if !node.discovery_running() {
        eprintln!("Warning: device discovery couldn't start on this network.");
    }
    Ok((node, events))
}

fn print_qr(text: &str) {
    match qrcode::QrCode::new(text) {
        Ok(qr) => {
            // Inverted, so it scans on the usual dark terminal background.
            let image = qr
                .render::<qrcode::render::unicode::Dense1x2>()
                .dark_color(qrcode::render::unicode::Dense1x2::Light)
                .light_color(qrcode::render::unicode::Dense1x2::Dark)
                .quiet_zone(true)
                .build();
            println!("{image}");
        }
        Err(e) => eprintln!("(couldn't draw a QR code: {e})"),
    }
}

/// Show a pairing code and wait until someone uses it (or it expires).
async fn show_pairing_code(node: &Node, events: &mut UnboundedReceiver<NodeEvent>) -> Result<()> {
    let code = node.start_pairing();
    print_qr(&code);
    println!("Scan this with Nudge on your phone, or on another computer run:");
    println!("  nudge pair \"{code}\"");
    println!("The code works once and expires in 5 minutes.\n");
    while let Some(event) = events.recv().await {
        match event {
            NodeEvent::Paired { peer } => {
                println!(
                    "Paired with {} ({})",
                    peer.device_name,
                    peer.fingerprint.short()
                );
                return Ok(());
            }
            NodeEvent::PairingClosed => anyhow::bail!("the pairing code expired"),
            _ => {}
        }
    }
    Ok(())
}

async fn pair(identity_dir: PathBuf, code: Option<String>) -> Result<()> {
    let (node, mut events) = start_node(identity_dir, std::env::temp_dir()).await?;
    match code {
        Some(code) => {
            // Give discovery a moment, in case the code's addresses don't work.
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let peer = node.pair(&code).await?;
            println!(
                "Paired with {} ({})",
                peer.device_name,
                peer.fingerprint.short()
            );
            Ok(())
        }
        None => show_pairing_code(&node, &mut events).await,
    }
}

async fn devices(identity_dir: PathBuf, wait: u64) -> Result<()> {
    let (node, _events) = start_node(identity_dir, std::env::temp_dir()).await?;
    node.refresh();
    tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
    let peers = node.peers();
    if peers.is_empty() {
        println!("No devices found. Pair one with `nudge pair`.");
    }
    for p in peers {
        let status = match (p.paired, p.online) {
            (true, true) => "paired, nearby",
            (true, false) => "paired, not seen",
            (false, _) => "nearby, not paired",
        };
        let platform = if p.platform.is_empty() {
            String::new()
        } else {
            format!(" [{}]", p.platform)
        };
        println!(
            "{}  {}{platform}  ({status})",
            p.fingerprint.short(),
            p.device_name
        );
    }
    Ok(())
}

async fn serve(identity_dir: PathBuf, dir: PathBuf, pair: bool) -> Result<()> {
    let (node, mut events) = start_node(identity_dir, dir.clone()).await?;
    println!(
        "{} is ready on port {} (fingerprint {}), saving to {}",
        node.device_name(),
        node.port(),
        node.fingerprint().short(),
        dir.display()
    );
    if pair {
        show_pairing_code(&node, &mut events).await?;
    }
    let mut names = std::collections::HashMap::new();
    while let Some(event) = events.recv().await {
        match event {
            NodeEvent::TransferStarted {
                id,
                direction: Direction::Incoming,
                file_name,
                size,
                ..
            } => {
                println!("Receiving {file_name} ({})...", human_size(size));
                names.insert(id, file_name);
            }
            NodeEvent::TransferDone { id, path, stats } if names.remove(&id).is_some() => {
                println!("Received {}: {}", path.display(), summary(&stats));
            }
            NodeEvent::TransferFailed { id, error } => {
                let name = names.remove(&id).unwrap_or_default();
                eprintln!("Transfer {name} failed: {error}");
            }
            NodeEvent::Paired { peer } => println!("Paired with {}", peer.device_name),
            _ => {}
        }
    }
    Ok(())
}

async fn web_share(identity_dir: PathBuf, dir: PathBuf) -> Result<()> {
    use tokio::io::AsyncBufReadExt;

    let (node, mut events) = start_node(identity_dir, dir).await?;
    println!("Sharing at {}", node.start_web_share().await?);
    println!("Commands: send <file>..., stop, start, quit");
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else { return Ok(()) };
                let mut words = line.split_whitespace();
                match words.next() {
                    Some("send") => {
                        let files: Vec<PathBuf> = words.map(PathBuf::from).collect();
                        let tabs: Vec<_> = node
                            .peers()
                            .into_iter()
                            .filter(|p| p.platform == "browser" && p.online)
                            .collect();
                        if tabs.is_empty() {
                            println!("No page open");
                        }
                        for tab in tabs {
                            let ids = node.send_files(tab.fingerprint, files.clone());
                            println!("Offered {ids:?} to {}", tab.device_name);
                        }
                    }
                    Some("stop") => node.stop_web_share(),
                    Some("start") => println!("Sharing at {}", node.start_web_share().await?),
                    Some("quit") => return Ok(()),
                    Some(other) => println!("Unknown command {other}"),
                    None => {}
                }
            }
            event = events.recv() => {
                let Some(event) = event else { return Ok(()) };
                if let NodeEvent::OfferReceived { id, .. } = &event {
                    node.answer_offer(*id, true, false)?;
                }
                if !matches!(event, NodeEvent::TransferProgress { .. }) {
                    println!("{event:?}");
                }
            }
        }
    }
}

fn load_tls(identity_dir: &Path) -> Result<Tls> {
    let identity = Identity::load_or_create(identity_dir)
        .with_context(|| format!("couldn't load identity from {}", identity_dir.display()))?;
    let trusted = TrustedPeers::load(identity_dir)?;
    Ok(Tls::new(&identity, trusted)?)
}

async fn receive(
    tls: Option<Tls>,
    dir: PathBuf,
    bind: IpAddr,
    port: u16,
    once: bool,
    discard: bool,
) -> Result<()> {
    anyhow::ensure!(discard || dir.is_dir(), "{} is not a folder", dir.display());
    let listener = TcpListener::bind((bind, port))
        .await
        .with_context(|| format!("couldn't listen on {bind}:{port}"))?;
    let destination = if discard {
        "discarding data, benchmark mode".to_owned()
    } else {
        format!("saving to {}", dir.display())
    };
    println!(
        "Waiting for files on {} ({destination}){}",
        listener.local_addr()?,
        if tls.is_none() { " [UNENCRYPTED]" } else { "" }
    );

    loop {
        let (tcp, addr) = listener.accept().await?;
        tcp.set_nodelay(true)?;
        let result = match &tls {
            Some(tls) => match tls.accept(tcp).await {
                Ok((mut stream, peer)) => receive_one(&mut stream, &dir, Some(peer), discard).await,
                Err(e) => Err(e),
            },
            None => {
                let mut tcp = tcp;
                receive_one(&mut tcp, &dir, None, discard).await
            }
        };
        if let Err(e) = result {
            eprintln!("Transfer from {addr} failed: {e}");
            hint_trust(&e);
        }
        if once {
            return Ok(());
        }
    }
}

async fn receive_one<S>(
    stream: &mut S,
    dir: &Path,
    peer: Option<Fingerprint>,
    discard: bool,
) -> nudge_core::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (what, stats) = if discard {
        let (offer, stats) = receive_into(stream, Discard).await?;
        (format!("{} (discarded)", offer.name), stats)
    } else {
        let file = receive_file(stream, dir, peer).await?;
        (file.path.display().to_string(), file.stats)
    };
    let _ = stream.shutdown().await;
    println!("Received {what}: {}", summary(&stats));
    Ok(())
}

async fn send(tls: Option<Tls>, payload: Payload, to: String) -> Result<()> {
    let addr = if to.contains(':') {
        to
    } else {
        format!("{to}:{DEFAULT_PORT}")
    };
    let tcp = TcpStream::connect(&addr)
        .await
        .with_context(|| format!("couldn't connect to {addr}"))?;
    tcp.set_nodelay(true)?;

    let result = match tls {
        Some(tls) => match tls.connect(tcp).await {
            Ok((mut stream, _peer)) => send_one(&mut stream, &payload).await,
            Err(e) => Err(e),
        },
        None => {
            let mut tcp = tcp;
            send_one(&mut tcp, &payload).await
        }
    };
    if let Err(e) = &result {
        hint_trust(e);
    }
    let stats = result.with_context(|| format!("sending {} failed", payload.describe()))?;
    println!("Sent {}: {}", payload.describe(), summary(&stats));
    Ok(())
}

async fn send_one<S>(stream: &mut S, payload: &Payload) -> nudge_core::Result<TransferStats>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let stats = match payload {
        Payload::File(path) => send_file(stream, path).await?,
        Payload::Generated(size) => bench::send_generated(stream, *size).await?,
    };
    let _ = stream.shutdown().await;
    Ok(stats)
}

async fn bench_local(size: u64, disk: bool) -> Result<()> {
    println!(
        "Nudge local benchmark: {} per run, both ends on this machine (loopback).",
        human_size(size)
    );
    println!("CPU figures cover sender and receiver together.\n");

    let temp = std::env::temp_dir();
    let results = bench::run_local(size, disk.then_some(temp.as_path())).await?;
    let latency = bench::setup_latency(10).await?;

    println!(
        "{:<44} {:>9} {:>11} {:>11}",
        "Layer", "MB/s", "CPU cores", "CPU s/GB"
    );
    for r in &results {
        println!(
            "{:<44} {:>9.0} {:>11.2} {:>11.2}",
            r.label,
            r.mb_per_sec(),
            r.cores(),
            r.cpu_sec_per_gb()
        );
    }
    println!(
        "\nSetup cost (1-byte file over a new TLS connection, median of 10): {:.1} ms",
        latency.as_secs_f64() * 1000.0
    );
    if !disk {
        println!("Add --disk to include a real file-to-file transfer.");
    }
    Ok(())
}

/// Parse sizes like `2G`, `500M`, `64K` or plain bytes (powers of 1000).
fn parse_size(s: &str) -> std::result::Result<u64, String> {
    let s = s.trim();
    let (number, multiplier) = match s.char_indices().last() {
        Some((i, c)) if c.is_ascii_alphabetic() => {
            let m: u64 = match c.to_ascii_uppercase() {
                'K' => 1_000,
                'M' => 1_000_000,
                'G' => 1_000_000_000,
                'T' => 1_000_000_000_000,
                _ => return Err(format!("unknown size unit in {s:?}; use K, M, G or T")),
            };
            (&s[..i], m)
        }
        _ => (s, 1),
    };
    let value: f64 = number
        .trim()
        .parse()
        .map_err(|_| format!("not a size: {s:?}"))?;
    if value.is_nan() || value <= 0.0 {
        return Err("size must be positive".into());
    }
    Ok((value * multiplier as f64) as u64)
}

fn hint_trust(e: &Error) {
    match e {
        Error::UntrustedPeer(fp) => {
            eprintln!("If you recognise that device, run:\n  nudge trust {fp}");
        }
        Error::NotTrustedByPeer => {
            eprintln!("On the other device, run `nudge trust` with this device's `nudge id`.");
        }
        _ => {}
    }
}

fn summary(stats: &TransferStats) -> String {
    let resumed = if stats.resumed_from > 0 {
        format!(", resumed from {}", human_size(stats.resumed_from))
    } else {
        String::new()
    };
    format!(
        "{} in {:.2} s ({:.1} MB/s{resumed})",
        human_size(stats.bytes),
        stats.elapsed.as_secs_f64(),
        stats.mb_per_sec()
    )
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::parse_size;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("2G"), Ok(2_000_000_000));
        assert_eq!(parse_size("500m"), Ok(500_000_000));
        assert_eq!(parse_size("1.5G"), Ok(1_500_000_000));
        assert_eq!(parse_size("4096"), Ok(4096));
        assert!(parse_size("0").is_err());
        assert!(parse_size("3X").is_err());
        assert!(parse_size("lots").is_err());
    }
}
