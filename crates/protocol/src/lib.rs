//! Shared protocol primitives used across the FastTransfer engine.

use std::fmt;

use integrity::{Sha256Hash, SHA256_LEN};

/// Current protocol version for wire compatibility checks.
pub const PROTOCOL_VERSION: u16 = 1;

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

/// Size of the fixed-length transfer-manifest preamble.
pub const TRANSFER_MANIFEST_PREAMBLE_LEN: usize = 58;

/// Size of the fixed-length chunk-stream preamble.
pub const CHUNK_STREAM_PREAMBLE_LEN: usize = 56;

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
    /// Human-readable file name.
    pub file_name: String,
    /// Total file size in bytes.
    pub total_bytes: u64,
    /// Planned chunk size in bytes.
    pub chunk_size: u32,
    /// Session transfer direction.
    pub mode: TransferMode,
}

impl TransferSession {
    /// Returns the number of chunks needed to transfer the file.
    pub fn chunk_count(&self) -> u64 {
        if self.total_bytes == 0 {
            return 0;
        }

        let chunk_size = u64::from(self.chunk_size.max(1));
        self.total_bytes.div_ceil(chunk_size)
    }
}

/// Metadata describing a discrete file chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkDescriptor {
    /// Zero-based chunk index.
    pub index: u64,
    /// Byte offset within the file.
    pub offset: u64,
    /// Chunk payload size in bytes.
    pub size: u32,
}

/// Header sent before a chunk payload begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkStreamHeader {
    /// Planned chunk location within the file.
    pub descriptor: ChunkDescriptor,
    /// Expected SHA-256 of the chunk payload.
    pub chunk_sha256: Sha256Hash,
}

impl ChunkStreamHeader {
    /// Encodes a chunk-stream header.
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(CHUNK_STREAM_PREAMBLE_LEN);
        bytes.extend_from_slice(&CHUNK_STREAM_MAGIC);
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

        let mut index_bytes = [0_u8; 8];
        index_bytes.copy_from_slice(&preamble[4..12]);

        let mut offset_bytes = [0_u8; 8];
        offset_bytes.copy_from_slice(&preamble[12..20]);

        let mut size_bytes = [0_u8; 4];
        size_bytes.copy_from_slice(&preamble[20..24]);

        let size = u32::from_le_bytes(size_bytes);
        if size == 0 {
            return Err(ProtocolError::InvalidChunkSize(0));
        }

        let mut chunk_sha256 = [0_u8; SHA256_LEN];
        chunk_sha256.copy_from_slice(&preamble[24..56]);

        Ok(Self {
            descriptor: ChunkDescriptor {
                index: u64::from_le_bytes(index_bytes),
                offset: u64::from_le_bytes(offset_bytes),
                size,
            },
            chunk_sha256,
        })
    }
}

/// Metadata sent once before chunk streams begin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferManifest {
    /// File name that the receiver should persist.
    pub file_name: String,
    /// Total number of bytes that will be streamed.
    pub file_size: u64,
    /// Chunk size selected by the sender.
    pub chunk_size: u32,
    /// Total number of chunks expected for the transfer.
    pub chunk_count: u64,
    /// Expected SHA-256 of the full source file.
    pub file_sha256: Sha256Hash,
}

impl TransferManifest {
    /// Encodes the transfer manifest into a compact binary representation.
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        let name_bytes = self.file_name.as_bytes();
        let name_len = u16::try_from(name_bytes.len())
            .map_err(|_| ProtocolError::FileNameTooLong(name_bytes.len()))?;

        if name_len == 0 {
            return Err(ProtocolError::EmptyFileName);
        }
        if self.chunk_size == 0 {
            return Err(ProtocolError::InvalidChunkSize(0));
        }

        let mut bytes = Vec::with_capacity(TRANSFER_MANIFEST_PREAMBLE_LEN + usize::from(name_len));
        bytes.extend_from_slice(&TRANSFER_MANIFEST_MAGIC);
        bytes.extend_from_slice(&self.file_size.to_le_bytes());
        bytes.extend_from_slice(&self.chunk_size.to_le_bytes());
        bytes.extend_from_slice(&self.chunk_count.to_le_bytes());
        bytes.extend_from_slice(&self.file_sha256);
        bytes.extend_from_slice(&name_len.to_le_bytes());
        bytes.extend_from_slice(name_bytes);
        Ok(bytes)
    }

    /// Parses the fixed-length prefix of a transfer manifest.
    pub fn decode_preamble(
        preamble: &[u8; TRANSFER_MANIFEST_PREAMBLE_LEN],
    ) -> Result<(u64, u32, u64, Sha256Hash, usize), ProtocolError> {
        if &preamble[..4] != TRANSFER_MANIFEST_MAGIC.as_slice() {
            return Err(ProtocolError::InvalidMagic);
        }

        let mut file_size_bytes = [0_u8; 8];
        file_size_bytes.copy_from_slice(&preamble[4..12]);

        let mut chunk_size_bytes = [0_u8; 4];
        chunk_size_bytes.copy_from_slice(&preamble[12..16]);

        let mut chunk_count_bytes = [0_u8; 8];
        chunk_count_bytes.copy_from_slice(&preamble[16..24]);

        let mut file_sha256 = [0_u8; SHA256_LEN];
        file_sha256.copy_from_slice(&preamble[24..56]);

        let mut name_len_bytes = [0_u8; 2];
        name_len_bytes.copy_from_slice(&preamble[56..58]);

        let chunk_size = u32::from_le_bytes(chunk_size_bytes);
        if chunk_size == 0 {
            return Err(ProtocolError::InvalidChunkSize(0));
        }

        let name_len = usize::from(u16::from_le_bytes(name_len_bytes));
        if name_len == 0 {
            return Err(ProtocolError::EmptyFileName);
        }

        Ok((
            u64::from_le_bytes(file_size_bytes),
            chunk_size,
            u64::from_le_bytes(chunk_count_bytes),
            file_sha256,
            name_len,
        ))
    }

    /// Constructs a transfer manifest from decoded wire parts.
    pub fn from_parts(
        file_size: u64,
        chunk_size: u32,
        chunk_count: u64,
        file_sha256: Sha256Hash,
        file_name_bytes: Vec<u8>,
    ) -> Result<Self, ProtocolError> {
        let file_name = String::from_utf8(file_name_bytes).map_err(|_| ProtocolError::InvalidFileName)?;
        if file_name.is_empty() {
            return Err(ProtocolError::EmptyFileName);
        }
        if chunk_size == 0 {
            return Err(ProtocolError::InvalidChunkSize(0));
        }

        Ok(Self {
            file_name,
            file_size,
            chunk_size,
            chunk_count,
            file_sha256,
        })
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
    /// Completed chunk index.
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
    /// File transfers require a non-empty file name.
    EmptyFileName,
    /// Chunk sizes must be positive.
    InvalidChunkSize(u32),
    /// Control frames must use a known status byte.
    InvalidStatusCode(u8),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMagic => write!(f, "invalid protocol magic"),
            Self::FileNameTooLong(length) => write!(f, "file name too long for protocol header: {length} bytes"),
            Self::InvalidFileName => write!(f, "file name was not valid UTF-8"),
            Self::EmptyFileName => write!(f, "file name cannot be empty"),
            Self::InvalidChunkSize(size) => write!(f, "invalid chunk size: {size}"),
            Self::InvalidStatusCode(code) => write!(f, "invalid transfer status code: {code}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

#[cfg(test)]
mod tests {
    use integrity::sha256_bytes;

    use super::{
        ChunkAck, ChunkDescriptor, ChunkStreamHeader, ResumePlan, TransferManifest, TransferStatus,
        CHUNK_ACK_FRAME_LEN, CHUNK_STREAM_PREAMBLE_LEN, RESUME_PLAN_PREAMBLE_LEN,
        TRANSFER_MANIFEST_PREAMBLE_LEN, TRANSFER_STATUS_FRAME_LEN,
    };

    #[test]
    fn transfer_manifest_round_trips() {
        let manifest = TransferManifest {
            file_name: "archive.tar".to_owned(),
            file_size: 123_456,
            chunk_size: 4_096,
            chunk_count: 31,
            file_sha256: sha256_bytes(b"archive contents"),
        };

        let encoded = manifest.encode().expect("encoding should succeed");
        let mut preamble = [0_u8; TRANSFER_MANIFEST_PREAMBLE_LEN];
        preamble.copy_from_slice(&encoded[..TRANSFER_MANIFEST_PREAMBLE_LEN]);
        let (file_size, chunk_size, chunk_count, file_sha256, name_len) =
            TransferManifest::decode_preamble(&preamble).expect("preamble should decode");
        let decoded = TransferManifest::from_parts(
            file_size,
            chunk_size,
            chunk_count,
            file_sha256,
            encoded[TRANSFER_MANIFEST_PREAMBLE_LEN..][..name_len].to_vec(),
        )
        .expect("name should decode");

        assert_eq!(decoded, manifest);
    }

    #[test]
    fn chunk_header_round_trips() {
        let header = ChunkStreamHeader {
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
