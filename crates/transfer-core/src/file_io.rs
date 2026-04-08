//! File-system helpers for chunked transfers.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{bail, Context, Result};
use integrity::{format_sha256, verify_sha256, Sha256State};
use protocol::{ChunkDescriptor, ChunkStreamHeader, TransferManifest};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom},
    sync::Mutex,
};

const BUFFER_SIZE: usize = 256 * 1024;

/// Reassembles incoming chunks into the final output file.
#[derive(Debug, Clone)]
pub struct ChunkedFileAssembler {
    destination_path: PathBuf,
    file: Arc<Mutex<File>>,
}

impl ChunkedFileAssembler {
    /// Prepares the destination file and preallocates the expected final length.
    pub async fn prepare(output_dir: &Path, manifest: &TransferManifest, resume_existing: bool) -> Result<Self> {
        fs::create_dir_all(output_dir)
            .await
            .with_context(|| format!("failed to create output directory {}", output_dir.display()))?;

        let destination_path = destination_path(output_dir, &manifest.file_name)?;
        let file = if resume_existing {
            if !destination_path.exists() {
                bail!(
                    "resume checkpoint exists but partial file is missing: {}",
                    destination_path.display()
                );
            }
            OpenOptions::new()
                .write(true)
                .read(true)
                .open(&destination_path)
                .await
                .with_context(|| format!("failed to open destination file {}", destination_path.display()))?
        } else {
            if destination_path.exists() {
                bail!(
                    "destination file already exists, refusing to overwrite: {}",
                    destination_path.display()
                );
            }
            OpenOptions::new()
                .create_new(true)
                .write(true)
                .read(true)
                .open(&destination_path)
                .await
                .with_context(|| format!("failed to create destination file {}", destination_path.display()))?
        };

        file.set_len(manifest.file_size)
            .await
            .with_context(|| format!("failed to size destination file {}", destination_path.display()))?;

        Ok(Self {
            destination_path,
            file: Arc::new(Mutex::new(file)),
        })
    }

    /// Streams an incoming chunk into its destination byte range and verifies the chunk hash.
    pub async fn write_chunk_stream<R>(&self, header: &ChunkStreamHeader, reader: &mut R) -> Result<()>
    where
        R: AsyncRead + Unpin,
    {
        let descriptor = &header.descriptor;
        let mut remaining = u64::from(descriptor.size);
        let mut current_offset = descriptor.offset;
        let mut buffer = vec![0_u8; BUFFER_SIZE.min(descriptor.size as usize).max(1)];
        let mut hasher = Sha256State::new();

        while remaining > 0 {
            let read_len = remaining.min(buffer.len() as u64) as usize;
            reader
                .read_exact(&mut buffer[..read_len])
                .await
                .with_context(|| format!("failed to read payload for chunk {}", descriptor.index))?;
            hasher.update(&buffer[..read_len]);

            let mut file = self.file.lock().await;
            file.seek(SeekFrom::Start(current_offset))
                .await
                .with_context(|| format!("failed to seek destination file for chunk {}", descriptor.index))?;
            file.write_all(&buffer[..read_len])
                .await
                .with_context(|| format!("failed to write destination bytes for chunk {}", descriptor.index))?;
            drop(file);

            current_offset += read_len as u64;
            remaining -= read_len as u64;
        }

        let mut trailing = [0_u8; 1];
        let trailing_bytes = reader
            .read(&mut trailing)
            .await
            .with_context(|| format!("failed while checking for trailing bytes in chunk {}", descriptor.index))?;
        if trailing_bytes != 0 {
            bail!("chunk {} contained trailing data beyond its declared size", descriptor.index);
        }

        let actual_hash = hasher.finalize();
        if !verify_sha256(&actual_hash, &header.chunk_sha256) {
            bail!(
                "chunk {} failed SHA-256 verification: expected {}, got {}",
                descriptor.index,
                format_sha256(&header.chunk_sha256),
                format_sha256(&actual_hash)
            );
        }

        Ok(())
    }

    /// Flushes the reassembled file to durable storage.
    pub async fn finalize(&self) -> Result<()> {
        let file = self.file.lock().await;
        file.sync_all()
            .await
            .with_context(|| format!("failed to flush destination file {}", self.destination_path.display()))
    }

    /// Returns the final destination path.
    pub fn destination_path(&self) -> &Path {
        &self.destination_path
    }
}

/// Reads a single chunk from disk without loading the entire file into memory.
pub async fn read_chunk(source_path: &Path, descriptor: &ChunkDescriptor) -> Result<Vec<u8>> {
    let mut file = File::open(source_path)
        .await
        .with_context(|| format!("failed to open source file {}", source_path.display()))?;
    file.seek(SeekFrom::Start(descriptor.offset))
        .await
        .with_context(|| format!("failed to seek source file for chunk {}", descriptor.index))?;

    let mut buffer = vec![0_u8; descriptor.size as usize];
    file.read_exact(&mut buffer)
        .await
        .with_context(|| format!("failed to read source chunk {}", descriptor.index))?;
    Ok(buffer)
}

/// Builds a transfer manifest from a local source path, selected chunk size, and expected file hash.
pub async fn build_manifest(
    source_path: &Path,
    chunk_size: u32,
    chunk_count: u64,
    file_sha256: integrity::Sha256Hash,
) -> Result<TransferManifest> {
    let metadata = fs::metadata(source_path)
        .await
        .with_context(|| format!("failed to read source metadata for {}", source_path.display()))?;

    if !metadata.is_file() {
        bail!("source path is not a regular file: {}", source_path.display());
    }

    let file_name = source_path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .with_context(|| format!("failed to derive a valid file name from {}", source_path.display()))?;

    Ok(TransferManifest {
        file_name,
        file_size: metadata.len(),
        chunk_size: chunk_size.max(1),
        chunk_count,
        file_sha256,
    })
}

/// Resolves the destination path for a received file.
pub fn destination_path(output_dir: &Path, file_name: &str) -> Result<PathBuf> {
    let safe_name = Path::new(file_name)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .context("received file name was not a safe path component")?;
    Ok(output_dir.join(safe_name))
}
