use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{mpsc, Mutex};

/// Configure TCP keepalive on a TcpStream for fast dead-peer detection.
/// idle=10s, interval=5s → detects dead connections within ~20s.
fn configure_tcp_keepalive(stream: &TcpStream) {
    let sock = socket2::SockRef::from(stream);
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(std::time::Duration::from_secs(10))
        .with_interval(std::time::Duration::from_secs(5));
    if let Err(e) = sock.set_tcp_keepalive(&keepalive) {
        log::warn!("Failed to set TCP keepalive: {}", e);
    }
}

use super::diagnostics::DiagnosticSender;

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum TransportType {
    Udp,
    Tcp,
    Tls,
}

impl TransportType {
    #[allow(dead_code)]
    pub fn default_port(&self) -> u16 {
        match self {
            Self::Udp | Self::Tcp => 5060,
            Self::Tls => 5061,
        }
    }

    pub fn param(&self) -> &str {
        match self {
            Self::Udp => "udp",
            Self::Tcp => "tcp",
            Self::Tls => "tls",
        }
    }
}

#[derive(Debug)]
pub struct SipMessage {
    pub data: Vec<u8>,
    pub remote: SocketAddr,
}

// ---------------------------------------------------------------------------
// Unified SipTransport enum
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub enum SipTransport {
    Udp(UdpTransport),
    Tcp(TcpTransport),
    Tls(TlsTransport),
}

impl SipTransport {
    pub async fn send_to(&self, data: &[u8], addr: SocketAddr) -> Result<(), String> {
        // Log outbound SIP message to diagnostics before sending
        if let Some(diag) = self.diagnostic_sender() {
            if let Ok(text) = std::str::from_utf8(data) {
                diag.log_sent(text, addr).await;
            }
        }

        match self {
            Self::Udp(t) => t.send_to(data, addr).await,
            Self::Tcp(t) => t.send_to(data, addr).await,
            Self::Tls(t) => t.send_to(data, addr).await,
        }
    }

    pub fn local_addr(&self) -> SocketAddr {
        match self {
            Self::Udp(t) => t.local_addr(),
            Self::Tcp(t) => t.local_addr(),
            Self::Tls(t) => t.local_addr(),
        }
    }

    /// Attach a diagnostic sender to this transport for automatic send logging.
    pub fn set_diagnostic_sender(&mut self, sender: DiagnosticSender) {
        match self {
            Self::Udp(t) => t.diagnostic = Some(sender),
            Self::Tcp(t) => t.diagnostic = Some(sender),
            Self::Tls(t) => t.diagnostic = Some(sender),
        }
    }

    fn diagnostic_sender(&self) -> Option<&DiagnosticSender> {
        match self {
            Self::Udp(t) => t.diagnostic.as_ref(),
            Self::Tcp(t) => t.diagnostic.as_ref(),
            Self::Tls(t) => t.diagnostic.as_ref(),
        }
    }
}

// ---------------------------------------------------------------------------
// UDP transport
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct UdpTransport {
    socket: Arc<UdpSocket>,
    local_addr: SocketAddr,
    pub diagnostic: Option<DiagnosticSender>,
}

impl UdpTransport {
    pub async fn bind(addr: &str) -> Result<(Self, mpsc::Receiver<SipMessage>), String> {
        let socket = UdpSocket::bind(addr)
            .await
            .map_err(|e| format!("Failed to bind UDP: {}", e))?;
        let local_addr = socket
            .local_addr()
            .map_err(|e| format!("Failed to get local addr: {}", e))?;

        log::info!("SIP UDP transport bound to {}", local_addr);

        let socket = Arc::new(socket);
        let (tx, rx) = mpsc::channel::<SipMessage>(256);

        let recv_socket = socket.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 65535];
            loop {
                match recv_socket.recv_from(&mut buf).await {
                    Ok((len, remote)) => {
                        let msg = SipMessage {
                            data: buf[..len].to_vec(),
                            remote,
                        };
                        if tx.send(msg).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        log::error!("UDP recv error: {}", e);
                    }
                }
            }
        });

        Ok((Self { socket, local_addr, diagnostic: None }, rx))
    }

    pub async fn send_to(&self, data: &[u8], addr: SocketAddr) -> Result<(), String> {
        self.socket
            .send_to(data, addr)
            .await
            .map_err(|e| format!("UDP send error: {}", e))?;
        Ok(())
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

// ---------------------------------------------------------------------------
// SIP message framing helpers for stream transports (TCP / TLS)
// ---------------------------------------------------------------------------

/// Largest SIP message (headers + body) we are willing to assemble.
/// RFC 3261 has no hard limit, but real SIP is a few KB; 128 KiB is generous.
pub(crate) const MAX_SIP_MESSAGE: usize = 128 * 1024;

/// Largest amount of un-framed data we will hold for a stream peer before
/// giving up. Prevents a peer that never sends a complete message (or never
/// sends the double-CRLF at all) from exhausting memory.
pub(crate) const MAX_PENDING: usize = 512 * 1024;

/// Outcome of a framing attempt.
enum FrameResult {
    /// Number of bytes consumed from the front of the buffer.
    Consumed(usize),
    /// The peer sent something we refuse to parse; the connection must be torn
    /// down (continuing would desynchronise the stream).
    Fatal(&'static str),
}

/// Extract complete SIP messages from a byte buffer using Content-Length
/// framing. Returns the number of bytes consumed, or a fatal framing error.
fn extract_sip_messages(buf: &[u8], out: &mut Vec<Vec<u8>>) -> FrameResult {
    let mut consumed = 0;

    loop {
        let remaining = &buf[consumed..];
        if remaining.is_empty() {
            break;
        }

        // Find the header/body separator: \r\n\r\n
        let sep = match find_double_crlf(remaining) {
            Some(pos) => pos,
            None => {
                // No complete header block yet. Refuse to buffer an unbounded
                // header section.
                if remaining.len() > MAX_SIP_MESSAGE {
                    return FrameResult::Fatal("SIP header block exceeds maximum size");
                }
                break; // incomplete headers
            }
        };

        let body_start = sep + 4; // past the double-CRLF

        let headers = match std::str::from_utf8(&remaining[..sep]) {
            Ok(s) => s,
            Err(_) => return FrameResult::Fatal("non-UTF-8 SIP header block"),
        };

        let content_length = match parse_content_length(headers) {
            Ok(n) => n,
            Err(e) => return FrameResult::Fatal(e),
        };

        // Checked arithmetic: a hostile Content-Length must not be able to wrap
        // `total_len` and desynchronise the framer (or panic in debug builds).
        let total_len = match body_start.checked_add(content_length) {
            Some(n) if n <= MAX_SIP_MESSAGE => n,
            Some(_) => return FrameResult::Fatal("SIP message exceeds maximum size"),
            None => return FrameResult::Fatal("Content-Length overflow"),
        };

        if remaining.len() < total_len {
            break; // incomplete body
        }

        out.push(remaining[..total_len].to_vec());
        consumed += total_len;
    }

    FrameResult::Consumed(consumed)
}

fn find_double_crlf(data: &[u8]) -> Option<usize> {
    data.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Parse the Content-Length header.
///
/// Rejects values above `MAX_SIP_MESSAGE` and rejects *conflicting* duplicate
/// headers: letting a peer supply two different lengths and picking one is a
/// request-smuggling primitive when a proxy sits in front of us.
fn parse_content_length(headers: &str) -> Result<usize, &'static str> {
    let mut found: Option<usize> = None;

    for line in headers.lines() {
        let lower = line.to_ascii_lowercase();
        if !(lower.starts_with("content-length:") || lower.starts_with("l:")) {
            continue;
        }
        let value = match line.split_once(':') {
            Some((_, v)) => v.trim(),
            None => continue,
        };
        let n: usize = match value.parse() {
            Ok(n) => n,
            Err(_) => return Err("malformed Content-Length"),
        };
        if n > MAX_SIP_MESSAGE {
            return Err("Content-Length exceeds maximum message size");
        }
        match found {
            Some(prev) if prev != n => return Err("conflicting Content-Length headers"),
            _ => found = Some(n),
        }
    }

    Ok(found.unwrap_or(0))
}

/// Shared receive logic for any `AsyncRead` stream.
async fn stream_receive_loop<R: AsyncReadExt + Unpin>(
    mut reader: R,
    tx: mpsc::Sender<SipMessage>,
    remote: SocketAddr,
    label: &str,
) {
    let mut buf = vec![0u8; 65535];
    let mut pending = Vec::new();

    loop {
        match reader.read(&mut buf).await {
            Ok(0) => {
                log::info!("{} connection closed by remote {}", label, remote);
                break;
            }
            Ok(n) => {
                pending.extend_from_slice(&buf[..n]);
                let mut messages = Vec::new();
                match extract_sip_messages(&pending, &mut messages) {
                    FrameResult::Consumed(consumed) => {
                        if consumed > 0 {
                            pending.drain(..consumed);
                        }
                    }
                    FrameResult::Fatal(reason) => {
                        log::error!("{} framing error from {}: {}", label, remote, reason);
                        break;
                    }
                }
                // A peer that keeps sending without ever yielding a complete
                // message must not be able to grow this buffer without bound.
                if pending.len() > MAX_PENDING {
                    log::error!(
                        "{} peer {} exceeded {} bytes of unframed data — closing",
                        label,
                        remote,
                        MAX_PENDING
                    );
                    break;
                }
                for data in messages {
                    let msg = SipMessage { data, remote };
                    if tx.send(msg).await.is_err() {
                        return;
                    }
                }
            }
            Err(e) => {
                log::error!("{} recv error from {}: {}", label, remote, e);
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TCP transport
// ---------------------------------------------------------------------------

/// A writer trait-object so we can store either a TCP or TLS write half.
type AsyncWriter = Box<dyn tokio::io::AsyncWrite + Send + Unpin>;

#[derive(Clone)]
pub struct TcpTransport {
    writer: Arc<Mutex<AsyncWriter>>,
    local_addr: SocketAddr,
    pub diagnostic: Option<DiagnosticSender>,
}

impl TcpTransport {
    pub async fn connect(
        server_addr: SocketAddr,
    ) -> Result<(Self, mpsc::Receiver<SipMessage>), String> {
        // 10 second connection timeout
        let stream = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            TcpStream::connect(server_addr)
        )
            .await
            .map_err(|_| format!("TCP connect to {} timed out", server_addr))?
            .map_err(|e| format!("TCP connect to {} failed: {}", server_addr, e))?;

        let local_addr = stream
            .local_addr()
            .map_err(|e| format!("Failed to get TCP local addr: {}", e))?;

        log::info!(
            "SIP TCP transport connected {} -> {}",
            local_addr,
            server_addr
        );

        configure_tcp_keepalive(&stream);

        let (read_half, write_half) = stream.into_split();
        let writer: Arc<Mutex<AsyncWriter>> = Arc::new(Mutex::new(Box::new(write_half)));

        let (tx, rx) = mpsc::channel::<SipMessage>(256);

        let remote = server_addr;
        tokio::spawn(async move {
            stream_receive_loop(read_half, tx, remote, "TCP").await;
        });

        Ok((Self { writer, local_addr, diagnostic: None }, rx))
    }

    pub async fn send_to(&self, data: &[u8], _addr: SocketAddr) -> Result<(), String> {
        let mut w = self.writer.lock().await;
        w.write_all(data)
            .await
            .map_err(|e| format!("TCP send error: {}", e))?;
        w.flush()
            .await
            .map_err(|e| format!("TCP flush error: {}", e))?;
        Ok(())
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

// ---------------------------------------------------------------------------
// TLS transport
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct TlsTransport {
    writer: Arc<Mutex<AsyncWriter>>,
    local_addr: SocketAddr,
    pub diagnostic: Option<DiagnosticSender>,
}

impl TlsTransport {
    /// Connect a SIP/TLS transport.
    ///
    /// `insecure` disables certificate and hostname verification. It exists
    /// only for deployments with a private PBX certificate that cannot be
    /// installed into the OS trust store, must be opted into explicitly per
    /// account, and is never enabled by QR provisioning.
    pub async fn connect(
        server_addr: SocketAddr,
        server_name: &str,
        insecure: bool,
    ) -> Result<(Self, mpsc::Receiver<SipMessage>), String> {
        // 10 second TCP connection timeout
        let tcp_stream = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            TcpStream::connect(server_addr)
        )
            .await
            .map_err(|_| format!("TLS/TCP connect to {} timed out", server_addr))?
            .map_err(|e| format!("TLS/TCP connect to {} failed: {}", server_addr, e))?;

        let local_addr = tcp_stream
            .local_addr()
            .map_err(|e| format!("Failed to get TLS local addr: {}", e))?;

        configure_tcp_keepalive(&tcp_stream);

        let tls_config =
            build_tls_config(insecure).map_err(|e| format!("TLS config error: {}", e))?;

        let connector = tokio_rustls::TlsConnector::from(Arc::new(tls_config));
        let dns_name = rustls_pki_types::ServerName::try_from(server_name.to_string())
            .map_err(|e| format!("Invalid server name '{}': {}", server_name, e))?;

        // 10 second TLS handshake timeout
        let tls_stream = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            connector.connect(dns_name, tcp_stream)
        )
            .await
            .map_err(|_| format!("TLS handshake with {} timed out", server_addr))?
            .map_err(|e| format!("TLS handshake with {} failed: {}", server_addr, e))?;

        log::info!(
            "SIP TLS transport connected {} -> {}",
            local_addr,
            server_addr
        );

        let (read_half, write_half) = tokio::io::split(tls_stream);
        let writer: Arc<Mutex<AsyncWriter>> = Arc::new(Mutex::new(Box::new(write_half)));

        let (tx, rx) = mpsc::channel::<SipMessage>(256);

        let remote = server_addr;
        tokio::spawn(async move {
            stream_receive_loop(read_half, tx, remote, "TLS").await;
        });

        Ok((Self { writer, local_addr, diagnostic: None }, rx))
    }

    pub async fn send_to(&self, data: &[u8], _addr: SocketAddr) -> Result<(), String> {
        let mut w = self.writer.lock().await;
        w.write_all(data)
            .await
            .map_err(|e| format!("TLS send error: {}", e))?;
        w.flush()
            .await
            .map_err(|e| format!("TLS flush error: {}", e))?;
        Ok(())
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

// ---------------------------------------------------------------------------
// TLS configuration
// ---------------------------------------------------------------------------

/// Build the TLS client config for SIP/TLS.
///
/// By default the platform trust store is used, so certificates are validated
/// against the OS root CAs *and* any CA an administrator has installed —
/// which is the supported way to run a PBX with a private certificate.
///
/// `insecure` restores the legacy "trust anything" behaviour. It is opt-in per
/// account and logs loudly, because with it enabled any on-path attacker can
/// read the digest credentials and the SDES-SRTP keys off the wire.
fn build_tls_config(insecure: bool) -> Result<rustls::ClientConfig, rustls::Error> {
    // Use ring as the crypto provider (must be explicit in rustls 0.23+)
    let provider = Arc::new(rustls::crypto::ring::default_provider());

    if insecure {
        log::error!(
            "SIP/TLS certificate verification is DISABLED for this account. \
             The connection is not protected against interception — SIP credentials \
             and SRTP keys can be read by anyone on the network path."
        );
        return Ok(rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCert))
            .with_no_client_auth());
    }

    let verifier = rustls_platform_verifier::Verifier::new(provider.clone())
        .map_err(|e| rustls::Error::General(format!("platform verifier: {e}")))?;

    Ok(rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth())
}

/// Certificate verifier that accepts anything. Only reachable when an account
/// explicitly opts into `tls_insecure`.
#[derive(Debug)]
struct AcceptAnyCert;

impl rustls::client::danger::ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls_pki_types::CertificateDer<'_>,
        _intermediates: &[rustls_pki_types::CertificateDer<'_>],
        _server_name: &rustls_pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls_pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls_pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls_pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ED448,
        ]
    }
}
