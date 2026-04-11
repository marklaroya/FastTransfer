use std::{
    ffi::OsString,
    fs as stdfs,
    path::{Component, Path, PathBuf},
    time::Instant,
};

use anyhow::{bail, Context, Result};
use integrity::{format_sha256, Sha256State};
use protocol::PackageItemKind;
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncReadExt, AsyncWriteExt},
};

use crate::{
    configure_reporter,
    progress::ProgressListener,
    ProgressReporter,
    TransferControl,
    TransferIssue,
    TransferSummary,
    TransferSourceSummary,
};

const MIN_BUFFER_SIZE: usize = 64 * 1024;
const MAX_BUFFER_SIZE: usize = 8 * 1024 * 1024;

async fn wait_for_transfer_control(control: Option<&TransferControl>) -> Result<()> {
    if let Some(control) = control {
        control.wait_if_paused().await?;
    }

    Ok(())
}

fn record_transfer_issue(
    issues: &mut Vec<TransferIssue>,
    failed_files: &mut u64,
    path: &str,
    message: impl Into<String>,
) {
    *failed_files = failed_files.saturating_add(1);
    issues.push(TransferIssue {
        path: path.to_owned(),
        message: message.into(),
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalDestinationKind {
    LocalFolder,
    UsbDrive,
}

impl LocalDestinationKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LocalFolder => "local_folder",
            Self::UsbDrive => "usb_drive",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LocalCopyRequest {
    pub source_path: PathBuf,
    pub destination_dir: PathBuf,
    pub chunk_size: u32,
    pub parallelism: usize,
    pub destination_kind: LocalDestinationKind,
}

pub(crate) async fn copy_local_streaming(
    request: LocalCopyRequest,
    progress_listener: Option<ProgressListener>,
    render_terminal: bool,
    control: Option<TransferControl>,
) -> Result<TransferSummary> {
    wait_for_transfer_control(control.as_ref()).await?;

    let inspection = crate::file_io::inspect_source(request.source_path.as_path())
        .await
        .with_context(|| {
            format!(
                "failed to inspect local copy source {}",
                request.source_path.display()
            )
        })?;

    println!(
        "[DEST] Local copy destination selected: {} ({})",
        request.destination_dir.display(),
        request.destination_kind.as_str()
    );

    fs::create_dir_all(&request.destination_dir)
        .await
        .with_context(|| {
            format!(
                "failed to create destination directory {}",
                request.destination_dir.display()
            )
        })?;

    let mut reporter = configure_reporter(
        ProgressReporter::new(
            format!("Copying {}", inspection.root_name),
            inspection.total_bytes,
            inspection.total_files,
        ),
        progress_listener,
        render_terminal,
    );
    reporter.set_phase("sending");

    let mut aggregate_hasher = Sha256State::new();
    let mut completed_files = 0_u64;
    let mut completed_chunks = 0_u64;
    let mut failed_files = 0_u64;
    let mut issues = Vec::new();

    let package_root = request.destination_dir.join(&inspection.root_name);
    match inspection.root_kind {
        PackageItemKind::File => {
            if let Err(error) = copy_single_file(
                request.source_path.as_path(),
                package_root.as_path(),
                inspection.root_name.as_str(),
                &request,
                &mut reporter,
                &mut completed_files,
                &mut completed_chunks,
                &mut aggregate_hasher,
                control.as_ref(),
            )
            .await
            {
                record_transfer_issue(
                    &mut issues,
                    &mut failed_files,
                    inspection.root_name.as_str(),
                    format!("{error:#}"),
                );
            }
        }
        PackageItemKind::Directory => {
            fs::create_dir_all(&package_root)
                .await
                .with_context(|| format!("failed to create package root {}", package_root.display()))?;

            stream_directory_copy(
                request.source_path.as_path(),
                package_root.as_path(),
                &inspection,
                &request,
                &mut reporter,
                &mut completed_files,
                &mut completed_chunks,
                &mut failed_files,
                &mut issues,
                &mut aggregate_hasher,
                control.as_ref(),
            )
            .await?;
        }
    }

    reporter.set_completed_files(completed_files);
    let snapshot = reporter.finish();
    let aggregate = aggregate_hasher.finalize();

    println!(
        "[TRANSFER] Local transfer complete: {} bytes copied to {}",
        snapshot.bytes_transferred,
        request.destination_dir.display()
    );

    Ok(TransferSummary {
        file_name: inspection.root_name,
        bytes_transferred: snapshot.bytes_transferred,
        elapsed: snapshot.elapsed,
        average_mib_per_sec: snapshot.average_mib_per_sec,
        completed_chunks,
        completed_files,
        total_files: inspection.total_files,
        failed_files,
        total_directories: inspection.total_directories,
        sha256_hex: format_sha256(&aggregate),
        integrity_verified: failed_files == 0,
        issues,
    })
}

#[allow(clippy::too_many_arguments)]
async fn stream_directory_copy(
    source_root: &Path,
    destination_root: &Path,
    inspection: &TransferSourceSummary,
    request: &LocalCopyRequest,
    reporter: &mut ProgressReporter,
    completed_files: &mut u64,
    completed_chunks: &mut u64,
    failed_files: &mut u64,
    issues: &mut Vec<TransferIssue>,
    aggregate_hasher: &mut Sha256State,
    control: Option<&TransferControl>,
) -> Result<()> {
    let mut stack = vec![DirectoryFrame {
        entries: read_sorted_entries(source_root)?,
        index: 0,
    }];

    while let Some(frame) = stack.last_mut() {
        wait_for_transfer_control(control).await?;

        if frame.index >= frame.entries.len() {
            stack.pop();
            continue;
        }

        let entry = frame.entries[frame.index].clone();
        frame.index += 1;

        let relative = entry
            .path
            .strip_prefix(source_root)
            .with_context(|| format!("failed to compute relative path for {}", entry.path.display()))?;
        let relative_path = match normalize_relative_path(relative) {
            Ok(path) => path,
            Err(error) => {
                record_transfer_issue(
                    issues,
                    failed_files,
                    &entry.path.display().to_string(),
                    format!("failed to normalize relative path: {error:#}"),
                );
                continue;
            }
        };
        let destination_path = match safe_relative_path(&relative_path) {
            Ok(path) => destination_root.join(path),
            Err(error) => {
                record_transfer_issue(
                    issues,
                    failed_files,
                    &format!("{}/{}", inspection.root_name, relative_path),
                    format!("destination path was not safe: {error:#}"),
                );
                continue;
            }
        };

        if entry.is_dir {
            if let Err(error) = fs::create_dir_all(&destination_path).await {
                record_transfer_issue(
                    issues,
                    failed_files,
                    &format!("{}/{}", inspection.root_name, relative_path),
                    format!(
                        "failed to create destination directory {}: {error:#}",
                        destination_path.display()
                    ),
                );
                continue;
            }
            reporter.set_current_path(Some(format!(
                "{}/{}",
                inspection.root_name, relative_path
            )));
            let child_entries = match read_sorted_entries(&entry.path) {
                Ok(entries) => entries,
                Err(error) => {
                    record_transfer_issue(
                        issues,
                        failed_files,
                        &format!("{}/{}", inspection.root_name, relative_path),
                        format!(
                            "failed to enumerate source directory {}: {error:#}",
                            entry.path.display()
                        ),
                    );
                    continue;
                }
            };
            stack.push(DirectoryFrame {
                entries: child_entries,
                index: 0,
            });
            continue;
        }

        if entry.is_file {
            if let Err(error) = copy_single_file(
                &entry.path,
                &destination_path,
                format!("{}/{}", inspection.root_name, relative_path).as_str(),
                request,
                reporter,
                completed_files,
                completed_chunks,
                aggregate_hasher,
                control,
            )
            .await
            {
                record_transfer_issue(
                    issues,
                    failed_files,
                    &format!("{}/{}", inspection.root_name, relative_path),
                    format!("{error:#}"),
                );
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn copy_single_file(
    source_path: &Path,
    destination_path: &Path,
    display_path: &str,
    request: &LocalCopyRequest,
    reporter: &mut ProgressReporter,
    completed_files: &mut u64,
    completed_chunks: &mut u64,
    aggregate_hasher: &mut Sha256State,
    control: Option<&TransferControl>,
) -> Result<()> {
    wait_for_transfer_control(control).await?;

    if request.destination_kind == LocalDestinationKind::UsbDrive && !request.destination_dir.exists() {
        bail!(
            "destination drive was removed before copying {}",
            display_path
        );
    }

    let file_start = Instant::now();
    println!("[TRANSFER] Starting file: {}", display_path);

    let metadata = stdfs::metadata(source_path)
        .with_context(|| format!("failed to read source metadata for {}", source_path.display()))?;
    let file_size = metadata.len();
    let chunk_size = request.chunk_size.max(1) as usize;
    let chunk_count = if file_size == 0 {
        0
    } else {
        ((file_size + request.chunk_size.max(1) as u64 - 1) / request.chunk_size.max(1) as u64) as u64
    };

    if let Some(parent) = destination_path.parent() {
        fs::create_dir_all(parent)
            .await
            .with_context(|| {
                format!(
                    "failed to create destination parent directory {}",
                    parent.display()
                )
            })?;
    }

    reporter.set_current_path(Some(display_path.to_owned()));
    println!("[TRANSFER] Metadata sent after {:?}", file_start.elapsed());

    let mut source = File::open(source_path)
        .await
        .with_context(|| format!("failed to open source file {}", source_path.display()))?;
    let mut destination = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(destination_path)
        .await
        .with_context(|| format!("failed to open destination file {}", destination_path.display()))?;

    println!("[HASH] Start hashing {}", display_path);
    let hash_started = Instant::now();
    let mut file_hasher = Sha256State::new();

    let mut buffer = vec![0_u8; chunk_size.clamp(MIN_BUFFER_SIZE, MAX_BUFFER_SIZE)];
    let mut first_chunk_logged = false;
    let mut chunk_index = 0_u64;

    loop {
        wait_for_transfer_control(control).await?;

        if request.destination_kind == LocalDestinationKind::UsbDrive && !request.destination_dir.exists() {
            bail!("destination drive was removed while copying {}", display_path);
        }

        let read = source
            .read(&mut buffer)
            .await
            .with_context(|| format!("failed to read source file {}", source_path.display()))?;
        if read == 0 {
            break;
        }

        destination
            .write_all(&buffer[..read])
            .await
            .with_context(|| format!("failed to write destination file {}", destination_path.display()))?;
        file_hasher.update(&buffer[..read]);
        reporter.advance(read as u64);

        chunk_index = chunk_index.saturating_add(1);
        if !first_chunk_logged {
            first_chunk_logged = true;
            println!("[TRANSFER] First chunk sent after {:?}", file_start.elapsed());
        }
        if chunk_index == 1 || chunk_index % 256 == 0 {
            println!(
                "[TRANSFER] Chunk write {} for {} ({} bytes)",
                chunk_index,
                display_path,
                read
            );
        }
    }

    destination
        .sync_data()
        .await
        .with_context(|| format!("failed to flush destination file {}", destination_path.display()))?;

    let file_hash = file_hasher.finalize();
    println!(
        "[HASH] Finished hashing {} in {:?}",
        display_path,
        hash_started.elapsed()
    );

    aggregate_hasher.update(&file_hash);
    *completed_files = completed_files.saturating_add(1);
    *completed_chunks = completed_chunks.saturating_add(chunk_count);
    reporter.set_completed_files(*completed_files);

    println!("[TRANSFER] Finished file in {:?}", file_start.elapsed());

    Ok(())
}

#[derive(Debug, Clone)]
struct DirectoryFrame {
    entries: Vec<FsEntry>,
    index: usize,
}

#[derive(Debug, Clone)]
struct FsEntry {
    path: PathBuf,
    name: OsString,
    is_dir: bool,
    is_file: bool,
}

fn read_sorted_entries(directory: &Path) -> Result<Vec<FsEntry>> {
    let mut entries = stdfs::read_dir(directory)
        .with_context(|| format!("failed to read directory {}", directory.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("failed to enumerate directory {}", directory.display()))?
        .into_iter()
        .map(|entry| {
            let path = entry.path();
            let metadata = entry
                .metadata()
                .with_context(|| format!("failed to read metadata for {}", path.display()))?;
            Ok::<FsEntry, anyhow::Error>(FsEntry {
                path,
                name: entry.file_name(),
                is_dir: metadata.is_dir(),
                is_file: metadata.is_file(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(entries)
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
