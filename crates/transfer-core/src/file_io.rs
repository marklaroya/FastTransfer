//! File-system helpers for chunked package transfers.

use std::{
    ffi::OsString,
    fs as stdfs,
    path::{Component, Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use chunker::FixedChunker;
use integrity::{format_sha256, sha256_file, verify_sha256, Sha256Hash, Sha256State};
use protocol::{ChunkDescriptor, ChunkStreamHeader, PackageEntry, PackageItemKind, TransferManifest};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom},
    task::spawn_blocking,
};

const BUFFER_SIZE: usize = 256 * 1024;

/// Summary of a file or directory selected for transfer.
#[derive(Debug, Clone)]
pub struct SourceInspection {
    /// Selected absolute source path.
    pub source_path: PathBuf,
    /// Human-readable root name.
    pub root_name: String,
    /// Whether the selected source is a file or directory.
    pub root_kind: PackageItemKind,
    /// Number of files that will be transferred.
    pub total_files: u64,
    /// Number of directories preserved in the package, including the root when it is a directory.
    pub total_directories: u64,
    /// Total number of payload bytes across all files.
    pub total_bytes: u64,
}

/// Prepared sender-side package with manifest and chunk plan.
#[derive(Debug, Clone)]
pub struct SourcePackage {
    /// Selected absolute source path.
    pub source_path: PathBuf,
    /// Source inspection summary.
    pub inspection: SourceInspection,
    /// Transfer manifest shared with the receiver.
    pub manifest: TransferManifest,
    /// Files in manifest order.
    pub files: Vec<SourceFile>,
    /// Global chunk tasks indexed by global chunk index.
    pub chunks: Vec<PackageChunkTask>,
}

impl SourcePackage {
    /// Returns the chunk task for the given global chunk index.
    pub fn chunk_task(&self, chunk_index: u64) -> Option<&PackageChunkTask> {
        self.chunks.get(chunk_index as usize)
    }
}

/// Sender-side metadata for a manifest file entry.
#[derive(Debug, Clone)]
pub struct SourceFile {
    /// Package entry index within the manifest.
    pub entry_index: usize,
    /// Absolute file path on disk.
    pub absolute_path: PathBuf,
    /// Relative path inside the package.
    pub relative_path: String,
    /// File size in bytes.
    pub file_size: u64,
    /// First global chunk index assigned to the file.
    pub first_chunk_index: u64,
    /// Number of chunks assigned to the file.
    pub chunk_count: u64,
    /// Expected SHA-256 of the whole file.
    pub file_sha256: Sha256Hash,
}

/// Sender-side metadata for a chunk task.
#[derive(Debug, Clone)]
pub struct PackageChunkTask {
    /// Package entry index within the manifest.
    pub file_index: u32,
    /// Relative file path inside the package.
    pub relative_path: String,
    /// Absolute source file path.
    pub source_path: PathBuf,
    /// Global chunk descriptor.
    pub descriptor: ChunkDescriptor,
}

/// Reassembles incoming package chunks into the final output files.
#[derive(Debug, Clone)]
pub struct PackageAssembler {
    output_dir: PathBuf,
    manifest: TransferManifest,
}

impl PackageAssembler {
    /// Prepares the destination package layout and creates empty directories and files.
    pub async fn prepare(output_dir: &Path, manifest: &TransferManifest, resume_existing: bool) -> Result<Self> {
        fs::create_dir_all(output_dir)
            .await
            .with_context(|| format!("failed to create output directory {}", output_dir.display()))?;

        let package_root = package_root_path(output_dir, manifest);
        if manifest.root_kind.is_directory() {
            if package_root.exists() && !resume_existing {
                bail!(
                    "destination directory already exists, refusing to overwrite: {}",
                    package_root.display()
                );
            }
            fs::create_dir_all(&package_root)
                .await
                .with_context(|| format!("failed to create package root directory {}", package_root.display()))?;
        }

        for entry in &manifest.entries {
            let destination_path = absolute_entry_path(output_dir, manifest, entry)?;
            if entry.is_directory() {
                fs::create_dir_all(&destination_path)
                    .await
                    .with_context(|| format!("failed to create destination directory {}", destination_path.display()))?;
                continue;
            }

            if let Some(parent) = destination_path.parent() {
                fs::create_dir_all(parent)
                    .await
                    .with_context(|| format!("failed to create destination parent directory {}", parent.display()))?;
            }

            let file_exists = destination_path.exists();
            if file_exists && !resume_existing {
                bail!(
                    "destination file already exists, refusing to overwrite: {}",
                    destination_path.display()
                );
            }

            let file = if file_exists {
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&destination_path)
                    .await
                    .with_context(|| format!("failed to open destination file {}", destination_path.display()))?
            } else {
                OpenOptions::new()
                    .create_new(true)
                    .read(true)
                    .write(true)
                    .open(&destination_path)
                    .await
                    .with_context(|| format!("failed to create destination file {}", destination_path.display()))?
            };
            file.set_len(entry.file_size)
                .await
                .with_context(|| format!("failed to size destination file {}", destination_path.display()))?;
        }

        Ok(Self {
            output_dir: output_dir.to_path_buf(),
            manifest: manifest.clone(),
        })
    }

    /// Streams an incoming chunk into its destination file and verifies the chunk hash.
    pub async fn write_chunk_stream<R>(&self, header: &ChunkStreamHeader, reader: &mut R) -> Result<()>
    where
        R: AsyncRead + Unpin,
    {
        let entry = self
            .manifest
            .entry(header.file_index as usize)
            .with_context(|| format!("missing file entry {} for incoming chunk", header.file_index))?;
        if !entry.is_file() {
            bail!("chunk {} referenced non-file entry {}", header.descriptor.index, entry.relative_path);
        }
        if !entry.contains_chunk(header.descriptor.index) {
            bail!(
                "chunk {} did not belong to file entry {}",
                header.descriptor.index,
                entry.relative_path
            );
        }

        let destination_path = absolute_entry_path(&self.output_dir, &self.manifest, entry)?;
        let descriptor = &header.descriptor;
        let mut remaining = u64::from(descriptor.size);
        let mut current_offset = descriptor.offset;
        let mut buffer = vec![0_u8; BUFFER_SIZE.min(descriptor.size as usize).max(1)];
        let mut hasher = Sha256State::new();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&destination_path)
            .await
            .with_context(|| format!("failed to open destination file {}", destination_path.display()))?;

        while remaining > 0 {
            let read_len = remaining.min(buffer.len() as u64) as usize;
            reader
                .read_exact(&mut buffer[..read_len])
                .await
                .with_context(|| format!("failed to read payload for chunk {}", descriptor.index))?;
            hasher.update(&buffer[..read_len]);

            file.seek(SeekFrom::Start(current_offset))
                .await
                .with_context(|| format!("failed to seek destination file for chunk {}", descriptor.index))?;
            file.write_all(&buffer[..read_len])
                .await
                .with_context(|| format!("failed to write destination bytes for chunk {}", descriptor.index))?;

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

        file.sync_data()
            .await
            .with_context(|| format!("failed to flush destination file {}", destination_path.display()))?;
        Ok(())
    }

    /// Verifies every file in the package against the manifest hash.
    pub async fn verify_files(&self) -> Result<()> {
        for entry in self.manifest.file_entries().map(|(_, entry)| entry) {
            let destination_path = absolute_entry_path(&self.output_dir, &self.manifest, entry)?;
            let expected_hash = entry.file_sha256;
            let relative_path = display_relative_path(&self.manifest, entry);
            let actual_hash = spawn_blocking(move || sha256_file(&destination_path))
                .await
                .context("file hashing task panicked")?
                .with_context(|| format!("failed to compute SHA-256 for {}", relative_path))?;
            if !verify_sha256(&actual_hash, &expected_hash) {
                bail!(
                    "file {} failed SHA-256 verification: expected {}, got {}",
                    relative_path,
                    format_sha256(&expected_hash),
                    format_sha256(&actual_hash)
                );
            }
        }
        Ok(())
    }

    /// Returns the final saved package root path.
    pub fn saved_path(&self) -> PathBuf {
        package_root_path(&self.output_dir, &self.manifest)
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

/// Inspects a selected file or directory without hashing file contents.
pub async fn inspect_source(source_path: &Path) -> Result<SourceInspection> {
    let source_path = source_path.to_path_buf();
    spawn_blocking(move || inspect_source_sync(&source_path))
        .await
        .context("source inspection task panicked")?
}

/// Builds the package manifest, file hashes, and chunk plan for a selected file or directory.
pub async fn build_source_package(source_path: &Path, chunk_size: u32) -> Result<SourcePackage> {
    let source_path = source_path.to_path_buf();
    spawn_blocking(move || build_source_package_sync(&source_path, chunk_size.max(1)))
        .await
        .context("source package build task panicked")?
}

/// Resolves the destination path for a received manifest entry.
pub fn absolute_entry_path(output_dir: &Path, manifest: &TransferManifest, entry: &PackageEntry) -> Result<PathBuf> {
    let relative_path = safe_relative_path(&entry.relative_path)?;
    let base = package_root_path(output_dir, manifest);
    Ok(match manifest.root_kind {
        PackageItemKind::Directory => base.join(relative_path),
        PackageItemKind::File => output_dir.join(relative_path),
    })
}

/// Returns a user-facing relative path for a manifest entry.
pub fn display_relative_path(manifest: &TransferManifest, entry: &PackageEntry) -> String {
    match manifest.root_kind {
        PackageItemKind::Directory => format!("{}/{}", manifest.root_name, entry.relative_path),
        PackageItemKind::File => entry.relative_path.clone(),
    }
}

fn inspect_source_sync(source_path: &Path) -> Result<SourceInspection> {
    let metadata = stdfs::metadata(source_path)
        .with_context(|| format!("failed to read source metadata for {}", source_path.display()))?;
    let root_name = root_name(source_path)?;

    if metadata.is_file() {
        return Ok(SourceInspection {
            source_path: source_path.to_path_buf(),
            root_name,
            root_kind: PackageItemKind::File,
            total_files: 1,
            total_directories: 0,
            total_bytes: metadata.len(),
        });
    }

    if !metadata.is_dir() {
        bail!("source path is neither a file nor directory: {}", source_path.display());
    }

    let mut directories = Vec::new();
    let mut files = Vec::new();
    collect_directory_entries(source_path, source_path, &mut directories, &mut files)?;
    let total_bytes = files.iter().map(|file| file.size).sum();

    Ok(SourceInspection {
        source_path: source_path.to_path_buf(),
        root_name,
        root_kind: PackageItemKind::Directory,
        total_files: files.len() as u64,
        total_directories: (directories.len() + 1) as u64,
        total_bytes,
    })
}

fn build_source_package_sync(source_path: &Path, chunk_size: u32) -> Result<SourcePackage> {
    let inspection = inspect_source_sync(source_path)?;
    match inspection.root_kind {
        PackageItemKind::File => build_single_file_package(source_path, inspection, chunk_size),
        PackageItemKind::Directory => build_directory_package(source_path, inspection, chunk_size),
    }
}

fn build_single_file_package(source_path: &Path, inspection: SourceInspection, chunk_size: u32) -> Result<SourcePackage> {
    let file_size = stdfs::metadata(source_path)
        .with_context(|| format!("failed to read source metadata for {}", source_path.display()))?
        .len();
    let file_sha256 = sha256_file(source_path)
        .with_context(|| format!("failed to compute SHA-256 for {}", source_path.display()))?;
    let local_descriptors = FixedChunker::new(file_size, chunk_size).descriptors();
    let chunk_count = local_descriptors.len() as u64;
    let relative_path = inspection.root_name.clone();

    let manifest = TransferManifest {
        root_name: inspection.root_name.clone(),
        root_kind: PackageItemKind::File,
        total_bytes: file_size,
        chunk_size,
        chunk_count,
        files_count: 1,
        directories_count: 0,
        entries: vec![PackageEntry {
            relative_path: relative_path.clone(),
            item_kind: PackageItemKind::File,
            file_size,
            file_sha256,
            first_chunk_index: 0,
            chunk_count,
        }],
    };

    let file = SourceFile {
        entry_index: 0,
        absolute_path: source_path.to_path_buf(),
        relative_path: relative_path.clone(),
        file_size,
        first_chunk_index: 0,
        chunk_count,
        file_sha256,
    };

    let chunks = local_descriptors
        .into_iter()
        .map(|descriptor| PackageChunkTask {
            file_index: 0,
            relative_path: relative_path.clone(),
            source_path: source_path.to_path_buf(),
            descriptor,
        })
        .collect();

    Ok(SourcePackage {
        source_path: source_path.to_path_buf(),
        inspection,
        manifest,
        files: vec![file],
        chunks,
    })
}

fn build_directory_package(source_path: &Path, inspection: SourceInspection, chunk_size: u32) -> Result<SourcePackage> {
    let mut directories = Vec::new();
    let mut file_nodes = Vec::new();
    collect_directory_entries(source_path, source_path, &mut directories, &mut file_nodes)?;

    let mut entries = Vec::new();
    let mut files = Vec::with_capacity(file_nodes.len());
    let mut chunks = Vec::new();
    let mut next_chunk_index = 0_u64;

    for directory in &directories {
        entries.push(PackageEntry {
            relative_path: directory.relative_path.clone(),
            item_kind: PackageItemKind::Directory,
            file_size: 0,
            file_sha256: [0_u8; 32],
            first_chunk_index: next_chunk_index,
            chunk_count: 0,
        });
    }

    for file_node in &file_nodes {
        let entry_index = entries.len();
        let file_sha256 = sha256_file(&file_node.absolute_path)
            .with_context(|| format!("failed to compute SHA-256 for {}", file_node.absolute_path.display()))?;
        let local_descriptors = FixedChunker::new(file_node.size, chunk_size).descriptors();
        let first_chunk_index = next_chunk_index;
        let chunk_count = local_descriptors.len() as u64;
        next_chunk_index += chunk_count;

        entries.push(PackageEntry {
            relative_path: file_node.relative_path.clone(),
            item_kind: PackageItemKind::File,
            file_size: file_node.size,
            file_sha256,
            first_chunk_index,
            chunk_count,
        });

        files.push(SourceFile {
            entry_index,
            absolute_path: file_node.absolute_path.clone(),
            relative_path: file_node.relative_path.clone(),
            file_size: file_node.size,
            first_chunk_index,
            chunk_count,
            file_sha256,
        });

        for descriptor in local_descriptors {
            chunks.push(PackageChunkTask {
                file_index: entry_index as u32,
                relative_path: file_node.relative_path.clone(),
                source_path: file_node.absolute_path.clone(),
                descriptor: ChunkDescriptor {
                    index: first_chunk_index + descriptor.index,
                    offset: descriptor.offset,
                    size: descriptor.size,
                },
            });
        }
    }

    let manifest = TransferManifest {
        root_name: inspection.root_name.clone(),
        root_kind: PackageItemKind::Directory,
        total_bytes: inspection.total_bytes,
        chunk_size,
        chunk_count: next_chunk_index,
        files_count: inspection.total_files,
        directories_count: inspection.total_directories,
        entries,
    };

    Ok(SourcePackage {
        source_path: source_path.to_path_buf(),
        inspection,
        manifest,
        files,
        chunks,
    })
}

#[derive(Debug, Clone)]
struct DirectoryEntryRecord {
    relative_path: String,
}

#[derive(Debug, Clone)]
struct FileEntryRecord {
    absolute_path: PathBuf,
    relative_path: String,
    size: u64,
}

fn collect_directory_entries(
    root_path: &Path,
    current_dir: &Path,
    directories: &mut Vec<DirectoryEntryRecord>,
    files: &mut Vec<FileEntryRecord>,
) -> Result<()> {
    let mut children = stdfs::read_dir(current_dir)
        .with_context(|| format!("failed to read directory {}", current_dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("failed to enumerate directory {}", current_dir.display()))?;
    children.sort_by_key(|entry| entry.file_name());

    for child in children {
        let child_path = child.path();
        let child_metadata = child
            .metadata()
            .with_context(|| format!("failed to read metadata for {}", child_path.display()))?;
        let relative = child_path
            .strip_prefix(root_path)
            .with_context(|| format!("failed to compute relative path for {}", child_path.display()))?;
        let relative_path = normalize_relative_path(relative)?;

        if child_metadata.is_dir() {
            directories.push(DirectoryEntryRecord {
                relative_path: relative_path.clone(),
            });
            collect_directory_entries(root_path, &child_path, directories, files)?;
        } else if child_metadata.is_file() {
            files.push(FileEntryRecord {
                absolute_path: child_path,
                relative_path,
                size: child_metadata.len(),
            });
        }
    }

    Ok(())
}

fn normalize_relative_path(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let text = part
                    .to_str()
                    .with_context(|| format!("path component {:?} was not valid UTF-8", part))?;
                parts.push(text.to_owned());
            }
            _ => bail!("path {} contained an unsupported component", path.display()),
        }
    }

    if parts.is_empty() {
        bail!("relative path was empty");
    }

    Ok(parts.join("/"))
}

fn safe_relative_path(relative_path: &str) -> Result<PathBuf> {
    let mut path = PathBuf::new();
    for part in relative_path.split('/') {
        if part.is_empty() {
            bail!("relative path contained an empty segment: {relative_path}");
        }
        let component_path = Path::new(part);
        let mut components = component_path.components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(name)), None) => path.push(OsString::from(name)),
            _ => bail!("relative path segment was not safe: {relative_path}"),
        }
    }
    Ok(path)
}

fn package_root_path(output_dir: &Path, manifest: &TransferManifest) -> PathBuf {
    match manifest.root_kind {
        PackageItemKind::Directory => output_dir.join(&manifest.root_name),
        PackageItemKind::File => output_dir.join(&manifest.root_name),
    }
}

fn root_name(source_path: &Path) -> Result<String> {
    source_path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .with_context(|| format!("failed to derive a valid root name from {}", source_path.display()))
}
