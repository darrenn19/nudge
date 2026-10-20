//! TLS 1.3 with mutual authentication and certificate pinning.
//!
//! Both sides present their self-signed certificate. A connection succeeds
//! only if each side's fingerprint is in the other's trusted list. There is
//! no certificate authority and the server name is not checked.

use std::fmt;
use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    AlertDescription, CertificateError, ClientConfig, DigitallySignedStruct, DistinguishedName,
    OtherError, ServerConfig, SignatureScheme,
};
use tokio::net::TcpStream;
use tokio_rustls::{TlsAcceptor, TlsConnector, client, server};

use crate::identity::{CERT_NAME, Fingerprint, Identity, TrustedPeers};
use crate::{Error, Result};

/// Client and server TLS settings for this device.
#[derive(Clone)]
pub struct Tls {
    connector: TlsConnector,
    acceptor: TlsAcceptor,
}

impl Tls {
    /// `trusted` is shared: adding a peer to it later takes effect immediately.
    /// This device's own fingerprint is always trusted, which lets two
    /// processes on the same machine talk to each other.
    pub fn new(identity: &Identity, trusted: TrustedPeers) -> Result<Self> {
        // aws-lc-rs: AES-GCM runs on the CPU's hardware AES instructions.
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let verifier = Arc::new(PinnedPeers {
            trusted,
            own: identity.fingerprint(),
            algorithms: provider.signature_verification_algorithms,
        });

        let client = ClientConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .dangerous()
            .with_custom_certificate_verifier(verifier.clone())
            .with_client_auth_cert(identity.cert_chain(), identity.private_key())?;

        let server = ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .with_client_cert_verifier(verifier)
            .with_single_cert(identity.cert_chain(), identity.private_key())?;

        Ok(Self {
            connector: TlsConnector::from(Arc::new(client)),
            acceptor: TlsAcceptor::from(Arc::new(server)),
        })
    }

    /// Open a TLS session as the connecting side.
    /// Returns the stream and the peer's fingerprint.
    pub async fn connect(
        &self,
        tcp: TcpStream,
    ) -> Result<(client::TlsStream<TcpStream>, Fingerprint)> {
        let name = ServerName::try_from(CERT_NAME).expect("valid DNS name");
        let stream = self.connector.connect(name, tcp).await?;
        let peer = peer_fingerprint(stream.get_ref().1.peer_certificates())?;
        Ok((stream, peer))
    }

    /// Open a TLS session as the listening side.
    /// Returns the stream and the peer's fingerprint.
    pub async fn accept(
        &self,
        tcp: TcpStream,
    ) -> Result<(server::TlsStream<TcpStream>, Fingerprint)> {
        let stream = self.acceptor.accept(tcp).await?;
        let peer = peer_fingerprint(stream.get_ref().1.peer_certificates())?;
        Ok((stream, peer))
    }
}

fn peer_fingerprint(certs: Option<&[CertificateDer<'_>]>) -> Result<Fingerprint> {
    certs
        .and_then(|c| c.first())
        .map(Fingerprint::of)
        .ok_or(Error::Protocol("peer sent no certificate"))
}

/// Recognise pairing problems hidden inside I/O errors from a TLS stream:
/// - we rejected the peer → `UntrustedPeer` (with its fingerprint, so the
///   caller can offer to pair)
/// - the peer rejected us → `NotTrustedByPeer`
///
/// The second can surface after the handshake, on the first read, which is
/// why this runs on every I/O error rather than only in `connect`/`accept`.
pub(crate) fn classify_io_error(e: std::io::Error) -> Error {
    match e.get_ref().and_then(|r| r.downcast_ref::<rustls::Error>()) {
        Some(rustls::Error::InvalidCertificate(CertificateError::Other(OtherError(inner)))) => {
            if let Some(Untrusted(fp)) = inner.downcast_ref::<Untrusted>() {
                return Error::UntrustedPeer(*fp);
            }
        }
        Some(rustls::Error::AlertReceived(
            AlertDescription::CertificateUnknown | AlertDescription::BadCertificate,
        )) => return Error::NotTrustedByPeer,
        _ => {}
    }
    Error::Io(e)
}

/// Carried inside rustls' error so `classify_io_error` can recover the fingerprint.
#[derive(Debug)]
struct Untrusted(Fingerprint);

impl fmt::Display for Untrusted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "untrusted device {}", self.0)
    }
}

impl std::error::Error for Untrusted {}

/// Accepts exactly the certificates whose fingerprints are trusted.
struct PinnedPeers {
    trusted: TrustedPeers,
    own: Fingerprint,
    algorithms: WebPkiSupportedAlgorithms,
}

impl fmt::Debug for PinnedPeers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PinnedPeers").finish_non_exhaustive()
    }
}

impl PinnedPeers {
    fn check(&self, cert: &CertificateDer<'_>) -> std::result::Result<(), rustls::Error> {
        let fp = Fingerprint::of(cert);
        if fp == self.own || self.trusted.contains(&fp) {
            Ok(())
        } else {
            Err(rustls::Error::InvalidCertificate(CertificateError::Other(
                OtherError(Arc::new(Untrusted(fp))),
            )))
        }
    }

    fn verify_tls13(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }
}

impl ServerCertVerifier for PinnedPeers {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        self.check(end_entity)
            .map(|()| ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 is not supported".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.verify_tls13(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

impl ClientCertVerifier for PinnedPeers {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> std::result::Result<ClientCertVerified, rustls::Error> {
        self.check(end_entity)
            .map(|()| ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 is not supported".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.verify_tls13(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}
