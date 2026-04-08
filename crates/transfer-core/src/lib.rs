//! High-level transfer planning, chunk scheduling, file IO, and CLI orchestration.

mod file_io;
mod progress;
mod streaming;

use std::{
    collections::BTreeSet,
    fs as stdfs,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use chunker::FixedChunker;
use discovery::{local_loopback_advertisement, short_fingerprint, NearbyReceiver, PeerAdvertisement};
use file_io::PackageChunkTask;
use integrity::{format_sha256, sha256_bytes, IntegrityReport};
use progress::{ProgressListener, ProgressReporter};
use protocol::{
    ChunkAck, ChunkDescriptor, ChunkStreamHeader, PackageEntry, PackageItemKind, ResumePlan,
    TransferManifest, TransferMode, TransferSession, TransferStatus,
};
use quic_transport::{
    InMemoryQuicTransport, QuicEndpointConfig, QuicReceiver, QuicSender, DEFAULT_SERVER_NAME,
};
use resume::{PersistentResumeState, ResumeState};
use tokio::{sync::Semaphore, task::JoinSet};

pub use file_io::SourceInspection as TransferSourceSummary;
pub use progress::ProgressUpdate as TransferProgress;
pub const DEFAULT_CHUNK_SIZE: u32 = 4_194_304;
pub const DEFAULT_PARALLELISM: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoTuneProfile {
    TinyTransfer,
    ManySmallFiles,
    LargeFiles,
    Mixed,
}

impl AutoTuneProfile {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TinyTransfer => "tiny_transfer",
            Self::ManySmallFiles => "many_small_files",
            Self::LargeFiles => "large_files",
            Self::Mixed => "mixed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AutoTuneSettings {
    pub chunk_size: u32,
    pub parallelism: usize,
    pub profile: AutoTuneProfile,
    pub reason: String,
}

pub fn recommend_transfer_tuning(summary: &TransferSourceSummary) -> AutoTuneSettings {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;

    let avg_file_size = if summary.total_files == 0 {
        0
    } else {
        summary.total_bytes / summary.total_files
    };

    let tiny_transfer = summary.total_bytes <= 128 * MIB;
    let many_small_files = summary.total_files >= 1_500
        || (summary.total_files >= 200 && avg_file_size <= 512 * KIB);
    let very_large_files = avg_file_size >= 64 * MIB || summary.total_bytes >= 16 * GIB;
    let large_file_heavy = summary.total_files <= 256 && avg_file_size >= 8 * MIB;

    if tiny_transfer {
        return AutoTuneSettings {
            chunk_size: 1_048_576,
            parallelism: 4,
            profile: AutoTuneProfile::TinyTransfer,
            reason: format!(
                "Small transfer detected ({} bytes across {} files).",
                summary.total_bytes, summary.total_files
            ),
        };
    }

    if many_small_files {
        return AutoTuneSettings {
            chunk_size: 1_048_576,
            parallelism: 6,
            profile: AutoTuneProfile::ManySmallFiles,
            reason: format!(
                "Many small files detected ({} files, average {} bytes per file).",
                summary.total_files, avg_file_size
            ),
        };
    }

    if very_large_files {
        return AutoTuneSettings {
            chunk_size: 8_388_608,
            parallelism: 8,
            profile: AutoTuneProfile::LargeFiles,
            reason: format!(
                "Very large payload profile detected ({} bytes total, average {} bytes per file).",
                summary.total_bytes, avg_file_size
            ),
        };
    }

    if large_file_heavy {
        return AutoTuneSettings {
            chunk_size: 4_194_304,
            parallelism: 8,
            profile: AutoTuneProfile::LargeFiles,
            reason: format!(
                "Large-file workload detected ({} files, average {} bytes per file).",
                summary.total_files, avg_file_size
            ),
        };
    }

    AutoTuneSettings {
        chunk_size: 2_097_152,
        parallelism: 6,
        profile: AutoTuneProfile::Mixed,
        reason: format!(
            "Mixed workload detected ({} files, average {} bytes per file).",
            summary.total_files, avg_file_size
        ),
    }
}


#[derive(Debug)]
pub struct TransferPlan {
    pub session: TransferSession,
    pub peers: Vec<PeerAdvertisement>,
    pub chunks: Vec<ChunkDescriptor>,
    pub transport: QuicEndpointConfig,
    pub resume: ResumeState,
    pub integrity: IntegrityReport,
}

#[derive(Debug, Clone)]
pub struct SendRequest {
    pub server_addr: SocketAddr,
    pub source_path: PathBuf,
    pub certificate_path: PathBuf,
    pub server_name: String,
    pub chunk_size: u32,
    pub parallelism: usize,
}

impl SendRequest {
    pub fn with_default_server_name(
        server_addr: SocketAddr,
        source_path: PathBuf,
        certificate_path: PathBuf,
    ) -> Self {
        Self {
            server_addr,
            source_path,
            certificate_path,
            server_name: DEFAULT_SERVER_NAME.to_owned(),
            chunk_size: DEFAULT_CHUNK_SIZE,
            parallelism: DEFAULT_PARALLELISM,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiscoveredSendRequest {
    pub receiver: NearbyReceiver,
    pub source_path: PathBuf,
    pub trust_cache_dir: PathBuf,
    pub server_addr: Option<SocketAddr>,
    pub server_name: String,
    pub chunk_size: u32,
    pub parallelism: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiverTrustState {
    TrustedForSession,
    KnownDevice,
    FingerprintMismatch,
}

impl ReceiverTrustState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TrustedForSession => "trusted_for_session",
            Self::KnownDevice => "known_device",
            Self::FingerprintMismatch => "fingerprint_mismatch",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReceiverTrustReport {
    pub peer_id: String,
    pub device_name: String,
    pub fingerprint_hex: String,
    pub short_fingerprint: String,
    pub state: ReceiverTrustState,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct ReceiveRequest {
    pub bind_addr: SocketAddr,
    pub output_dir: PathBuf,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ReceiverReady {
    pub bind_addr: SocketAddr,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct TransferSummary {
    pub file_name: String,
    pub bytes_transferred: u64,
    pub elapsed: Duration,
    pub average_mib_per_sec: f64,
    pub completed_chunks: u64,
    pub completed_files: u64,
    pub total_directories: u64,
    pub sha256_hex: String,
    pub integrity_verified: bool,
}

#[derive(Debug, Clone)]
pub struct ReceivedTransfer {
    pub summary: TransferSummary,
    pub saved_path: PathBuf,
    pub remote_address: SocketAddr,
}

#[derive(Debug)]
pub struct ReceiverApp {
    output_dir: PathBuf,
    transport: QuicReceiver,
}

impl ReceiverApp {
    pub fn ready(&self) -> ReceiverReady {
        ReceiverReady {
            bind_addr: self.transport.local_addr(),
            cert_path: self.transport.identity().cert_path.clone(),
            key_path: self.transport.identity().key_path.clone(),
        }
    }

    pub async fn receive_once(self) -> Result<ReceivedTransfer> {
        self.receive_once_internal(None, true).await
    }

    pub async fn receive_once_with_progress<F>(self, on_progress: F) -> Result<ReceivedTransfer>
    where
        F: Fn(TransferProgress) + Send + Sync + 'static,
    {
        self.receive_once_internal(Some(Arc::new(on_progress)), false).await
    }

    async fn receive_once_internal(
        self,
        progress_listener: Option<ProgressListener>,
        render_terminal: bool,
    ) -> Result<ReceivedTransfer> {
        streaming::receive_streaming(
            self.output_dir,
            self.transport,
            progress_listener,
            render_terminal,
        )
        .await
    }
}

pub async fn inspect_transfer_source(source_path: &Path) -> Result<TransferSourceSummary> {
    file_io::inspect_source(source_path).await
}
pub fn assess_discovered_receiver(receiver: &NearbyReceiver, trust_cache_dir: &Path) -> Result<ReceiverTrustReport> {
    if receiver.certificate_der.is_empty() {
        bail!("discovered receiver {} did not include a certificate", receiver.device_name);
    }

    let actual_fingerprint = format_sha256(&sha256_bytes(&receiver.certificate_der));
    if actual_fingerprint != receiver.certificate_sha256_hex {
        bail!(
            "discovered receiver {} advertised fingerprint {} but the certificate hashes to {}",
            receiver.device_name,
            receiver.certificate_sha256_hex,
            actual_fingerprint
        );
    }

    let cached_fingerprint = read_cached_fingerprint(trust_cache_dir, &receiver.peer_id)?;
    let short_fingerprint_value = short_fingerprint(&receiver.certificate_sha256_hex);

    let (state, message) = match cached_fingerprint {
        None => (
            ReceiverTrustState::TrustedForSession,
            format!(
                "{} ({short_fingerprint_value}) is trusted for this session via LAN discovery.",
                receiver.device_name
            ),
        ),
        Some(cached) if cached == receiver.certificate_sha256_hex => (
            ReceiverTrustState::KnownDevice,
            format!("Known device {} ({short_fingerprint_value}).", receiver.device_name),
        ),
        Some(cached) => (
            ReceiverTrustState::FingerprintMismatch,
            format!(
                "Device fingerprint changed for {}: expected {}, discovered {}.",
                receiver.device_name,
                short_fingerprint(&cached),
                short_fingerprint_value
            ),
        ),
    };

    Ok(ReceiverTrustReport {
        peer_id: receiver.peer_id.clone(),
        device_name: receiver.device_name.clone(),
        fingerprint_hex: receiver.certificate_sha256_hex.clone(),
        short_fingerprint: short_fingerprint_value,
        state,
        message,
    })
}

pub fn prepare_discovered_send_request(request: DiscoveredSendRequest) -> Result<(SendRequest, ReceiverTrustReport)> {
    let trust = assess_discovered_receiver(&request.receiver, &request.trust_cache_dir)?;
    if trust.state == ReceiverTrustState::FingerprintMismatch {
        bail!(trust.message.clone());
    }

    let cert_path = persist_discovered_certificate(&request.receiver, &request.trust_cache_dir)?;
    persist_trust_cache(&request.receiver, &request.trust_cache_dir)?;
    let server_addr = request
        .server_addr
        .or_else(|| request.receiver.primary_address())
        .with_context(|| format!("discovered receiver {} did not publish any reachable addresses", request.receiver.device_name))?;

    Ok((
        SendRequest {
            server_addr,
            source_path: request.source_path,
            certificate_path: cert_path,
            server_name: request.server_name,
            chunk_size: request.chunk_size,
            parallelism: request.parallelism,
        },
        trust,
    ))
}

pub fn bind_receiver(request: ReceiveRequest) -> Result<ReceiverApp> {
    let transport = QuicReceiver::bind(request.bind_addr, request.cert_path, request.key_path)?;
    Ok(ReceiverApp {
        output_dir: request.output_dir,
        transport,
    })
}

pub async fn send_file(request: SendRequest) -> Result<TransferSummary> {
    send_file_internal(request, None, true).await
}

pub async fn send_file_with_progress<F>(request: SendRequest, on_progress: F) -> Result<TransferSummary>
where
    F: Fn(TransferProgress) + Send + Sync + 'static,
{
    send_file_internal(request, Some(Arc::new(on_progress)), false).await
}

async fn send_file_internal(
    request: SendRequest,
    progress_listener: Option<ProgressListener>,
    render_terminal: bool,
) -> Result<TransferSummary> {
    streaming::send_streaming(request, progress_listener, render_terminal).await
}
pub fn build_transfer_plan(
    file_name: impl Into<String>,
    total_bytes: u64,
    chunk_size: u32,
    device_name: impl Into<String>,
) -> TransferPlan {
    let file_name = file_name.into();
    let session = TransferSession {
        id: format!("session-{file_name}-{total_bytes}"),
        file_name,
        total_bytes,
        chunk_size: chunk_size.max(1),
        mode: TransferMode::Send,
    };

    let chunks = FixedChunker::new(total_bytes, chunk_size).descriptors();
    let peers = vec![local_loopback_advertisement(device_name)];
    let resume = ResumeState::new(session.clone());
    let transport = QuicEndpointConfig::production_default();
    let integrity = integrity::checksum(session.id.as_bytes());

    let mut warmup_transport = InMemoryQuicTransport::default();
    quic_transport::QuicTransport::open_session(&mut warmup_transport, &session);

    TransferPlan {
        session,
        peers,
        chunks,
        transport,
        resume,
        integrity,
    }
}

fn configure_reporter(
    reporter: ProgressReporter,
    listener: Option<ProgressListener>,
    render_terminal: bool,
) -> ProgressReporter {
    let reporter = if let Some(listener) = listener {
        reporter.with_listener(listener)
    } else {
        reporter
    };

    if render_terminal {
        reporter
    } else {
        reporter.without_terminal()
    }
}

fn sender_checkpoint_base_dir(source_path: &Path) -> &Path {
    source_path.parent().unwrap_or_else(|| Path::new("."))
}

pub fn ensure_preferred_receive_dir() -> Result<PathBuf> {
    let directory = preferred_receive_dir()?;
    stdfs::create_dir_all(&directory)
        .with_context(|| format!("failed to create receive directory {}", directory.display()))?;
    Ok(directory)
}

pub fn preferred_receive_dir_label(path: &Path) -> String {
    if let Some(downloads_dir) = dirs::download_dir() {
        if let Ok(relative) = path.strip_prefix(&downloads_dir) {
            return join_display_label("Downloads", relative);
        }
    }

    if let Some(desktop_dir) = dirs::desktop_dir() {
        if let Ok(relative) = path.strip_prefix(&desktop_dir) {
            return join_display_label("Desktop", relative);
        }
    }

    path.display().to_string()
}

fn preferred_receive_dir() -> Result<PathBuf> {
    if let Some(downloads_dir) = dirs::download_dir() {
        return Ok(downloads_dir.join("FastTransfer"));
    }

    if let Some(desktop_dir) = dirs::desktop_dir() {
        return Ok(desktop_dir.join("FastTransfer"));
    }

    bail!("failed to resolve a Downloads or Desktop directory for received files")
}

fn join_display_label(root: &str, relative: &Path) -> String {
    let suffix = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");

    if suffix.is_empty() {
        root.to_owned()
    } else {
        format!("{root}/{suffix}")
    }
}

fn trust_cache_file(trust_cache_dir: &Path, peer_id: &str) -> PathBuf {
    trust_cache_dir.join("receivers").join(format!("{}.trust", sanitize_id(peer_id)))
}

fn receiver_certificate_file(trust_cache_dir: &Path, peer_id: &str) -> PathBuf {
    trust_cache_dir.join("certs").join(format!("{}.der", sanitize_id(peer_id)))
}

fn sanitize_id(value: &str) -> String {
    let mut sanitized = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            sanitized.push(ch.to_ascii_lowercase());
        } else if sanitized.is_empty() || !sanitized.ends_with('-') {
            sanitized.push('-');
        }
    }
    sanitized.trim_matches('-').to_owned()
}

fn read_cached_fingerprint(trust_cache_dir: &Path, peer_id: &str) -> Result<Option<String>> {
    let cache_path = trust_cache_file(trust_cache_dir, peer_id);
    if !cache_path.exists() {
        return Ok(None);
    }

    let content = stdfs::read_to_string(&cache_path)
        .with_context(|| format!("failed to read receiver trust cache {}", cache_path.display()))?;
    for line in content.lines() {
        if let Some(value) = line.strip_prefix("fingerprint=") {
            return Ok(Some(value.trim().to_owned()));
        }
    }

    bail!("receiver trust cache {} is missing a fingerprint entry", cache_path.display())
}

fn persist_trust_cache(receiver: &NearbyReceiver, trust_cache_dir: &Path) -> Result<()> {
    let cache_path = trust_cache_file(trust_cache_dir, &receiver.peer_id);
    if let Some(parent) = cache_path.parent() {
        stdfs::create_dir_all(parent)
            .with_context(|| format!("failed to create receiver trust directory {}", parent.display()))?;
    }

    let content = format!(
        "peer_id={}\ndevice_name={}\nfingerprint={}\n",
        receiver.peer_id,
        receiver.device_name,
        receiver.certificate_sha256_hex,
    );
    stdfs::write(&cache_path, content)
        .with_context(|| format!("failed to write receiver trust cache {}", cache_path.display()))
}

fn persist_discovered_certificate(receiver: &NearbyReceiver, trust_cache_dir: &Path) -> Result<PathBuf> {
    let cert_path = receiver_certificate_file(trust_cache_dir, &receiver.peer_id);
    if let Some(parent) = cert_path.parent() {
        stdfs::create_dir_all(parent)
            .with_context(|| format!("failed to create receiver certificate cache {}", parent.display()))?;
    }
    stdfs::write(&cert_path, &receiver.certificate_der)
        .with_context(|| format!("failed to write receiver certificate {}", cert_path.display()))?;
    Ok(cert_path)
}

#[derive(Debug, Clone)]
struct ManifestChunk {
    file_index: usize,
    relative_path: String,
    descriptor: ChunkDescriptor,
}

fn manifest_chunks(manifest: &TransferManifest) -> Result<Vec<ManifestChunk>> {
    let mut chunks = Vec::with_capacity(manifest.chunk_count as usize);
    for (file_index, entry) in manifest.file_entries() {
        let local_descriptors = file_descriptors(manifest.chunk_size, entry);
        if local_descriptors.len() as u64 != entry.chunk_count {
            bail!(
                "manifest entry {} reported {} chunks but planned {}",
                entry.relative_path,
                entry.chunk_count,
                local_descriptors.len()
            );
        }
        for descriptor in local_descriptors {
            chunks.push(ManifestChunk {
                file_index,
                relative_path: entry.relative_path.clone(),
                descriptor: ChunkDescriptor {
                    index: entry.first_chunk_index + descriptor.index,
                    offset: descriptor.offset,
                    size: descriptor.size,
                },
            });
        }
    }
    if chunks.len() as u64 != manifest.chunk_count {
        bail!("manifest chunk count {} did not match planned {}", manifest.chunk_count, chunks.len());
    }
    Ok(chunks)
}

fn file_descriptors(chunk_size: u32, entry: &PackageEntry) -> Vec<ChunkDescriptor> {
    (0..entry.chunk_count)
        .map(|index| {
            let offset = index * u64::from(chunk_size);
            let remaining = entry.file_size.saturating_sub(offset);
            ChunkDescriptor {
                index,
                offset,
                size: remaining.min(u64::from(chunk_size)) as u32,
            }
        })
        .collect()
}

fn completed_indices_from_missing(chunks: &[PackageChunkTask], missing: &BTreeSet<u64>) -> Vec<u64> {
    chunks
        .iter()
        .filter(|chunk| !missing.contains(&chunk.descriptor.index))
        .map(|chunk| chunk.descriptor.index)
        .collect()
}

trait ChunkLike {
    fn descriptor(&self) -> &ChunkDescriptor;
}

impl ChunkLike for PackageChunkTask {
    fn descriptor(&self) -> &ChunkDescriptor { &self.descriptor }
}

impl ChunkLike for ManifestChunk {
    fn descriptor(&self) -> &ChunkDescriptor { &self.descriptor }
}

fn total_bytes_for_indices<T>(chunks: &[T], indices: &[u64]) -> Result<u64>
where
    T: ChunkLike,
{
    let mut total = 0_u64;
    for chunk_index in indices {
        total = total
            .checked_add(u64::from(chunk_for_index(chunks, *chunk_index)?.descriptor().size))
            .context("chunk byte count overflowed u64")?;
    }
    Ok(total)
}

fn chunk_for_index<'a, T>(chunks: &'a [T], chunk_index: u64) -> Result<&'a T>
where
    T: ChunkLike,
{
    chunks
        .get(chunk_index as usize)
        .filter(|chunk| chunk.descriptor().index == chunk_index)
        .ok_or_else(|| anyhow::anyhow!("missing descriptor for chunk {}", chunk_index))
}

fn completed_files_from_missing(manifest: &TransferManifest, missing: &BTreeSet<u64>) -> u64 {
    manifest
        .file_entries()
        .filter(|(_, entry)| {
            entry.chunk_count == 0
                || (entry.first_chunk_index..entry.first_chunk_index + entry.chunk_count)
                    .all(|chunk_index| !missing.contains(&chunk_index))
        })
        .count() as u64
}

fn completed_files_from_chunks(manifest: &TransferManifest, completed_chunks: &BTreeSet<u64>) -> u64 {
    manifest
        .file_entries()
        .filter(|(_, entry)| {
            entry.chunk_count == 0
                || (entry.first_chunk_index..entry.first_chunk_index + entry.chunk_count)
                    .all(|chunk_index| completed_chunks.contains(&chunk_index))
        })
        .count() as u64
}

fn validate_resume_plan(manifest: &TransferManifest, resume_plan: &ResumePlan) -> Result<BTreeSet<u64>> {
    let mut missing = BTreeSet::new();
    for chunk_index in &resume_plan.missing_chunks {
        if *chunk_index >= manifest.chunk_count {
            bail!(
                "receiver requested out-of-range chunk {} for transfer with {} chunks",
                chunk_index,
                manifest.chunk_count
            );
        }
        if !missing.insert(*chunk_index) {
            bail!("receiver listed chunk {} more than once in the resume plan", chunk_index);
        }
    }
    Ok(missing)
}

fn validate_chunk_header(manifest: &TransferManifest, header: &ChunkStreamHeader, chunks: &[ManifestChunk]) -> Result<()> {
    if header.descriptor.index >= manifest.chunk_count {
        bail!(
            "received out-of-range chunk index {} for transfer with {} chunks",
            header.descriptor.index,
            manifest.chunk_count
        );
    }
    let expected = chunk_for_index(chunks, header.descriptor.index)?;
    if header.file_index as usize != expected.file_index {
        bail!(
            "chunk {} referenced file index {} but expected {}",
            header.descriptor.index,
            header.file_index,
            expected.file_index
        );
    }
    if header.descriptor.offset != expected.descriptor.offset || header.descriptor.size != expected.descriptor.size {
        bail!(
            "chunk {} descriptor mismatch: expected offset {} size {}, got offset {} size {}",
            header.descriptor.index,
            expected.descriptor.offset,
            expected.descriptor.size,
            header.descriptor.offset,
            header.descriptor.size
        );
    }
    Ok(())
}

fn display_current_path(manifest: &TransferManifest, relative_path: &str) -> String {
    match manifest.root_kind {
        PackageItemKind::Directory => format!("{}/{}", manifest.root_name, relative_path),
        PackageItemKind::File => relative_path.to_owned(),
    }
}

fn summary_sha256_hex(manifest: &TransferManifest) -> Result<String> {
    let files = manifest.file_entries().collect::<Vec<_>>();
    if files.len() == 1 && manifest.root_kind == PackageItemKind::File {
        return Ok(format_sha256(&files[0].1.file_sha256));
    }
    Ok(format_sha256(&manifest.fingerprint()?))
}
#[cfg(test)]
mod tests {
    use super::build_transfer_plan;

    #[test]
    fn transfer_plan_contains_expected_chunks() {
        let plan = build_transfer_plan("example.bin", 10_000, 4_096, "devbox");
        assert_eq!(plan.chunks.len(), 3);
        assert_eq!(plan.resume.pending_chunks().len(), 3);
        assert_eq!(plan.peers[0].transport, "quic");
    }
}





