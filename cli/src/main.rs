//! `nudge` command-line tool for testing the core between two processes.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use nudge_core::{
    DEFAULT_PORT, Error, Fingerprint, Identity, Tls, TransferStats, TrustedPeers, receive_file,
    send_file,
};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

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
    },
    /// Send a file to a receiver.
    Send {
        /// File to send.
        file: PathBuf,
        /// Receiver address, as host or host:port.
        #[arg(default_value = "127.0.0.1")]
        to: String,
    },
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

    match cli.command {
        Command::Id => show_id(&identity_dir),
        Command::Trust { fingerprint } => trust(&identity_dir, &fingerprint),
        Command::Receive {
            dir,
            port,
            bind,
            once,
        } => {
            let tls = (!cli.no_tls).then(|| load_tls(&identity_dir)).transpose()?;
            receive(tls, dir, bind, port, once).await
        }
        Command::Send { file, to } => {
            let tls = (!cli.no_tls).then(|| load_tls(&identity_dir)).transpose()?;
            send(tls, file, to).await
        }
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
) -> Result<()> {
    anyhow::ensure!(dir.is_dir(), "{} is not a folder", dir.display());
    let listener = TcpListener::bind((bind, port))
        .await
        .with_context(|| format!("couldn't listen on {bind}:{port}"))?;
    println!(
        "Waiting for files on {} (saving to {}){}",
        listener.local_addr()?,
        dir.display(),
        if tls.is_none() { " [UNENCRYPTED]" } else { "" }
    );

    loop {
        let (tcp, addr) = listener.accept().await?;
        tcp.set_nodelay(true)?;
        let result = match &tls {
            Some(tls) => match tls.accept(tcp).await {
                Ok((mut stream, peer)) => receive_one(&mut stream, &dir, Some(peer)).await,
                Err(e) => Err(e),
            },
            None => {
                let mut tcp = tcp;
                receive_one(&mut tcp, &dir, None).await
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
) -> nudge_core::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let file = receive_file(stream, dir, peer).await?;
    let _ = stream.shutdown().await;
    println!("Received {}: {}", file.path.display(), summary(&file.stats));
    Ok(())
}

async fn send(tls: Option<Tls>, file: PathBuf, to: String) -> Result<()> {
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
            Ok((mut stream, _peer)) => send_one(&mut stream, &file).await,
            Err(e) => Err(e),
        },
        None => {
            let mut tcp = tcp;
            send_one(&mut tcp, &file).await
        }
    };
    if let Err(e) = &result {
        hint_trust(e);
    }
    let stats = result.with_context(|| format!("sending {} failed", file.display()))?;
    println!("Sent {}: {}", file.display(), summary(&stats));
    Ok(())
}

async fn send_one<S>(stream: &mut S, file: &Path) -> nudge_core::Result<TransferStats>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let stats = send_file(stream, file).await?;
    let _ = stream.shutdown().await;
    Ok(stats)
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
