//! QR-code pairing.
//!
//! One device shows a code holding its fingerprint, addresses and a one-time
//! secret. The other scans it, connects over TLS (trusting exactly the
//! fingerprint from the code), and sends a `PairRequest` whose proof is a
//! keyed hash of both fingerprints under the secret. The code's owner checks
//! the proof and, if it matches, trusts the scanner. Each side then stores
//! the other in its trusted list, and later connections need no prompt.
//!
//! A code works once and expires after `CODE_LIFETIME`.

use std::fmt::Write as _;
use std::net::IpAddr;
use std::time::Duration;

use crate::identity::{Fingerprint, clean_name};
use crate::{Error, Result};

/// How long a pairing code stays valid.
pub const CODE_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// Length of the one-time secret in a pairing code.
pub const SECRET_LEN: usize = 16;

const SCHEME: &str = "nudge:pair?";
const CODE_VERSION: &str = "1";
const PROOF_CONTEXT: &str = "nudge pairing proof v1";

/// What a pairing QR code carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingCode {
    pub fingerprint: Fingerprint,
    pub secret: [u8; SECRET_LEN],
    pub device_name: String,
    /// Addresses the code's owner can be reached at.
    pub addrs: Vec<IpAddr>,
    pub port: u16,
}

impl PairingCode {
    /// A fresh code with a random secret.
    pub fn new(fingerprint: Fingerprint, device_name: &str, addrs: Vec<IpAddr>, port: u16) -> Self {
        Self {
            fingerprint,
            secret: random_bytes(),
            device_name: clean_name(device_name),
            addrs,
            port,
        }
    }

    /// Text for the QR code, e.g.
    /// `nudge:pair?v=1&fp=<64 hex>&s=<32 hex>&n=Work%20laptop&a=192.168.1.20&p=47800`.
    pub fn to_uri(&self) -> String {
        let addrs: Vec<String> = self.addrs.iter().map(ToString::to_string).collect();
        format!(
            "{SCHEME}v={CODE_VERSION}&fp={}&s={}&n={}&a={}&p={}",
            self.fingerprint,
            hex(&self.secret),
            percent_encode(&self.device_name),
            percent_encode(&addrs.join(",")),
            self.port
        )
    }

    /// Read a scanned code. Unknown fields are ignored so later versions can
    /// add some.
    pub fn parse(uri: &str) -> Result<Self> {
        let bad = |why: &str| Error::BadPairingCode(why.to_owned());
        let query = uri
            .trim()
            .strip_prefix(SCHEME)
            .ok_or_else(|| bad("not a Nudge pairing code"))?;

        let (mut version, mut fp, mut secret, mut name, mut addrs, mut port) =
            (None, None, None, String::new(), Vec::new(), None);
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let value = percent_decode(value).ok_or_else(|| bad("bad escape"))?;
            match key {
                "v" => version = Some(value),
                "fp" => fp = Some(value.parse::<Fingerprint>()?),
                "s" => secret = Some(unhex::<SECRET_LEN>(&value).ok_or_else(|| bad("bad secret"))?),
                "n" => name = clean_name(&value),
                "a" => {
                    addrs = value
                        .split(',')
                        .filter(|a| !a.is_empty())
                        .map(|a| a.parse().map_err(|_| bad("bad address")))
                        .collect::<Result<_>>()?;
                }
                "p" => port = Some(value.parse().map_err(|_| bad("bad port"))?),
                _ => {}
            }
        }
        if version.as_deref() != Some(CODE_VERSION) {
            return Err(bad("made by a different version of Nudge"));
        }
        Ok(Self {
            fingerprint: fp.ok_or_else(|| bad("missing fingerprint"))?,
            secret: secret.ok_or_else(|| bad("missing secret"))?,
            device_name: name,
            addrs,
            port: port.ok_or_else(|| bad("missing port"))?,
        })
    }
}

/// Proof that the scanner (`client`) read the code shown by `server`.
/// Binding both fingerprints means a proof can't be replayed by, or for,
/// another device.
pub fn proof(secret: &[u8; SECRET_LEN], client: &Fingerprint, server: &Fingerprint) -> [u8; 32] {
    let key = blake3::derive_key(PROOF_CONTEXT, secret);
    let mut hasher = blake3::Hasher::new_keyed(&key);
    hasher.update(client.as_bytes());
    hasher.update(server.as_bytes());
    *hasher.finalize().as_bytes()
}

/// Constant-time check of a proof received from `client`.
pub fn verify(
    secret: &[u8; SECRET_LEN],
    client: &Fingerprint,
    server: &Fingerprint,
    received: &[u8; 32],
) -> bool {
    // blake3::Hash compares in constant time.
    blake3::Hash::from_bytes(proof(secret, client, server)) == blake3::Hash::from_bytes(*received)
}

/// Cryptographically secure random bytes from the TLS provider.
pub(crate) fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    rustls::crypto::aws_lc_rs::default_provider()
        .secure_random
        .fill(&mut out)
        .expect("system random source failed");
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Escape everything except unreserved URI characters.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b',' | b':') {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = s.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Identity;

    fn fp() -> Fingerprint {
        Identity::generate().unwrap().fingerprint()
    }

    #[test]
    fn code_round_trips() {
        let code = PairingCode::new(
            fp(),
            "Darren’s PC & more",
            vec!["192.168.1.20".parse().unwrap(), "fe80::1".parse().unwrap()],
            47_800,
        );
        let uri = code.to_uri();
        assert!(uri.starts_with("nudge:pair?v=1&fp="));
        assert!(!uri.contains(' '));
        assert_eq!(PairingCode::parse(&uri).unwrap(), code);
    }

    #[test]
    fn codes_are_unique() {
        let fp = fp();
        let a = PairingCode::new(fp, "x", vec![], 1);
        let b = PairingCode::new(fp, "x", vec![], 1);
        assert_ne!(a.secret, b.secret);
    }

    #[test]
    fn rejects_bad_codes() {
        let good = PairingCode::new(fp(), "pc", vec![], 47_800).to_uri();
        assert!(PairingCode::parse("https://example.com").is_err());
        assert!(PairingCode::parse(&good.replace("v=1", "v=9")).is_err());
        assert!(PairingCode::parse(&good.replace("&p=47800", "")).is_err());
        assert!(PairingCode::parse(&good.replace("s=", "s=zz")).is_err());
        // Unknown fields are fine.
        assert!(PairingCode::parse(&format!("{good}&future=1")).is_ok());
    }

    #[test]
    fn proof_binds_both_devices() {
        let secret = random_bytes();
        let (client, server, other) = (fp(), fp(), fp());
        let p = proof(&secret, &client, &server);
        assert!(verify(&secret, &client, &server, &p));
        assert!(!verify(&secret, &other, &server, &p));
        assert!(!verify(&secret, &client, &other, &p));
        assert!(!verify(&random_bytes(), &client, &server, &p));
    }
}
