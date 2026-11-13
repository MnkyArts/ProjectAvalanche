use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Limited};
use hyper::Request;
use hyper::header::{HOST, USER_AGENT};
use hyper_util::rt::TokioIo;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::config::Config;

// PORT(choice): 2 s to connect (loopback answers at once), 8 s in all (the master answers on loopback in under a
// millisecond; the bound only ends a master that hangs); the original has no master server.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_HEAD: usize = 8192;
const MAX_BODY: usize = 1024;

pub fn targets(config: &Config) -> Vec<SocketAddr> {
    match config.bind {
        IpAddr::V4(v4) if v4.is_unspecified() => vec![(Ipv4Addr::LOCALHOST, config.port).into()],
        IpAddr::V6(v6) if v6.is_unspecified() => {
            vec![(Ipv4Addr::LOCALHOST, config.port).into(), (Ipv6Addr::LOCALHOST, config.port).into()]
        }
        ip => vec![SocketAddr::new(ip, config.port)],
    }
}

#[derive(Debug)]
struct Pinned {
    leaf: CertificateDer<'static>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if end_entity.as_ref() == self.leaf.as_ref() {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

fn connector(config: &Config) -> Result<Option<tokio_rustls::TlsConnector>, String> {
    let Some(tls) = &config.tls else { return Ok(None) };
    let leaf = crate::tls::leaf(tls)?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let algorithms = provider.signature_verification_algorithms;
    let mut client = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned { leaf, algorithms }))
        .with_no_client_auth();
    client.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Some(tokio_rustls::TlsConnector::from(Arc::new(client))))
}

async fn exchange<I>(io: I, addr: SocketAddr) -> Result<(u16, Vec<u8>), String>
where
    I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let mut b = hyper::client::conn::http1::Builder::new();
    b.max_buf_size(MAX_HEAD).max_headers(32);
    let (mut sender, conn) = b.handshake::<_, Empty<Bytes>>(io).await.map_err(|e| format!("HTTP: {e}"))?;
    let driver = tokio::spawn(conn);
    let result = async {
        let request = Request::get("/v1/health")
            .header(HOST, addr.to_string())
            .header(USER_AGENT, concat!("avalanche-master-health/", env!("CARGO_PKG_VERSION")))
            .body(Empty::<Bytes>::new())
            .map_err(|e| e.to_string())?;
        let response = sender.send_request(request).await.map_err(|e| format!("HTTP: {e}"))?;
        let status = response.status().as_u16();
        let body = Limited::new(response.into_body(), MAX_BODY)
            .collect()
            .await
            .map_err(|_| format!("the answer is larger than {MAX_BODY} bytes or ended early"))?;
        Ok((status, body.to_bytes().to_vec()))
    }
    .await;
    driver.abort();
    result
}

async fn ask(addr: SocketAddr, tls: Option<&tokio_rustls::TlsConnector>) -> Result<(), String> {
    let stream = match timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(format!("{addr}: {e}")),
        Err(_) => return Err(format!("{addr}: no connection within {} s", CONNECT_TIMEOUT.as_secs())),
    };
    let (status, body) = match tls {
        Some(connector) => {
            let name = ServerName::IpAddress(addr.ip().into());
            let tls = connector.connect(name, stream).await.map_err(|e| format!("{addr}: TLS: {e}"))?;
            exchange(TokioIo::new(tls), addr).await
        }
        None => exchange(TokioIo::new(stream), addr).await,
    }
    .map_err(|e| format!("{addr}: {e}"))?;
    let said = String::from_utf8_lossy(&body);
    if status == 200 && said.trim_end() == "ok" {
        Ok(())
    } else {
        Err(format!("{addr}: GET /v1/health answered {status}: {}", ssx_core::text::quoted_max(said.trim_end(), 200)))
    }
}

pub fn check(config: &Config) -> Result<SocketAddr, String> {
    let tls = connector(config)?;
    let runtime =
        tokio::runtime::Builder::new_current_thread().enable_io().enable_time().build().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let all = async {
            let mut why = Vec::new();
            for addr in targets(config) {
                match ask(addr, tls.as_ref()).await {
                    Ok(()) => return Ok(addr),
                    Err(e) => why.push(e),
                }
            }
            Err(why.join("; "))
        };
        match timeout(CHECK_TIMEOUT, all).await {
            Ok(r) => r,
            Err(_) => Err(format!("no answer within {} s", CHECK_TIMEOUT.as_secs())),
        }
    })
}
