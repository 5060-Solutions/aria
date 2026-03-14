use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{mpsc, Mutex};

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

/// Extract complete SIP messages from a byte buffer using Content-Length
/// framing. Returns the number of bytes consumed.
fn extract_sip_messages(buf: &[u8], out: &mut Vec<Vec<u8>>) -> usize {
    let mut consumed = 0;

    loop {
        let remaining = &buf[consumed..];
        if remaining.is_empty() {
            break;
        }

        // Find the header/body separator: \r\n\r\n
        let sep = match find_double_crlf(remaining) {
            Some(pos) => pos,
            None => break, // incomplete headers
        };

        let body_start = sep + 4; // past the double-CRLF

        let headers = match std::str::from_utf8(&remaining[..sep]) {
            Ok(s) => s,
            Err(_) => break,
        };

        let content_length = parse_content_length(headers);

        let total_len = body_start + content_length;
        if remaining.len() < total_len {
            break; // incomplete body
        }

        out.push(remaining[..total_len].to_vec());
        consumed += total_len;
    }

    consumed
}

fn find_double_crlf(data: &[u8]) -> Option<usize> {
    data.windows(4).position(|w| w == b"\r\n\r\n")
}

fn parse_content_length(headers: &str) -> usize {
    for line in headers.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("content-length:") || lower.starts_with("l:") {
            if let Some(val) = line.split(':').nth(1) {
                if let Ok(n) = val.trim().parse::<usize>() {
                    return n;
                }
            }
        }
    }
    0
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
                let consumed = extract_sip_messages(&pending, &mut messages);
                if consumed > 0 {
                    pending.drain(..consumed);
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
    pub async fn connect(
        server_addr: SocketAddr,
        server_name: &str,
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

        // Build a TLS config that accepts any certificate (many PBXes use self-signed)
        let tls_config =
            build_permissive_tls_config().map_err(|e| format!("TLS config error: {}", e))?;

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
// Permissive TLS config (accepts self-signed certs)
// ---------------------------------------------------------------------------

fn build_permissive_tls_config() -> Result<rustls::ClientConfig, rustls::Error> {
    // Use ring as the crypto provider (must be explicit in rustls 0.23+)
    let provider = rustls::crypto::ring::default_provider();
    
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyCert))
        .with_no_client_auth();
    Ok(config)
}

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
