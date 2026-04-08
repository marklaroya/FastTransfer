use integrity::Sha256Hash;
use serde::{Deserialize, Serialize};

use crate::{PackageItemKind, ProtocolError};

pub const STREAM_CONTROL_MAGIC: [u8; 4] = *b"FTQ1";
pub const STREAM_CONTROL_HEADER_LEN: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum SenderControlMessage {
    SessionStart(StreamSessionStart),
    DirectoryEntry(StreamDirectoryEntry),
    FileEntry(StreamFileEntry),
    FileComplete(StreamFileComplete),
    TransferComplete(StreamTransferComplete),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ReceiverControlMessage {
    FileDisposition(StreamFileDisposition),
    FileResult(StreamFileResult),
    TransferStatus(StreamTransferStatus),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamSessionStart {
    pub root_name: String,
    pub root_kind: PackageItemKind,
    pub chunk_size: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamDirectoryEntry {
    pub relative_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamFileEntry {
    pub file_id: u32,
    pub relative_path: String,
    pub file_size: u64,
    pub file_sha256: Sha256Hash,
    pub chunk_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamFileComplete {
    pub file_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamTransferComplete {
    pub total_files: u64,
    pub total_directories: u64,
    pub total_bytes: u64,
    pub aggregate_sha256: Sha256Hash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamFileDisposition {
    pub file_id: u32,
    pub needed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamFileResult {
    pub file_id: u32,
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamTransferStatus {
    pub complete: bool,
}

impl SenderControlMessage {
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        encode_control_frame(self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        decode_control_frame(bytes)
    }
}

impl ReceiverControlMessage {
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        encode_control_frame(self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        decode_control_frame(bytes)
    }
}

fn encode_control_frame<T>(value: &T) -> Result<Vec<u8>, ProtocolError>
where
    T: Serialize,
{
    let payload = serde_json::to_vec(value)
        .map_err(|error| ProtocolError::InvalidManifest(error.to_string()))?;
    let payload_len = u32::try_from(payload.len())
        .map_err(|_| ProtocolError::InvalidManifest("control frame exceeded protocol size limit".to_owned()))?;

    let mut bytes = Vec::with_capacity(STREAM_CONTROL_HEADER_LEN + payload.len());
    bytes.extend_from_slice(&STREAM_CONTROL_MAGIC);
    bytes.extend_from_slice(&payload_len.to_le_bytes());
    bytes.extend_from_slice(&payload);
    Ok(bytes)
}

fn decode_control_frame<T>(bytes: &[u8]) -> Result<T, ProtocolError>
where
    T: for<'de> Deserialize<'de>,
{
    if bytes.len() < STREAM_CONTROL_HEADER_LEN {
        return Err(ProtocolError::InvalidManifest("control frame was truncated".to_owned()));
    }
    if bytes[..4] != STREAM_CONTROL_MAGIC {
        return Err(ProtocolError::InvalidMagic);
    }

    let mut length_bytes = [0_u8; 4];
    length_bytes.copy_from_slice(&bytes[4..8]);
    let payload_len = u32::from_le_bytes(length_bytes) as usize;
    if bytes.len() != STREAM_CONTROL_HEADER_LEN + payload_len {
        return Err(ProtocolError::InvalidManifest("control frame length mismatch".to_owned()));
    }

    serde_json::from_slice(&bytes[8..])
        .map_err(|error| ProtocolError::InvalidManifest(error.to_string()))
}

#[cfg(test)]
mod tests {
    use integrity::sha256_bytes;

    use super::{
        ReceiverControlMessage, SenderControlMessage, StreamDirectoryEntry, StreamFileComplete,
        StreamFileDisposition, StreamFileEntry, StreamFileResult, StreamSessionStart,
        StreamTransferComplete, StreamTransferStatus,
    };
    use crate::PackageItemKind;

    #[test]
    fn sender_control_message_round_trips() {
        let session = SenderControlMessage::SessionStart(StreamSessionStart {
            root_name: "photos".to_owned(),
            root_kind: PackageItemKind::Directory,
            chunk_size: 1024,
        });
        let encoded = session.encode().expect("session should encode");
        let decoded = SenderControlMessage::decode(&encoded).expect("session should decode");
        assert_eq!(decoded, session);

        let file = SenderControlMessage::FileEntry(StreamFileEntry {
            file_id: 7,
            relative_path: "nested/cat.jpg".to_owned(),
            file_size: 4096,
            file_sha256: sha256_bytes(b"cat"),
            chunk_count: 4,
        });
        let encoded = file.encode().expect("file should encode");
        let decoded = SenderControlMessage::decode(&encoded).expect("file should decode");
        assert_eq!(decoded, file);

        let directory = SenderControlMessage::DirectoryEntry(StreamDirectoryEntry {
            relative_path: "empty".to_owned(),
        });
        let encoded = directory.encode().expect("directory should encode");
        let decoded = SenderControlMessage::decode(&encoded).expect("directory should decode");
        assert_eq!(decoded, directory);

        let complete = SenderControlMessage::FileComplete(StreamFileComplete { file_id: 7 });
        let encoded = complete.encode().expect("complete should encode");
        let decoded = SenderControlMessage::decode(&encoded).expect("complete should decode");
        assert_eq!(decoded, complete);

        let transfer_complete = SenderControlMessage::TransferComplete(StreamTransferComplete {
            total_files: 2,
            total_directories: 3,
            total_bytes: 8192,
            aggregate_sha256: sha256_bytes(b"summary"),
        });
        let encoded = transfer_complete.encode().expect("transfer complete should encode");
        let decoded = SenderControlMessage::decode(&encoded).expect("transfer complete should decode");
        assert_eq!(decoded, transfer_complete);
    }

    #[test]
    fn receiver_control_message_round_trips() {
        let disposition = ReceiverControlMessage::FileDisposition(StreamFileDisposition {
            file_id: 9,
            needed: true,
        });
        let encoded = disposition.encode().expect("disposition should encode");
        let decoded = ReceiverControlMessage::decode(&encoded).expect("disposition should decode");
        assert_eq!(decoded, disposition);

        let result = ReceiverControlMessage::FileResult(StreamFileResult {
            file_id: 9,
            success: true,
            message: "verified".to_owned(),
        });
        let encoded = result.encode().expect("result should encode");
        let decoded = ReceiverControlMessage::decode(&encoded).expect("result should decode");
        assert_eq!(decoded, result);

        let status = ReceiverControlMessage::TransferStatus(StreamTransferStatus { complete: true });
        let encoded = status.encode().expect("status should encode");
        let decoded = ReceiverControlMessage::decode(&encoded).expect("status should decode");
        assert_eq!(decoded, status);
    }
}
