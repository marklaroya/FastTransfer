//! QUIC transport abstractions and a Quinn-based parallel chunk transport.

use std::{
    fs,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{bail, Context, Result};
use protocol::{
    ChunkAck, ChunkStreamHeader, ResumePlan, TransferManifest, TransferSession, TransferStatus,
    CHUNK_STREAM_MAGIC, CHUNK_STREAM_PREAMBLE_LEN, CHUNK_ACK_FRAME_LEN, RESUME_PLAN_PREAMBLE_LEN,
    TRANSFER_STATUS_FRAME_LEN,
};
use quic::{
    ClientConfig, Connection, Endpoint, RecvStream, SendStream, ServerConfig, TransportConfig,
};
use rcgen::generate_simple_self_signed;
use rustls::{
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    RootCertStore,
};

/// Default DNS name encoded into the development certificate.
pub const DEFAULT_SERVER_NAME: &str = "fasttransfer.local";

const MAX_CONCURRENT_UNI_STREAMS: u32 = 64;
const MAX_MANIFEST_BYTES: usize = 16 * 1024 * 1024;

/// Transport configuration values suitable for a production baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuicEndpointConfig {
    /// Application protocol negotiation string.
    pub alpn: String,
    /// Idle timeout in milliseconds.
    pub idle_timeout_ms: u64,
    /// Keep-alive interval in milliseconds.
    pub keep_alive_interval_ms: u64,
}

impl QuicEndpointConfig {
    /// Returns a conservative production default configuration.
    pub fn production_default() -> Self {
        Self {
            alpn: "fasttransfer/1".to_owned(),
            idle_timeout_ms: 30_000,
            keep_alive_interval_ms: 5_000,
        }
    }
}

/// Minimal QUIC transport interface used by higher-level orchestration.
pub trait QuicTransport {
    /// Opens a transport session.
    fn open_session(&mut self, session: &TransferSession);
}

/// In-memory placeholder transport that records opened sessions.
#[derive(Debug, Default)]
pub struct InMemoryQuicTransport {
    opened_sessions: Vec<String>,
}

impl InMemoryQuicTransport {
    /// Returns session identifiers opened by this transport.
    pub fn opened_sessions(&self) -> &[String] {
        &self.opened_sessions
    }
}

impl QuicTransport for InMemoryQuicTransport {
    fn open_session(&mut self, session: &TransferSession) {
        self.opened_sessions.push(session.id.clone());
    }
}

/// Paths used by a receiver to persist its development certificate and key.
#[derive(Debug, Clone)]
pub struct ReceiverIdentity {
    /// DER-encoded certificate trusted by the sender.
    pub cert_path: PathBuf,
    /// DER-encoded private key used by the receiver.
    pub key_path: PathBuf,
}

/// Thin sender-side QUIC handle used to open a streaming transfer.
#[derive(Debug)]
pub struct QuicSender {
    _endpoint: Endpoint,
    connection: Connection,
}

impl QuicSender {
    /// Connects to a receiver using the provided certificate authority file.
    pub async fn connect(server_addr: SocketAddr, server_name: &str, cert_path: &Path) -> Result<Self> {
        let mut endpoint = Endpoint::client(client_bind_addr(server_addr))
            .context("failed to bind local QUIC client socket")?;
        endpoint.set_default_client_config(build_client_config(cert_path)?);

        let connection = endpoint
            .connect(server_addr, server_name)
            .context("failed to create QUIC connection")?
            .await
            .context("QUIC handshake failed")?;

        Ok(Self {
            _endpoint: endpoint,
            connection,
        })
    }

    /// Opens the bidirectional control stream and sends the transfer manifest.
    pub async fn open_control_stream(&self, manifest: &TransferManifest) -> Result<SenderControlStream> {
        let (mut send, recv) = self
            .connection
            .open_bi()
            .await
            .context("failed to open QUIC control stream")?;
        let encoded_manifest = manifest.encode().context("failed to encode transfer manifest")?;
        send.write_all(&encoded_manifest)
            .await
            .context("failed to write transfer manifest")?;
        send.finish().context("failed to finish manifest half of control stream")?;
        Ok(SenderControlStream { recv })
    }

    /// Sends a single file chunk on its own QUIC stream.
    pub async fn send_chunk(&self, header: &ChunkStreamHeader, bytes: &[u8]) -> Result<()> {
        let expected_size = usize::try_from(header.descriptor.size).context("chunk size exceeded usize")?;
        if bytes.len() != expected_size {
            bail!(
                "chunk payload length mismatch for chunk {}: expected {} bytes, got {} bytes",
                header.descriptor.index,
                expected_size,
                bytes.len()
            );
        }

        let mut stream = self
            .connection
            .open_uni()
            .await
            .context("failed to open QUIC chunk stream")?;
        let encoded_header = header.encode();
        stream
            .write_all(&encoded_header)
            .await
            .context("failed to write chunk stream header")?;
        stream
            .write_all(bytes)
            .await
            .with_context(|| format!("failed to write payload for chunk {}", header.descriptor.index))?;
        stream.finish().context("failed to finish chunk stream")?;
        Ok(())
    }

    /// Closes the connection after the transfer is done.
    pub fn close(&self) {
        self.connection.close(0_u32.into(), b"transfer-complete");
    }
}

/// Sender-side handle for reading control-stream responses from the receiver.
#[derive(Debug)]
pub struct SenderControlStream {
    recv: RecvStream,
}

impl SenderControlStream {
    /// Reads the receiver's resume plan.
    pub async fn read_resume_plan(&mut self) -> Result<ResumePlan> {
        let mut preamble = [0_u8; RESUME_PLAN_PREAMBLE_LEN];
        self.recv
            .read_exact(&mut preamble)
            .await
            .context("failed to read resume-plan preamble")?;
        let missing_count = ResumePlan::decode_preamble(&preamble)
            .context("failed to decode resume-plan preamble")?;
        let mut missing_chunks = Vec::with_capacity(missing_count as usize);

        for _ in 0..missing_count {
            let mut index_bytes = [0_u8; 8];
            self.recv
                .read_exact(&mut index_bytes)
                .await
                .context("failed to read resume-plan chunk index")?;
            missing_chunks.push(u64::from_le_bytes(index_bytes));
        }

        Ok(ResumePlan::from_missing_chunks(missing_chunks))
    }

    /// Reads a chunk acknowledgement from the receiver.
    pub async fn read_chunk_ack(&mut self) -> Result<ChunkAck> {
        let mut frame = [0_u8; CHUNK_ACK_FRAME_LEN];
        self.recv
            .read_exact(&mut frame)
            .await
            .context("failed to read chunk-ack frame")?;
        ChunkAck::decode(&frame).context("failed to decode chunk-ack frame")
    }

    /// Reads the receiver's final transfer status.
    pub async fn read_transfer_status(&mut self) -> Result<TransferStatus> {
        let mut frame = [0_u8; TRANSFER_STATUS_FRAME_LEN];
        self.recv
            .read_exact(&mut frame)
            .await
            .context("failed to read transfer-status frame")?;
        TransferStatus::decode(&frame).context("failed to decode transfer-status frame")
    }
}

/// Receiver-side QUIC endpoint that accepts incoming transfer sessions.
#[derive(Debug)]
pub struct QuicReceiver {
    endpoint: Endpoint,
    local_addr: SocketAddr,
    identity: ReceiverIdentity,
}

impl QuicReceiver {
    /// Binds a QUIC receiver endpoint and ensures a development certificate exists on disk.
    pub fn bind(bind_addr: SocketAddr, cert_path: PathBuf, key_path: PathBuf) -> Result<Self> {
        let server_config = build_server_config(&cert_path, &key_path)?;
        let endpoint = Endpoint::server(server_config, bind_addr)
            .context("failed to bind QUIC receiver endpoint")?;
        let local_addr = endpoint.local_addr().context("failed to read receiver bind address")?;

        Ok(Self {
            endpoint,
            local_addr,
            identity: ReceiverIdentity { cert_path, key_path },
        })
    }

    /// Returns the bound local address.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Returns the persisted receiver identity paths.
    pub fn identity(&self) -> &ReceiverIdentity {
        &self.identity
    }

    /// Waits for a sender and returns the accepted QUIC connection.
    pub async fn accept_connection(&self) -> Result<IncomingConnection> {
        let incoming = self
            .endpoint
            .accept()
            .await
            .context("receiver endpoint closed before an incoming connection arrived")?;
        let connection = incoming.await.context("failed during incoming QUIC handshake")?;

        Ok(IncomingConnection {
            remote_address: connection.remote_address(),
            connection,
        })
    }
}

/// A connected incoming QUIC session.
#[derive(Debug)]
pub struct IncomingConnection {
    connection: Connection,
    /// Remote peer address.
    pub remote_address: SocketAddr,
}

impl IncomingConnection {
    /// Accepts the control stream and returns the manifest plus control writer.
    pub async fn accept_control_stream(&self) -> Result<ReceiverControlStream> {
        let (send, mut recv) = self
            .connection
            .accept_bi()
            .await
            .context("sender closed before opening the control stream")?;
        let manifest = read_manifest_from_stream(&mut recv).await?;
        Ok(ReceiverControlStream { manifest, send })
    }

    /// Accepts the next incoming chunk stream.
    pub async fn accept_chunk_stream(&self) -> Result<IncomingChunkStream> {
        let mut stream = self
            .connection
            .accept_uni()
            .await
            .context("sender closed before opening the next chunk stream")?;
        let mut magic = [0_u8; 4];
        stream
            .read_exact(&mut magic)
            .await
            .context("failed to read incoming chunk-stream magic")?;
        if magic != CHUNK_STREAM_MAGIC {
            bail!("received unexpected stream magic on chunk stream: {:x?}", magic);
        }

        let mut preamble = [0_u8; CHUNK_STREAM_PREAMBLE_LEN];
        preamble[..4].copy_from_slice(&magic);
        stream
            .read_exact(&mut preamble[4..])
            .await
            .context("failed to read chunk-stream header")?;
        let header = ChunkStreamHeader::decode(&preamble)
            .context("failed to decode chunk-stream header")?;

        Ok(IncomingChunkStream { header, stream })
    }
}

/// Receiver-side handle for sending control-stream responses to the sender.
#[derive(Debug)]
pub struct ReceiverControlStream {
    /// Transfer manifest announced by the sender.
    pub manifest: TransferManifest,
    send: SendStream,
}

impl ReceiverControlStream {
    /// Sends the resume plan listing missing chunks.
    pub async fn send_resume_plan(&mut self, plan: &ResumePlan) -> Result<()> {
        self.send
            .write_all(&plan.encode())
            .await
            .context("failed to write resume plan")
    }

    /// Sends an acknowledgement for a verified chunk.
    pub async fn send_chunk_ack(&mut self, ack: ChunkAck) -> Result<()> {
        self.send
            .write_all(&ack.encode())
            .await
            .context("failed to write chunk acknowledgement")
    }

    /// Sends the final transfer status and finishes the control stream.
    pub async fn send_transfer_status(&mut self, status: TransferStatus) -> Result<()> {
        self.send
            .write_all(&status.encode())
            .await
            .context("failed to write transfer status")?;
        self.send
            .finish()
            .context("failed to finish receiver control stream")
    }
}

/// A chunk stream with its decoded header.
#[derive(Debug)]
pub struct IncomingChunkStream {
    /// Header for the incoming chunk payload.
    pub header: ChunkStreamHeader,
    /// QUIC stream carrying the chunk payload.
    pub stream: RecvStream,
}

fn build_server_config(cert_path: &Path, key_path: &Path) -> Result<ServerConfig> {
    let (certificate, private_key) = ensure_receiver_identity(cert_path, key_path)?;
    let mut server_config = ServerConfig::with_single_cert(vec![certificate], private_key)
        .context("failed to construct QUIC server config")?;
    server_config.transport_config(shared_transport_config());
    Ok(server_config)
}

fn build_client_config(cert_path: &Path) -> Result<ClientConfig> {
    let certificate_bytes = fs::read(cert_path)
        .with_context(|| format!("failed to read receiver certificate at {}", cert_path.display()))?;

    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(certificate_bytes))
        .context("failed to add receiver certificate to root store")?;

    let mut client_config = ClientConfig::with_root_certificates(Arc::new(roots))
        .context("failed to construct QUIC client config")?;
    client_config.transport_config(shared_transport_config());
    Ok(client_config)
}

fn shared_transport_config() -> Arc<TransportConfig> {
    let mut transport_config = TransportConfig::default();
    transport_config.max_concurrent_uni_streams(MAX_CONCURRENT_UNI_STREAMS.into());
    Arc::new(transport_config)
}

fn ensure_receiver_identity(
    cert_path: &Path,
    key_path: &Path,
) -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>)> {
    if cert_path.exists() && key_path.exists() {
        let cert_bytes = fs::read(cert_path)
            .with_context(|| format!("failed to read certificate {}", cert_path.display()))?;
        let key_bytes = fs::read(key_path)
            .with_context(|| format!("failed to read key {}", key_path.display()))?;
        return Ok((
            CertificateDer::from(cert_bytes),
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_bytes)),
        ));
    }

    if let Some(parent) = cert_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create certificate directory {}", parent.display()))?;
    }
    if let Some(parent) = key_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create key directory {}", parent.display()))?;
    }

    let certified = generate_simple_self_signed(vec![DEFAULT_SERVER_NAME.to_owned(), "localhost".to_owned()])
        .context("failed to generate development certificate")?;
    let cert_bytes = certified.cert.der().to_vec();
    let key_bytes = certified.key_pair.serialize_der();

    fs::write(cert_path, &cert_bytes)
        .with_context(|| format!("failed to write certificate {}", cert_path.display()))?;
    fs::write(key_path, &key_bytes)
        .with_context(|| format!("failed to write key {}", key_path.display()))?;

    Ok((
        CertificateDer::from(cert_bytes),
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_bytes)),
    ))
}

fn client_bind_addr(server_addr: SocketAddr) -> SocketAddr {
    match server_addr {
        SocketAddr::V4(_) => SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)),
        SocketAddr::V6(_) => SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0)),
    }
}

async fn read_manifest_from_stream(stream: &mut RecvStream) -> Result<TransferManifest> {
    let bytes = stream
        .read_to_end(MAX_MANIFEST_BYTES)
        .await
        .context("failed to read transfer-manifest payload")?;
    TransferManifest::decode(&bytes).context("invalid transfer manifest payload")
}



/// Sender-side control channel for streaming package metadata and results.
#[derive(Debug)]
pub struct StreamingSenderControlStream {
    send: SendStream,
    recv: RecvStream,
}

impl StreamingSenderControlStream {
    /// Sends a streaming sender-control message.
    pub async fn send_message(&mut self, message: &protocol::SenderControlMessage) -> Result<()> {
        let encoded = message
            .encode()
            .context("failed to encode streaming sender control message")?;
        self.send
            .write_all(&encoded)
            .await
            .context("failed to write streaming sender control message")
    }

    /// Reads a streaming receiver-control message.
    pub async fn read_message(&mut self) -> Result<protocol::ReceiverControlMessage> {
        let frame = read_stream_control_frame(&mut self.recv).await?;
        protocol::ReceiverControlMessage::decode(&frame)
            .context("failed to decode streaming receiver control message")
    }

    /// Finishes the streaming control channel after the transfer completes.
    pub fn finish(&mut self) -> Result<()> {
        self.send
            .finish()
            .context("failed to finish sender streaming control stream")
    }
}

/// Receiver-side control channel for streaming package metadata and results.
#[derive(Debug)]
pub struct StreamingReceiverControlStream {
    send: SendStream,
    recv: RecvStream,
}

impl StreamingReceiverControlStream {
    /// Reads a streaming sender-control message.
    pub async fn read_message(&mut self) -> Result<protocol::SenderControlMessage> {
        let frame = read_stream_control_frame(&mut self.recv).await?;
        protocol::SenderControlMessage::decode(&frame)
            .context("failed to decode streaming sender control message")
    }

    /// Sends a streaming receiver-control message.
    pub async fn send_message(&mut self, message: &protocol::ReceiverControlMessage) -> Result<()> {
        let encoded = message
            .encode()
            .context("failed to encode streaming receiver control message")?;
        self.send
            .write_all(&encoded)
            .await
            .context("failed to write streaming receiver control message")
    }

    /// Finishes the receiver control stream.
    pub fn finish(&mut self) -> Result<()> {
        self.send
            .finish()
            .context("failed to finish receiver streaming control stream")
    }
}

impl QuicSender {
    /// Opens a bidirectional control stream used by the progressive streaming pipeline.
    pub async fn open_streaming_control_stream(&self) -> Result<StreamingSenderControlStream> {
        let (send, recv) = self
            .connection
            .open_bi()
            .await
            .context("failed to open QUIC streaming control stream")?;
        Ok(StreamingSenderControlStream { send, recv })
    }
}

impl IncomingConnection {
    /// Accepts the bidirectional control stream used by the progressive streaming pipeline.
    pub async fn accept_streaming_control_stream(&self) -> Result<StreamingReceiverControlStream> {
        let (send, recv) = self
            .connection
            .accept_bi()
            .await
            .context("sender closed before opening the streaming control stream")?;
        Ok(StreamingReceiverControlStream { send, recv })
    }
}

async fn read_stream_control_frame(stream: &mut RecvStream) -> Result<Vec<u8>> {
    let mut header = [0_u8; protocol::STREAM_CONTROL_HEADER_LEN];
    stream
        .read_exact(&mut header)
        .await
        .context("failed to read streaming control frame header")?;

    let mut len_bytes = [0_u8; 4];
    len_bytes.copy_from_slice(&header[4..8]);
    let payload_len = u32::from_le_bytes(len_bytes) as usize;
    let mut payload = vec![0_u8; payload_len];
    if payload_len > 0 {
        stream
            .read_exact(&mut payload)
            .await
            .context("failed to read streaming control frame payload")?;
    }

    let mut frame = Vec::with_capacity(protocol::STREAM_CONTROL_HEADER_LEN + payload_len);
    frame.extend_from_slice(&header);
    frame.extend_from_slice(&payload);
    Ok(frame)
}
