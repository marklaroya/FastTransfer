
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Error, ErrorKind},
    path::{Path, PathBuf},
};

use integrity::Sha256Hash;
use protocol::PackageItemKind;
use serde::{Deserialize, Serialize};

const STREAMING_CHECKPOINT_VERSION: &str = "1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamingResumeMetadata {
    pub root_name: String,
    pub root_kind: PackageItemKind,
    pub chunk_size: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletedFileRecord {
    pub relative_path: String,
    pub file_size: u64,
    pub file_sha256: Sha256Hash,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StreamingCheckpointFile {
    version: String,
    metadata: StreamingResumeMetadata,
    completed_files: Vec<CompletedFileRecord>,
}

#[derive(Debug, Clone)]
pub struct PersistentFileResumeState {
    pub metadata: StreamingResumeMetadata,
    completed_files: BTreeMap<String, CompletedFileRecord>,
    path: PathBuf,
    existed: bool,
}

impl PersistentFileResumeState {
    pub fn load_or_create_in(
        base_dir: &Path,
        root_name: &str,
        root_kind: PackageItemKind,
        chunk_size: u32,
    ) -> io::Result<Self> {
        let metadata = StreamingResumeMetadata {
            root_name: root_name.to_owned(),
            root_kind,
            chunk_size,
        };
        let checkpoint_dir = checkpoint_dir(base_dir);
        fs::create_dir_all(&checkpoint_dir)?;
        let path = checkpoint_dir.join(checkpoint_file_name(root_name, root_kind));

        if path.exists() {
            let mut loaded = Self::load_from_path(&path)?;
            if loaded.metadata != metadata {
                return Err(invalid_data(format!(
                    "streaming resume checkpoint metadata mismatch for {}",
                    path.display()
                )));
            }
            loaded.path = path;
            loaded.existed = true;
            return Ok(loaded);
        }

        let state = Self {
            metadata,
            completed_files: BTreeMap::new(),
            path,
            existed: false,
        };
        state.persist()?;
        Ok(state)
    }

    pub fn existed(&self) -> bool {
        self.existed
    }

    pub fn has_progress(&self) -> bool {
        !self.completed_files.is_empty()
    }

    pub fn completed_files(&self) -> &BTreeMap<String, CompletedFileRecord> {
        &self.completed_files
    }

    pub fn is_file_complete(
        &self,
        relative_path: &str,
        file_size: u64,
        file_sha256: &Sha256Hash,
    ) -> bool {
        self.completed_files
            .get(relative_path)
            .map(|record| record.file_size == file_size && record.file_sha256 == *file_sha256)
            .unwrap_or(false)
    }

    pub fn mark_file_complete(
        &mut self,
        relative_path: String,
        file_size: u64,
        file_sha256: Sha256Hash,
    ) -> io::Result<bool> {
        let record = CompletedFileRecord {
            relative_path: relative_path.clone(),
            file_size,
            file_sha256,
        };

        if self.completed_files.get(&relative_path) == Some(&record) {
            return Ok(false);
        }

        self.completed_files.insert(relative_path, record);
        self.persist()?;
        Ok(true)
    }

    pub fn remove(self) -> io::Result<()> {
        if self.path.exists() {
            fs::remove_file(&self.path)?;
        }
        Ok(())
    }

    fn load_from_path(path: &Path) -> io::Result<Self> {
        let content = fs::read(path)?;
        let checkpoint: StreamingCheckpointFile = serde_json::from_slice(&content)
            .map_err(|error| invalid_data(format!("failed to decode streaming checkpoint: {error}")))?;

        if checkpoint.version != STREAMING_CHECKPOINT_VERSION {
            return Err(invalid_data("unsupported streaming checkpoint version"));
        }

        Ok(Self {
            metadata: checkpoint.metadata,
            completed_files: checkpoint
                .completed_files
                .into_iter()
                .map(|record| (record.relative_path.clone(), record))
                .collect(),
            path: path.to_path_buf(),
            existed: true,
        })
    }

    fn persist(&self) -> io::Result<()> {
        let checkpoint = StreamingCheckpointFile {
            version: STREAMING_CHECKPOINT_VERSION.to_owned(),
            metadata: self.metadata.clone(),
            completed_files: self.completed_files.values().cloned().collect(),
        };
        let content = serde_json::to_vec_pretty(&checkpoint)
            .map_err(|error| invalid_data(format!("failed to encode streaming checkpoint: {error}")))?;
        fs::write(&self.path, content)
    }
}

fn checkpoint_dir(base_dir: &Path) -> PathBuf {
    base_dir.join(".fasttransfer").join("resume-streaming")
}

fn checkpoint_file_name(root_name: &str, root_kind: PackageItemKind) -> String {
    let suffix = match root_kind {
        PackageItemKind::File => "file",
        PackageItemKind::Directory => "directory",
    };
    format!("{}-{suffix}.ftstreamresume", sanitize_id(root_name))
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

fn invalid_data(message: impl Into<String>) -> io::Error {
    Error::new(ErrorKind::InvalidData, message.into())
}

