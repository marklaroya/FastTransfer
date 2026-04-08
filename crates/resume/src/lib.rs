//! Resume bookkeeping and persistent checkpoints for interrupted transfers.

use std::{
    collections::BTreeSet,
    fs,
    io::{self, Error, ErrorKind},
    path::{Path, PathBuf},
};

use integrity::{format_sha256, Sha256Hash, SHA256_LEN};
use protocol::{PackageItemKind, TransferManifest, TransferSession};

const CHECKPOINT_VERSION: &str = "2";

/// Tracks completed chunks for a resumable transfer session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeState {
    /// Transfer metadata tied to this resume record.
    pub session: TransferSession,
    completed_chunks: BTreeSet<u64>,
}

impl ResumeState {
    /// Initializes empty resume state for a session.
    pub fn new(session: TransferSession) -> Self {
        Self {
            session,
            completed_chunks: BTreeSet::new(),
        }
    }

    /// Marks a chunk as completed.
    pub fn mark_complete(&mut self, chunk_index: u64) {
        self.completed_chunks.insert(chunk_index);
    }

    /// Returns `true` when a chunk was already completed.
    pub fn is_complete(&self, chunk_index: u64) -> bool {
        self.completed_chunks.contains(&chunk_index)
    }

    /// Returns the set of pending chunk indices.
    pub fn pending_chunks(&self) -> Vec<u64> {
        (0..self.session.chunk_count())
            .filter(|index| !self.completed_chunks.contains(index))
            .collect()
    }
}

/// Metadata that must stay stable across resumed transfers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeMetadata {
    /// Package root name for the transfer.
    pub root_name: String,
    /// Whether the selected root was a file or directory.
    pub root_kind: PackageItemKind,
    /// Total package size in bytes.
    pub total_bytes: u64,
    /// Configured chunk size in bytes.
    pub chunk_size: u32,
    /// Total number of chunks in the transfer.
    pub chunk_count: u64,
    /// Total number of files in the package.
    pub files_count: u64,
    /// Total number of directories in the package.
    pub directories_count: u64,
    /// Stable fingerprint of the manifest contents.
    pub manifest_sha256: Sha256Hash,
}

impl ResumeMetadata {
    /// Builds stable resume metadata from a transfer manifest.
    pub fn from_manifest(manifest: &TransferManifest) -> io::Result<Self> {
        let manifest_sha256 = manifest
            .fingerprint()
            .map_err(|error| invalid_data(format!("failed to fingerprint manifest: {error}")))?;
        Ok(Self {
            root_name: manifest.root_name.clone(),
            root_kind: manifest.root_kind,
            total_bytes: manifest.total_bytes,
            chunk_size: manifest.chunk_size,
            chunk_count: manifest.chunk_count,
            files_count: manifest.files_count,
            directories_count: manifest.directories_count,
            manifest_sha256,
        })
    }

    fn checkpoint_file_name(&self) -> String {
        let root_name_hex = encode_hex(self.root_name.as_bytes());
        format!("{root_name_hex}-{}.ftresume", format_sha256(&self.manifest_sha256))
    }
}

/// Persistent checkpoint state stored on disk for resumable transfers.
#[derive(Debug, Clone)]
pub struct PersistentResumeState {
    /// Stable transfer metadata used for compatibility checks.
    pub metadata: ResumeMetadata,
    completed_chunks: BTreeSet<u64>,
    path: PathBuf,
    existed: bool,
}

impl PersistentResumeState {
    /// Loads an existing checkpoint or creates an empty one under the given base directory.
    pub fn load_or_create_in(base_dir: &Path, manifest: &TransferManifest) -> io::Result<Self> {
        let metadata = ResumeMetadata::from_manifest(manifest)?;
        let checkpoint_dir = checkpoint_dir(base_dir);
        fs::create_dir_all(&checkpoint_dir)?;
        let path = checkpoint_dir.join(metadata.checkpoint_file_name());

        if path.exists() {
            let mut loaded = Self::load_from_path(&path)?;
            if loaded.metadata != metadata {
                return Err(invalid_data(format!(
                    "resume checkpoint metadata mismatch for {}",
                    path.display()
                )));
            }
            loaded.path = path;
            loaded.existed = true;
            return Ok(loaded);
        }

        let state = Self {
            metadata,
            completed_chunks: BTreeSet::new(),
            path,
            existed: false,
        };
        state.persist()?;
        Ok(state)
    }

    /// Returns whether this checkpoint already existed on disk.
    pub fn existed(&self) -> bool {
        self.existed
    }

    /// Returns `true` when any chunk has already been completed.
    pub fn has_progress(&self) -> bool {
        !self.completed_chunks.is_empty()
    }

    /// Returns `true` when a chunk is already checkpointed as complete.
    pub fn is_complete(&self, chunk_index: u64) -> bool {
        self.completed_chunks.contains(&chunk_index)
    }

    /// Returns the completed chunk indices.
    pub fn completed_chunks(&self) -> &BTreeSet<u64> {
        &self.completed_chunks
    }

    /// Returns the pending chunk indices for this checkpoint.
    pub fn pending_chunks(&self) -> Vec<u64> {
        (0..self.metadata.chunk_count)
            .filter(|index| !self.completed_chunks.contains(index))
            .collect()
    }

    /// Marks a chunk as complete and persists the checkpoint if it changed.
    pub fn mark_complete(&mut self, chunk_index: u64) -> io::Result<bool> {
        if chunk_index >= self.metadata.chunk_count {
            return Err(invalid_data(format!(
                "chunk index {} exceeds checkpoint chunk count {}",
                chunk_index,
                self.metadata.chunk_count
            )));
        }

        if !self.completed_chunks.insert(chunk_index) {
            return Ok(false);
        }

        self.persist()?;
        Ok(true)
    }

    /// Deletes the checkpoint file from disk.
    pub fn remove(self) -> io::Result<()> {
        if self.path.exists() {
            fs::remove_file(&self.path)?;
        }
        Ok(())
    }

    /// Returns the checkpoint file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn load_from_path(path: &Path) -> io::Result<Self> {
        let content = fs::read_to_string(path)?;
        let mut version = None;
        let mut root_name = None;
        let mut root_kind = None;
        let mut total_bytes = None;
        let mut chunk_size = None;
        let mut chunk_count = None;
        let mut files_count = None;
        let mut directories_count = None;
        let mut manifest_sha256 = None;
        let mut completed_chunks = BTreeSet::new();

        for line in content.lines() {
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| invalid_data(format!("invalid checkpoint line: {line}")))?;

            match key {
                "version" => version = Some(value.to_owned()),
                "root_name_hex" => {
                    let bytes = decode_hex(value)?;
                    let decoded = String::from_utf8(bytes)
                        .map_err(|_| invalid_data("checkpoint root name was not valid UTF-8"))?;
                    root_name = Some(decoded);
                }
                "root_kind" => {
                    root_kind = Some(match value {
                        "file" => PackageItemKind::File,
                        "directory" => PackageItemKind::Directory,
                        _ => return Err(invalid_data("invalid checkpoint root kind")),
                    })
                }
                "total_bytes" => total_bytes = Some(parse_u64(value, "total_bytes")?),
                "chunk_size" => chunk_size = Some(parse_u32(value, "chunk_size")?),
                "chunk_count" => chunk_count = Some(parse_u64(value, "chunk_count")?),
                "files_count" => files_count = Some(parse_u64(value, "files_count")?),
                "directories_count" => directories_count = Some(parse_u64(value, "directories_count")?),
                "manifest_sha256" => manifest_sha256 = Some(parse_sha256(value)?),
                "completed" => {
                    if !value.is_empty() {
                        for item in value.split(',') {
                            completed_chunks.insert(parse_u64(item, "completed chunk index")?);
                        }
                    }
                }
                _ => return Err(invalid_data(format!("unknown checkpoint field: {key}"))),
            }
        }

        if version.as_deref() != Some(CHECKPOINT_VERSION) {
            return Err(invalid_data("unsupported checkpoint version"));
        }

        Ok(Self {
            metadata: ResumeMetadata {
                root_name: root_name.ok_or_else(|| invalid_data("missing checkpoint root name"))?,
                root_kind: root_kind.ok_or_else(|| invalid_data("missing checkpoint root kind"))?,
                total_bytes: total_bytes.ok_or_else(|| invalid_data("missing checkpoint total bytes"))?,
                chunk_size: chunk_size.ok_or_else(|| invalid_data("missing checkpoint chunk size"))?,
                chunk_count: chunk_count.ok_or_else(|| invalid_data("missing checkpoint chunk count"))?,
                files_count: files_count.ok_or_else(|| invalid_data("missing checkpoint files count"))?,
                directories_count: directories_count.ok_or_else(|| invalid_data("missing checkpoint directories count"))?,
                manifest_sha256: manifest_sha256.ok_or_else(|| invalid_data("missing checkpoint manifest SHA-256"))?,
            },
            completed_chunks,
            path: path.to_path_buf(),
            existed: true,
        })
    }

    fn persist(&self) -> io::Result<()> {
        let completed = self
            .completed_chunks
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let root_kind = match self.metadata.root_kind {
            PackageItemKind::File => "file",
            PackageItemKind::Directory => "directory",
        };
        let content = format!(
            "version={version}\nroot_name_hex={root_name_hex}\nroot_kind={root_kind}\ntotal_bytes={total_bytes}\nchunk_size={chunk_size}\nchunk_count={chunk_count}\nfiles_count={files_count}\ndirectories_count={directories_count}\nmanifest_sha256={manifest_sha256}\ncompleted={completed}\n",
            version = CHECKPOINT_VERSION,
            root_name_hex = encode_hex(self.metadata.root_name.as_bytes()),
            root_kind = root_kind,
            total_bytes = self.metadata.total_bytes,
            chunk_size = self.metadata.chunk_size,
            chunk_count = self.metadata.chunk_count,
            files_count = self.metadata.files_count,
            directories_count = self.metadata.directories_count,
            manifest_sha256 = format_sha256(&self.metadata.manifest_sha256),
            completed = completed,
        );
        fs::write(&self.path, content)
    }
}

fn checkpoint_dir(base_dir: &Path) -> PathBuf {
    base_dir.join(".fasttransfer").join("resume")
}

fn parse_u64(value: &str, field: &str) -> io::Result<u64> {
    value
        .parse::<u64>()
        .map_err(|_| invalid_data(format!("invalid {field} value: {value}")))
}

fn parse_u32(value: &str, field: &str) -> io::Result<u32> {
    value
        .parse::<u32>()
        .map_err(|_| invalid_data(format!("invalid {field} value: {value}")))
}

fn parse_sha256(value: &str) -> io::Result<Sha256Hash> {
    let bytes = decode_hex(value)?;
    if bytes.len() != SHA256_LEN {
        return Err(invalid_data("invalid SHA-256 length in checkpoint"));
    }

    let mut hash = [0_u8; SHA256_LEN];
    hash.copy_from_slice(&bytes);
    Ok(hash)
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

fn decode_hex(value: &str) -> io::Result<Vec<u8>> {
    if value.len() % 2 != 0 {
        return Err(invalid_data("hex value had an odd number of characters"));
    }

    let mut bytes = Vec::with_capacity(value.len() / 2);
    let raw = value.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        let high = decode_hex_nibble(raw[index])?;
        let low = decode_hex_nibble(raw[index + 1])?;
        bytes.push((high << 4) | low);
        index += 2;
    }

    Ok(bytes)
}

fn decode_hex_nibble(byte: u8) -> io::Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(invalid_data("invalid hex digit in checkpoint")),
    }
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    Error::new(ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use integrity::sha256_bytes;
    use protocol::{PackageEntry, PackageItemKind, TransferManifest};

    use super::PersistentResumeState;

    #[test]
    fn checkpoint_round_trips_completed_chunks() {
        let temp_root = std::env::temp_dir().join(format!(
            "fasttransfer-resume-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be monotonic enough for tests")
                .as_nanos()
        ));
        fs::create_dir_all(&temp_root).expect("temp directory should be created");

        let manifest = TransferManifest {
            root_name: "example".to_owned(),
            root_kind: PackageItemKind::Directory,
            total_bytes: 12_345,
            chunk_size: 4_096,
            chunk_count: 4,
            files_count: 1,
            directories_count: 1,
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
                    relative_path: "file.bin".to_owned(),
                    item_kind: PackageItemKind::File,
                    file_size: 12_345,
                    file_sha256: sha256_bytes(b"example"),
                    first_chunk_index: 0,
                    chunk_count: 4,
                },
            ],
        };

        let mut state = PersistentResumeState::load_or_create_in(&temp_root, &manifest)
            .expect("checkpoint should be created");
        assert!(!state.existed());
        state.mark_complete(1).expect("chunk should persist");
        state.mark_complete(3).expect("chunk should persist");

        let loaded = PersistentResumeState::load_or_create_in(&temp_root, &manifest)
            .expect("checkpoint should be loaded");
        assert!(loaded.existed());
        assert!(loaded.is_complete(1));
        assert!(loaded.is_complete(3));
        assert_eq!(loaded.pending_chunks(), vec![0, 2]);

        fs::remove_dir_all(&temp_root).expect("temp directory should be cleaned up");
    }
}
