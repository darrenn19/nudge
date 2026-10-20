//! Device identity and the list of trusted (paired) devices.
//!
//! Each device has one self-signed certificate. Devices recognise each other
//! by the certificate's fingerprint (BLAKE3 of the DER bytes), never by a
//! certificate authority. Pairing = adding a peer's fingerprint to the
//! trusted list.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, RwLock};

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use crate::{Error, Result};

const CERT_FILE: &str = "cert.der";
const KEY_FILE: &str = "key.der";
const TRUSTED_FILE: &str = "trusted_peers.txt";

/// Name baked into every Nudge certificate. Not used for verification.
pub(crate) const CERT_NAME: &str = "nudge.local";

/// Identifies a device: BLAKE3 hash of its certificate.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    pub fn of(cert: &CertificateDer<'_>) -> Self {
        Self(*blake3::hash(cert.as_ref()).as_bytes())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// First 8 hex characters, for showing in a UI next to a device name.
    pub fn short(&self) -> String {
        self.to_string()[..8].to_owned()
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({self})")
    }
}

impl FromStr for Fingerprint {
    type Err = Error;

    /// Accepts 64 hex digits; spaces, `:` and `-` are ignored.
    fn from_str(s: &str) -> Result<Self> {
        let hex: Vec<u8> = s
            .bytes()
            .filter(|b| !matches!(b, b' ' | b':' | b'-'))
            .collect();
        let bad = || Error::BadFingerprint(s.to_owned());
        if hex.len() != 64 {
            return Err(bad());
        }
        let mut out = [0u8; 32];
        for (i, pair) in hex.chunks_exact(2).enumerate() {
            let pair = std::str::from_utf8(pair).map_err(|_| bad())?;
            out[i] = u8::from_str_radix(pair, 16).map_err(|_| bad())?;
        }
        Ok(Self(out))
    }
}

/// This device's certificate and private key.
pub struct Identity {
    cert: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
}

impl Identity {
    /// Make a fresh identity (new key pair and self-signed certificate).
    pub fn generate() -> Result<Self> {
        let generated = rcgen::generate_simple_self_signed(vec![CERT_NAME.to_owned()])
            .map_err(|e| Error::Identity(e.to_string()))?;
        Ok(Self {
            cert: generated.cert.der().clone(),
            key: PrivatePkcs8KeyDer::from(generated.signing_key.serialize_der()),
        })
    }

    /// Load the identity stored in `dir`, creating and saving one if none exists.
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        let (cert_path, key_path) = (dir.join(CERT_FILE), dir.join(KEY_FILE));
        match (fs::read(&cert_path), fs::read(&key_path)) {
            (Ok(cert), Ok(key)) => Ok(Self {
                cert: CertificateDer::from(cert),
                key: PrivatePkcs8KeyDer::from(key),
            }),
            (Err(e), _) | (_, Err(e)) if e.kind() == io::ErrorKind::NotFound => {
                let identity = Self::generate()?;
                fs::create_dir_all(dir)?;
                fs::write(&key_path, identity.key.secret_pkcs8_der())?;
                fs::write(&cert_path, identity.cert.as_ref())?;
                Ok(identity)
            }
            (Err(e), _) | (_, Err(e)) => Err(e.into()),
        }
    }

    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint::of(&self.cert)
    }

    pub(crate) fn cert_chain(&self) -> Vec<CertificateDer<'static>> {
        vec![self.cert.clone()]
    }

    pub(crate) fn private_key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(self.key.clone_key())
    }
}

/// Fingerprints of devices we accept connections from and connect to.
/// Cheap to clone; clones share the same list, so pairing a new device
/// takes effect on live TLS configs immediately.
#[derive(Clone, Default, Debug)]
pub struct TrustedPeers(Arc<RwLock<HashSet<Fingerprint>>>);

impl TrustedPeers {
    pub fn new(peers: impl IntoIterator<Item = Fingerprint>) -> Self {
        Self(Arc::new(RwLock::new(peers.into_iter().collect())))
    }

    /// Load `trusted_peers.txt` from `dir` (one fingerprint per line, `#` comments).
    /// A missing file means no trusted peers yet.
    pub fn load(dir: &Path) -> Result<Self> {
        let text = match fs::read_to_string(dir.join(TRUSTED_FILE)) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.into()),
        };
        let peers = text
            .lines()
            .map(|l| l.split('#').next().unwrap_or("").trim())
            .filter(|l| !l.is_empty())
            .map(Fingerprint::from_str)
            .collect::<Result<Vec<_>>>()?;
        Ok(Self::new(peers))
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        let mut peers: Vec<String> = self.read().iter().map(ToString::to_string).collect();
        peers.sort();
        let mut text = String::from("# Devices this one trusts. One fingerprint per line.\n");
        for p in peers {
            text.push_str(&p);
            text.push('\n');
        }
        fs::create_dir_all(dir)?;
        fs::write(dir.join(TRUSTED_FILE), text)?;
        Ok(())
    }

    /// Returns true if the peer was newly added.
    pub fn add(&self, peer: Fingerprint) -> bool {
        self.0
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(peer)
    }

    pub fn remove(&self, peer: &Fingerprint) -> bool {
        self.0
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(peer)
    }

    pub fn contains(&self, peer: &Fingerprint) -> bool {
        self.read().contains(peer)
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashSet<Fingerprint>> {
        self.0.read().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_text_round_trip() {
        let id = Identity::generate().unwrap();
        let fp = id.fingerprint();
        let text = fp.to_string();
        assert_eq!(text.len(), 64);
        assert_eq!(text.parse::<Fingerprint>().unwrap(), fp);
        // Separators people might paste are ignored.
        let spaced: String = text
            .as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect::<Vec<_>>()
            .join("-");
        assert_eq!(spaced.parse::<Fingerprint>().unwrap(), fp);
        assert!("abc".parse::<Fingerprint>().is_err());
        assert!("zz".repeat(32).parse::<Fingerprint>().is_err());
    }

    #[test]
    fn identity_persists() {
        let dir = tempfile::tempdir().unwrap();
        let first = Identity::load_or_create(dir.path()).unwrap();
        let second = Identity::load_or_create(dir.path()).unwrap();
        assert_eq!(first.fingerprint(), second.fingerprint());
        assert_ne!(
            first.fingerprint(),
            Identity::generate().unwrap().fingerprint()
        );
    }

    #[test]
    fn trusted_peers_persist() {
        let dir = tempfile::tempdir().unwrap();
        let fp = Identity::generate().unwrap().fingerprint();
        let peers = TrustedPeers::load(dir.path()).unwrap();
        assert!(!peers.contains(&fp));
        assert!(peers.add(fp));
        assert!(!peers.add(fp));
        peers.save(dir.path()).unwrap();
        assert!(TrustedPeers::load(dir.path()).unwrap().contains(&fp));
    }
}
