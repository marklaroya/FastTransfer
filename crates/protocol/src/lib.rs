//! Shared protocol primitives used across the FastTransfer engine.

use std::fmt;

use integrity::{sha256_bytes, Sha256Hash, SHA256_LEN};
use serde::{Deserialize, Serialize};

mod streaming;
pub use streaming::*;

/// Current protocol version for wire compatibility checks.
pub const PROTOCOL_VERSION: u16 = 2;

/// Magic bytes used to frame a transfer manifest on the control stream.
pub const TRANSFER_MANIFEST_MAGIC: [u8; 4] = *b"FTM1";

/// Magic bytes used to frame a chunk data stream.
pub const CHUNK_STREAM_MAGIC: [u8; 4] = *b"FTC1";

/// Magic bytes used to frame a resume plan response.
pub const RESUME_PLAN_MAGIC: [u8; 4] = *b"FTR1";

/// Magic bytes used to acknowledge a completed chunk.
pub const CHUNK_ACK_MAGIC: [u8; 4] = *b"FTA1";

/// Magic bytes used to signal transfer completion.
pub const TRANSFER_STATUS_MAGIC: [u8; 4] = *b"FTS1";

/// Size of the fixed-length transfer-manifest header.
pub const TRANSFER_MANIFEST_HEADER_LEN: usize = 6;

/// Size of the fixed-length chunk-stream preamble.
pub const CHUNK_STREAM_PREAMBLE_LEN: usize = 60;

/// Size of the fixed-length resume-plan preamble.
pub const RESUME_PLAN_PREAMBLE_LEN: usize = 12;

/// Size of a chunk-ack control frame.
pub const CHUNK_ACK_FRAME_LEN: usize = 12;

/// Size of a transfer-status control frame.
pub const TRANSFER_STATUS_FRAME_LEN: usize = 5;

/// Transfer mode selected for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferMode {
    /// Push data to a receiving peer.
    Send,
    /// Pull data from a sending peer.
    Receive,
}

/// Metadata that identifies a transfer session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferSession {
    /// Stable session identifier.
    pub id: String,
    /// Human-readable package root name.
    pub file_name: String,
    /// Total package size in bytes.
    pub total_bytes: u64,
    /// Planned chunk size in bytes.
    pub chunk_size: u32,
    /// Session transfer direction.
    pub mode: TransferMode,
}

impl TransferSession {
    /// Returns the number of chunks needed to transfer the session payload.
    pub fn chunk_count(&self) -> u64 {
        if self.total_bytes == 0 {
            return 0;
        }

        let chunk_size = u64::from(self.chunk_size.max(1));
        self.total_bytes.div_ceil(chunk_size)
    }
}

/// Type of item described by a package manifest entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageItemKind {
    /// A regular file whose contents must be transferred.
    File,
    /// A directory entry that should exist on the receiver.
    Directory,
}

impl PackageItemKind {
    /// Returns `true` when this entry refers to a regular file.
    pub fn is_file(self) -> bool {
        matches!(self, Self::File)
    }

    /// Returns `true` when this entry refers to a directory.
    pub fn is_directory(self) -> bool {
        matches!(self, Self::Directory)
    }
}

/// Metadata describing a discrete file chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkDescriptor {
    /// Zero-based global chunk index within the package.
    pub index: u64,
    /// Byte offset within the source file.
    pub offset: u64,
    /// Chunk payload size in bytes.
    pub size: u32,
}

/// Header sent before a chunk payload begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkStreamHeader {
    /// Package entry index for the target file.
    pub file_index: u32,
    /// Planned chunk location within the package and file.
    pub descriptor: ChunkDescriptor,
    /// Expected SHA-256 of the chunk payload.
    pub chunk_sha256: Sha256Hash,
}

impl ChunkStreamHeader {
    /// Encodes a chunk-stream header.
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(CHUNK_STREAM_PREAMBLE_LEN);
        bytes.extend_from_slice(&CHUNK_STREAM_MAGIC);
        bytes.extend_from_slice(&self.file_index.to_le_bytes());
        bytes.extend_from_slice(&self.descriptor.index.to_le_bytes());
        bytes.extend_from_slice(&self.descriptor.offset.to_le_bytes());
        bytes.extend_from_slice(&self.descriptor.size.to_le_bytes());
        bytes.extend_from_slice(&self.chunk_sha256);
        bytes
    }

    /// Decodes a fixed-length chunk-stream preamble.
    pub fn decode(preamble: &[u8; CHUNK_STREAM_PREAMBLE_LEN]) -> Result<Self, ProtocolError> {
        if &preamble[..4] != CHUNK_STREAM_MAGIC.as_slice() {
            return Err(ProtocolError::InvalidMagic);
        }

        let mut file_index_bytes = [0_u8; 4];
        file_index_bytes.copy_from_slice(&preamble[4..8]);

        let mut index_bytes = [0_u8; 8];
        index_bytes.copy_from_slice(&preamble[8..16]);

        let mut offset_bytes = [0_u8; 8];
        offset_bytes.copy_from_slice(&preamble[16..24]);

        let mut size_bytes = [0_u8; 4];
        size_bytes.copy_from_slice(&preamble[24..28]);

        let size = u32::from_le_bytes(size_bytes);
        if size == 0 {
            return Err(ProtocolError::InvalidChunkSize(0));
        }

        let mut chunk_sha256 = [0_u8; SHA256_LEN];
        chunk_sha256.copy_from_slice(&preamble[28..60]);

        Ok(Self {
            file_index: u32::from_le_bytes(file_index_bytes),
            descriptor: ChunkDescriptor {
                index: u64::from_le_bytes(index_bytes),
                offset: u64::from_le_bytes(offset_bytes),
                size,
            },
            chunk_sha256,
        })
    }
}

/// Metadata for a single package item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageEntry {
    /// Path relative to the selected source root.
    pub relative_path: String,
    /// Item kind.
    pub item_kind: PackageItemKind,
    /// File size in bytes, or zero for directories.
    pub file_size: u64,
    /// Expected SHA-256 of the file contents, or all zeros for directories.
    pub file_sha256: Sha256Hash,
    /// First global chunk index assigned to this file.
    pub first_chunk_index: u64,
    /// Number of chunks assigned to this file.
    pub chunk_count: u64,
}

impl PackageEntry {
    /// Returns `true` if the entry represents a regular file.
    pub fn is_file(&self) -> bool {
        self.item_kind.is_file()
    }

    /// Returns `true` if the entry represents a directory.
    pub fn is_directory(&self) -> bool {
        self.item_kind.is_directory()
    }

    /// Returns `true` if the entry has no payload chunks.
    pub fn is_empty_file(&self) -> bool {
        self.is_file() && self.file_size == 0 && self.chunk_count == 0
    }

    /// Returns `true` if the given global chunk index belongs to this entry.
    pub fn contains_chunk(&self, chunk_index: u64) -> bool {
        self.is_file()
            && chunk_index >= self.first_chunk_index
            && chunk_index < self.first_chunk_index.saturating_add(self.chunk_count)
    }
}

/// Metadata sent once before chunk streams begin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferManifest {
    /// Human-readable selected root name.
    pub root_name: String,
    /// Whether the selected root is a file or directory.
    pub root_kind: PackageItemKind,
    /// Total number of bytes transferred across all files.
    pub total_bytes: u64,
    /// Chunk size selected by the sender.
    pub chunk_size: u32,
    /// Total number of chunks expected across the package.
    pub chunk_count: u64,
    /// Number of files in the package.
    pub files_count: u64,
    /// Number of directories in the package.
    pub directories_count: u64,
    /// File and directory entries in relative-path order.
    pub entries: Vec<PackageEntry>,
}

impl TransferManifest {
    /// Encodes the transfer manifest into a binary representation.
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        self.validate()?;

        let payload = serde_json::to_vec(self).map_err(|error| ProtocolError::InvalidManifest(error.to_string()))?;
        let mut bytes = Vec::with_capacity(TRANSFER_MANIFEST_HEADER_LEN + payload.len());
        bytes.extend_from_slice(&TRANSFER_MANIFEST_MAGIC);
        bytes.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
        bytes.extend_from_slice(&payload);
        Ok(bytes)
    }

    /// Decodes a manifest from the full control-stream payload.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() < TRANSFER_MANIFEST_HEADER_LEN {
            return Err(ProtocolError::InvalidManifest("manifest payload was truncated".to_owned()));
        }
        if bytes[..4] != TRANSFER_MANIFEST_MAGIC {
            return Err(ProtocolError::InvalidMagic);
        }

        let mut version_bytes = [0_u8; 2];
        version_bytes.copy_from_slice(&bytes[4..6]);
        let version = u16::from_le_bytes(version_bytes);
        if version != PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedProtocolVersion(version));
        }

        let manifest: Self = serde_json::from_slice(&bytes[6..])
            .map_err(|error| ProtocolError::InvalidManifest(error.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Computes a stable SHA-256 fingerprint for the manifest contents.
    pub fn fingerprint(&self) -> Result<Sha256Hash, ProtocolError> {
        Ok(sha256_bytes(&self.encode()?))
    }

    /// Returns the package entries that represent regular files.
    pub fn file_entries(&self) -> impl Iterator<Item = (usize, &PackageEntry)> {
        self.entries.iter().enumerate().filter(|(_, entry)| entry.is_file())
    }

    /// Looks up an entry by its package index.
    pub fn entry(&self, entry_index: usize) -> Option<&PackageEntry> {
        self.entries.get(entry_index)
    }

    /// Returns the package entry that owns the given global chunk index.
    pub fn entry_for_chunk(&self, chunk_index: u64) -> Option<(usize, &PackageEntry)> {
        self.entries
            .iter()
            .enumerate()
            .find(|(_, entry)| entry.contains_chunk(chunk_index))
    }

    fn validate(&self) -> Result<(), ProtocolError> {
        if self.root_name.trim().is_empty() {
            return Err(ProtocolError::EmptyFileName);
        }
        if self.chunk_size == 0 {
            return Err(ProtocolError::InvalidChunkSize(0));
        }

        let mut computed_total_bytes = 0_u64;
        let mut computed_chunk_count = 0_u64;
        let mut computed_files = 0_u64;
        let mut computed_directories = u64::from(self.root_kind.is_directory());
        let mut expected_chunk_index = 0_u64;

        for entry in &self.entries {
            if entry.relative_path.trim().is_empty() {
                return Err(ProtocolError::EmptyRelativePath);
            }

            if entry.is_directory() {
                computed_directories = computed_directories.saturating_add(1);
                if entry.file_size != 0 || entry.chunk_count != 0 || entry.first_chunk_index != expected_chunk_index {
                    return Err(ProtocolError::InvalidManifest(format!(
                        "directory entry {} contained file payload metadata",
                        entry.relative_path
                    )));
                }
                continue;
            }

            computed_files = computed_files.saturating_add(1);
            computed_total_bytes = computed_total_bytes.saturating_add(entry.file_size);
            let expected_file_chunks = if entry.file_size == 0 {
                0
            } else {
                entry.file_size.div_ceil(u64::from(self.chunk_size))
            };
            if entry.chunk_count != expected_file_chunks {
                return Err(ProtocolError::InvalidManifest(format!(
                    "file entry {} had chunk_count {} but expected {}",
                    entry.relative_path, entry.chunk_count, expected_file_chunks
                )));
            }
            if entry.first_chunk_index != expected_chunk_index {
                return Err(ProtocolError::InvalidManifest(format!(
                    "file entry {} started at chunk {} but expected {}",
                    entry.relative_path, entry.first_chunk_index, expected_chunk_index
                )));
            }
            expected_chunk_index = expected_chunk_index.saturating_add(entry.chunk_count);
            computed_chunk_count = computed_chunk_count.saturating_add(entry.chunk_count);
        }

        if computed_total_bytes != self.total_bytes {
            return Err(ProtocolError::InvalidManifest(format!(
                "manifest total_bytes {} did not match computed {}",
                self.total_bytes, computed_total_bytes
            )));
        }
        if computed_chunk_count != self.chunk_count {
            return Err(ProtocolError::InvalidManifest(format!(
                "manifest chunk_count {} did not match computed {}",
                self.chunk_count, computed_chunk_count
            )));
        }
        if computed_files != self.files_count {
            return Err(ProtocolError::InvalidManifest(format!(
                "manifest files_count {} did not match computed {}",
                self.files_count, computed_files
            )));
        }
        if computed_directories != self.directories_count {
            return Err(ProtocolError::InvalidManifest(format!(
                "manifest directories_count {} did not match computed {}",
                self.directories_count, computed_directories
            )));
        }

        Ok(())
    }
}

/// Control response telling the sender which chunk indices are still missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumePlan {
    /// Chunk indices that must still be sent.
    pub missing_chunks: Vec<u64>,
}

impl ResumePlan {
    /// Encodes the resume plan into a compact binary representation.
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(RESUME_PLAN_PREAMBLE_LEN + (self.missing_chunks.len() * 8));
        bytes.extend_from_slice(&RESUME_PLAN_MAGIC);
        bytes.extend_from_slice(&(self.missing_chunks.len() as u64).to_le_bytes());
        for chunk_index in &self.missing_chunks {
            bytes.extend_from_slice(&chunk_index.to_le_bytes());
        }
        bytes
    }

    /// Decodes the fixed-length prefix of a resume plan.
    pub fn decode_preamble(
        preamble: &[u8; RESUME_PLAN_PREAMBLE_LEN],
    ) -> Result<u64, ProtocolError> {
        if &preamble[..4] != RESUME_PLAN_MAGIC.as_slice() {
            return Err(ProtocolError::InvalidMagic);
        }

        let mut count_bytes = [0_u8; 8];
        count_bytes.copy_from_slice(&preamble[4..12]);
        Ok(u64::from_le_bytes(count_bytes))
    }

    /// Builds a resume plan from decoded chunk indices.
    pub fn from_missing_chunks(missing_chunks: Vec<u64>) -> Self {
        Self { missing_chunks }
    }
}

/// Control frame acknowledging that a chunk has been verified and persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkAck {
    /// Completed global chunk index.
    pub chunk_index: u64,
}

impl ChunkAck {
    /// Encodes the acknowledgement frame.
    pub fn encode(&self) -> [u8; CHUNK_ACK_FRAME_LEN] {
        let mut bytes = [0_u8; CHUNK_ACK_FRAME_LEN];
        bytes[..4].copy_from_slice(&CHUNK_ACK_MAGIC);
        bytes[4..12].copy_from_slice(&self.chunk_index.to_le_bytes());
        bytes
    }

    /// Decodes an acknowledgement frame.
    pub fn decode(frame: &[u8; CHUNK_ACK_FRAME_LEN]) -> Result<Self, ProtocolError> {
        if &frame[..4] != CHUNK_ACK_MAGIC.as_slice() {
            return Err(ProtocolError::InvalidMagic);
        }

        let mut chunk_index = [0_u8; 8];
        chunk_index.copy_from_slice(&frame[4..12]);
        Ok(Self {
            chunk_index: u64::from_le_bytes(chunk_index),
        })
    }
}

/// Final status message sent once the receiver has verified the completed transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferStatus {
    /// Whether the receiver completed and verified the transfer successfully.
    pub complete: bool,
}

impl TransferStatus {
    /// Encodes the transfer-status frame.
    pub fn encode(&self) -> [u8; TRANSFER_STATUS_FRAME_LEN] {
        let mut bytes = [0_u8; TRANSFER_STATUS_FRAME_LEN];
        bytes[..4].copy_from_slice(&TRANSFER_STATUS_MAGIC);
        bytes[4] = u8::from(self.complete);
        bytes
    }

    /// Decodes a transfer-status frame.
    pub fn decode(frame: &[u8; TRANSFER_STATUS_FRAME_LEN]) -> Result<Self, ProtocolError> {
        if &frame[..4] != TRANSFER_STATUS_MAGIC.as_slice() {
            return Err(ProtocolError::InvalidMagic);
        }

        match frame[4] {
            0 => Ok(Self { complete: false }),
            1 => Ok(Self { complete: true }),
            other => Err(ProtocolError::InvalidStatusCode(other)),
        }
    }
}

/// Error returned when decoding or encoding protocol frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// The incoming frame did not contain the expected magic bytes.
    InvalidMagic,
    /// The encoded file name exceeds the protocol limit.
    FileNameTooLong(usize),
    /// The file name bytes were not valid UTF-8.
    InvalidFileName,
    /// Transfers require a non-empty package root name.
    EmptyFileName,
    /// Relative entry paths must be non-empty.
    EmptyRelativePath,
    /// Chunk sizes must be positive.
    InvalidChunkSize(u32),
    /// Control frames must use a known status byte.
    InvalidStatusCode(u8),
    /// The manifest payload failed semantic validation.
    InvalidManifest(String),
    /// The manifest version is not supported by this build.
    UnsupportedProtocolVersion(u16),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMagic => write!(f, "invalid protocol magic"),
            Self::FileNameTooLong(length) => write!(f, "file name too long for protocol header: {length} bytes"),
            Self::InvalidFileName => write!(f, "file name was not valid UTF-8"),
            Self::EmptyFileName => write!(f, "file name cannot be empty"),
            Self::EmptyRelativePath => write!(f, "relative path cannot be empty"),
            Self::InvalidChunkSize(size) => write!(f, "invalid chunk size: {size}"),
            Self::InvalidStatusCode(code) => write!(f, "invalid transfer status code: {code}"),
            Self::InvalidManifest(message) => write!(f, "invalid transfer manifest: {message}"),
            Self::UnsupportedProtocolVersion(version) => write!(f, "unsupported protocol version: {version}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

#[cfg(test)]
mod tests {
    use integrity::sha256_bytes;

    use super::{
        ChunkAck, ChunkDescriptor, ChunkStreamHeader, PackageEntry, PackageItemKind, ResumePlan,
        TransferManifest, TransferStatus, CHUNK_ACK_FRAME_LEN, CHUNK_STREAM_PREAMBLE_LEN,
        RESUME_PLAN_PREAMBLE_LEN, TRANSFER_MANIFEST_HEADER_LEN, TRANSFER_STATUS_FRAME_LEN,
    };

    #[test]
    fn transfer_manifest_round_trips() {
        let manifest = TransferManifest {
            root_name: "photos".to_owned(),
            root_kind: PackageItemKind::Directory,
            total_bytes: 123_456,
            chunk_size: 4_096,
            chunk_count: 31,
            files_count: 2,
            directories_count: 2,
            entries: vec![
                PackageEntry {
                    relative_path: "empty".to_owned(),
                    item_kind: PackageItemKind::Directory,
                    file_size: 0,
                    file_sha256: [0_u8; 32],
                    first_chunk_index: 0,
                    chunk_count: 0,
                },
                PackageEntry {
                    relative_path: "nested/cat.jpg".to_owned(),
                    item_kind: PackageItemKind::File,
                    file_size: 120_000,
                    file_sha256: sha256_bytes(b"cat"),
                    first_chunk_index: 0,
                    chunk_count: 30,
                },
                PackageEntry {
                    relative_path: "nested/dog.jpg".to_owned(),
                    item_kind: PackageItemKind::File,
                    file_size: 3_456,
                    file_sha256: sha256_bytes(b"dog"),
                    first_chunk_index: 30,
                    chunk_count: 1,
                },
            ],
        };

        let encoded = manifest.encode().expect("encoding should succeed");
        assert_eq!(&encoded[..4], b"FTM1");
        assert_eq!(encoded.len(), TRANSFER_MANIFEST_HEADER_LEN + encoded[TRANSFER_MANIFEST_HEADER_LEN..].len());
        let decoded = TransferManifest::decode(&encoded).expect("manifest should decode");
        assert_eq!(decoded, manifest);
        assert!(decoded.entry_for_chunk(30).is_some());
    }

    #[test]
    fn chunk_header_round_trips() {
        let header = ChunkStreamHeader {
            file_index: 7,
            descriptor: ChunkDescriptor {
                index: 3,
                offset: 12_288,
                size: 4_096,
            },
            chunk_sha256: sha256_bytes(b"chunk bytes"),
        };

        let encoded = header.encode();
        let mut preamble = [0_u8; CHUNK_STREAM_PREAMBLE_LEN];
        preamble.copy_from_slice(&encoded[..CHUNK_STREAM_PREAMBLE_LEN]);
        let decoded = ChunkStreamHeader::decode(&preamble).expect("header should decode");

        assert_eq!(decoded, header);
    }

    #[test]
    fn resume_plan_round_trips() {
        let plan = ResumePlan::from_missing_chunks(vec![0, 2, 5]);
        let encoded = plan.encode();
        let mut preamble = [0_u8; RESUME_PLAN_PREAMBLE_LEN];
        preamble.copy_from_slice(&encoded[..RESUME_PLAN_PREAMBLE_LEN]);
        let count = ResumePlan::decode_preamble(&preamble).expect("resume plan should decode");
        assert_eq!(count, 3);
        let mut indices = Vec::new();
        for chunk in encoded[RESUME_PLAN_PREAMBLE_LEN..].chunks_exact(8) {
            let mut bytes = [0_u8; 8];
            bytes.copy_from_slice(chunk);
            indices.push(u64::from_le_bytes(bytes));
        }
        assert_eq!(indices, vec![0, 2, 5]);
    }

    #[test]
    fn control_frames_round_trip() {
        let ack = ChunkAck { chunk_index: 9 };
        let ack_bytes = ack.encode();
        let mut ack_frame = [0_u8; CHUNK_ACK_FRAME_LEN];
        ack_frame.copy_from_slice(&ack_bytes);
        assert_eq!(ChunkAck::decode(&ack_frame).expect("ack should decode"), ack);

        let status = TransferStatus { complete: true };
        let status_bytes = status.encode();
        let mut status_frame = [0_u8; TRANSFER_STATUS_FRAME_LEN];
        status_frame.copy_from_slice(&status_bytes);
        assert_eq!(TransferStatus::decode(&status_frame).expect("status should decode"), status);
    }
}

