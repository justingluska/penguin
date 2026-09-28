//! TCP + TLS for IMAP and SMTP.
//!
//! Certificates are checked by the operating system's verifier (macOS trust
//! settings, so a corporate root the user trusts works). The one exception
//! is a server on this Mac itself (`localhost`, `127.0.0.1`, `::1`): Proton
//! Mail Bridge presents a self-signed certificate there, so for loopback
//! hosts any certificate is accepted (the traffic never leaves the machine).
//! Unencrypted connections are refused for every other host before we get
//! here (`providers/connect.rs` validation, and [`crate::config`]).

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

use crate::{Error, Result};

/// A byte stream: plain TCP, or TLS over it.
pub trait Io: AsyncRead + AsyncWrite + Unpin + Send + Sync {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + Sync> Io for T {}
pub type BoxIo = Box<dyn Io>;

/// Opening a connection (DNS + TCP) gives up after this.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// An address on this machine or the local network.
pub fn is_local_network(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        std::net::IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_local_network(std::net::IpAddr::V4(v4)),
            None => v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local(),
        },
    }
}

/// A host on this machine (Proton Mail Bridge and friends).
pub fn is_loopback(host: &str) -> bool {
    matches!(
        host.trim()
            .trim_end_matches('.')
            .to_ascii_lowercase()
            .as_str(),
        "localhost" | "127.0.0.1" | "::1" | "[::1]"
    )
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn platform_config() -> Result<Arc<ClientConfig>> {
    static CONFIG: OnceLock<std::result::Result<Arc<ClientConfig>, String>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            use rustls_platform_verifier::BuilderVerifierExt;
            ClientConfig::builder_with_provider(provider())
                .with_safe_default_protocol_versions()
                .map_err(|e| e.to_string())?
                .with_platform_verifier()
                .map_err(|e| e.to_string())
                .map(|b| Arc::new(b.with_no_client_auth()))
        })
        .clone()
        .map_err(|e| Error::Network(format!("TLS setup failed: {e}")))
}

/// Accepts any certificate (signatures still checked): loopback only.
#[derive(Debug)]
struct LoopbackVerifier(Arc<CryptoProvider>);

impl ServerCertVerifier for LoopbackVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn loopback_config() -> Result<Arc<ClientConfig>> {
    static CONFIG: OnceLock<std::result::Result<Arc<ClientConfig>, String>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let p = provider();
            ClientConfig::builder_with_provider(p.clone())
                .with_safe_default_protocol_versions()
                .map_err(|e| e.to_string())
                .map(|b| {
                    Arc::new(
                        b.dangerous()
                            .with_custom_certificate_verifier(Arc::new(LoopbackVerifier(p)))
                            .with_no_client_auth(),
                    )
                })
        })
        .clone()
        .map_err(|e| Error::Network(format!("TLS setup failed: {e}")))
}

/// Open a TCP connection with a timeout.
pub async fn tcp(host: &str, port: u16) -> Result<BoxIo> {
    Ok(tcp_to(host, port).await?.0)
}

/// [`tcp`], plus whether the server turned out to be on this machine or
/// the local network (loopback, private, link-local or unique-local
/// addresses): no internet link between us.
pub async fn tcp_to(host: &str, port: u16) -> Result<(BoxIo, bool)> {
    let target = (host.trim_start_matches('[').trim_end_matches(']'), port);
    match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(target)).await {
        Ok(Ok(s)) => {
            let _ = s.set_nodelay(true);
            let local = s.peer_addr().is_ok_and(|a| is_local_network(a.ip()));
            Ok((Box::new(s), local))
        }
        Ok(Err(e)) => Err(Error::Network(format!("couldn't reach {host}:{port}: {e}"))),
        Err(_) => Err(Error::Network(format!(
            "couldn't reach {host}:{port}: timed out"
        ))),
    }
}

/// Start TLS on `stream` for `host`.
pub async fn tls(stream: BoxIo, host: &str) -> Result<BoxIo> {
    let config = if is_loopback(host) {
        loopback_config()?
    } else {
        platform_config()?
    };
    let bare = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let name = ServerName::try_from(bare)
        .map_err(|_| Error::InvalidInput(format!("{host} is not a valid server name")))?;
    let connector = tokio_rustls::TlsConnector::from(config);
    match tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(name, stream)).await {
        Ok(Ok(s)) => Ok(Box::new(s)),
        Ok(Err(e)) => Err(tls_error(host, e)),
        Err(_) => Err(Error::Network(format!(
            "secure connection to {host} timed out"
        ))),
    }
}

fn tls_error(host: &str, e: std::io::Error) -> Error {
    let text = e.to_string();
    if text.contains("certificate")
        || text.contains("Certificate")
        || text.contains("UnknownIssuer")
    {
        Error::Network(format!(
            "{host}'s security certificate isn't trusted by this Mac ({text})"
        ))
    } else {
        Error::Network(format!("secure connection to {host} failed: {text}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_network_addresses() {
        for a in [
            "127.0.0.1",
            "10.0.0.5",
            "172.16.4.1",
            "192.168.1.20",
            "169.254.3.3",
            "::1",
            "fd12:3456::1",
            "fe80::1",
            "::ffff:192.168.1.20",
        ] {
            assert!(is_local_network(a.parse().unwrap()), "{a}");
        }
        for a in [
            "142.250.1.108",
            "2607:f8b0::6d",
            "::ffff:142.250.1.108",
            "100.64.0.1",
        ] {
            assert!(!is_local_network(a.parse().unwrap()), "{a}");
        }
    }

    #[test]
    fn loopback_hosts() {
        for h in ["localhost", "127.0.0.1", "::1", "[::1]", "LOCALHOST."] {
            assert!(is_loopback(h), "{h}");
        }
        for h in ["imap.mail.me.com", "127.0.0.2.example", "localhost.example"] {
            assert!(!is_loopback(h), "{h}");
        }
    }

    #[test]
    fn tls_configs_build() {
        assert!(loopback_config().is_ok());
        // The platform verifier may need OS support; building must not panic.
        let _ = platform_config();
    }
}
