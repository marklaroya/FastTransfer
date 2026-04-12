use std::{
    collections::HashSet,
    ffi::OsString,
    fs as stdfs,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use anyhow::{bail, Context, Result};
use chunker::FixedChunker;
use integrity::{
    format_sha256, sha256_bytes, sha256_file, verify_sha256, Sha256State,
};
use protocol::{
    ChunkDescriptor, ChunkStreamHeader, PackageItemKind, ReceiverControlMessage,
    SenderControlMessage, StreamDirectoryEntry, StreamFileComplete, StreamFileDisposition,
    StreamFileEntry, StreamFileResult, StreamSessionStart, StreamTransferComplete,
    StreamTransferStatus,
};
use quic_transport::{IncomingConnection, QuicReceiver, QuicSender};
use resume::PersistentFileResumeState;
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom},
    sync::Semaphore,
    task::{spawn_blocking, JoinSet},
};

use crate::{
    configure_reporter, sender_checkpoint_base_dir, ProgressListener, ProgressReporter,
    ReceivedTransfer, SendRequest, TransferControl, TransferIssue, TransferSummary,
};

const BUFFER_SIZE: usize = 256 * 1024;

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

pub(crate) async fn send_streaming(
    request: SendRequest,
    progress_listener: Option<ProgressListener>,
    render_terminal: bool,
    transfer_control: Option<TransferControl>,
) -> Result<TransferSummary> {
    wait_for_transfer_control(transfer_control.as_ref()).await?;
    let source_path = request.source_path.clone();
    let metadata = stdfs::metadata(&source_path)
        .with_context(|| format!("failed to read source metadata for {}", source_path.display()))?;
    let root_name = root_name(&source_path)?;
    let root_kind = if metadata.is_dir() {
        PackageItemKind::Directory
    } else if metadata.is_file() {
        PackageItemKind::File
    } else {
        bail!("source path is neither a file nor directory: {}", source_path.display());
    };

    let mut reporter = configure_reporter(
        ProgressReporter::new(format!("Scanning {}", root_name), 0, 0),
        progress_listener,
        render_terminal,
    );
    reporter.set_phase("scanning");

    let checkpoint_dir = sender_checkpoint_base_dir(&source_path).to_path_buf();
    let mut checkpoint = PersistentFileResumeState::load_or_recreate_in(
        &checkpoint_dir,
        &root_name,
        root_kind,
        request.chunk_size.max(1),
    )
    .with_context(|| format!("failed to load sender streaming checkpoint for {}", source_path.display()))?;
    let resume_from_existing_checkpoint = checkpoint.existed() && checkpoint.has_progress();

    let transport = Arc::new(
        QuicSender::connect(request.server_addr, &request.server_name, &request.certificate_path).await?,
    );
    let mut control = transport.open_streaming_control_stream().await?;
    control
        .send_message(&SenderControlMessage::SessionStart(StreamSessionStart {
            root_name: root_name.clone(),
            root_kind,
            chunk_size: request.chunk_size.max(1),
        }))
        .await?;

    let mut next_file_id = 0_u32;
    let mut total_files = 0_u64;
    let mut total_directories = if root_kind == PackageItemKind::Directory { 1_u64 } else { 0_u64 };
    let mut total_bytes = 0_u64;
    let mut completed_files = 0_u64;
    let mut completed_bytes = 0_u64;
    let mut completed_chunks = 0_u64;
    let mut failed_files = 0_u64;
    let mut issues = Vec::new();
    let mut aggregate_hasher = Sha256State::new();

    if root_kind == PackageItemKind::File {
        process_file(
            &source_path,
            root_name.clone(),
            &request,
            &transport,
            &mut control,
            &mut checkpoint,
            &mut reporter,
            &mut next_file_id,
            &mut total_files,
            &mut total_bytes,
            &mut completed_files,
            &mut completed_bytes,
            &mut completed_chunks,
            &mut failed_files,
            &mut issues,
            &mut aggregate_hasher,
            transfer_control.as_ref(),
            resume_from_existing_checkpoint,
        )
        .await?;
    } else {
        stream_directory(
            &source_path,
            &root_name,
            &request,
            &transport,
            &mut control,
            &mut checkpoint,
            &mut reporter,
            &mut next_file_id,
            &mut total_files,
            &mut total_directories,
            &mut total_bytes,
            &mut completed_files,
            &mut completed_bytes,
            &mut completed_chunks,
            &mut failed_files,
            &mut issues,
            &mut aggregate_hasher,
            transfer_control.as_ref(),
            resume_from_existing_checkpoint,
        )
        .await?;
    }

    let aggregate_sha256 = aggregate_hasher.finalize();
    control
        .send_message(&SenderControlMessage::TransferComplete(StreamTransferComplete {
            total_files: completed_files,
            total_directories,
            total_bytes: completed_bytes,
            aggregate_sha256,
        }))
        .await?;

    let status = control.read_message().await?;
    match status {
        ReceiverControlMessage::TransferStatus(StreamTransferStatus { complete: true }) => {}
        ReceiverControlMessage::TransferStatus(StreamTransferStatus { complete: false }) => {
            bail!("receiver reported incomplete transfer status")
        }
        other => bail!("unexpected receiver control message at transfer completion: {other:?}"),
    }

    let _ = control.finish();

    checkpoint.remove().with_context(|| {
        format!(
            "failed to remove sender streaming checkpoint for {}",
            source_path.display()
        )
    })?;

    reporter.set_phase("sending");
    reporter.set_completed_files(completed_files);
    let snapshot = reporter.finish();
    transport.close();

    Ok(TransferSummary {
        file_name: root_name,
        bytes_transferred: snapshot.bytes_transferred,
        elapsed: snapshot.elapsed,
        average_mib_per_sec: snapshot.average_mib_per_sec,
        completed_chunks: completed_chunks,
        completed_files,
        total_files,
        failed_files,
        total_directories,
        sha256_hex: format_sha256(&aggregate_sha256),
        integrity_verified: failed_files == 0,
        issues,
    })
}

pub(crate) async fn receive_streaming(
    output_dir: PathBuf,
    transport: &QuicReceiver,
    progress_listener: Option<ProgressListener>,
    render_terminal: bool,
) -> Result<ReceivedTransfer> {
    let incoming = transport.accept_connection().await?;
    let remote_address = incoming.remote_address;
    let mut control = incoming.accept_streaming_control_stream().await?;

    let session = match control.read_message().await? {
        SenderControlMessage::SessionStart(session) => session,
        other => bail!("expected session-start control frame, got {other:?}"),
    };

    let mut reporter = configure_reporter(
        ProgressReporter::new(format!("Scanning {}", session.root_name), 0, 0),
        progress_listener,
        render_terminal,
    );
    reporter.set_phase("scanning");

    let mut checkpoint = PersistentFileResumeState::load_or_create_in(
        &output_dir,
        &session.root_name,
        session.root_kind,
        session.chunk_size.max(1),
    )
    .with_context(|| {
        format!(
            "failed to load receiver streaming checkpoint for {}",
            session.root_name
        )
    })?;

    let package_root = package_root_path(&output_dir, session.root_kind, &session.root_name);
    let package_root_fs = filesystem_path(&package_root);
    if session.root_kind == PackageItemKind::Directory {
        if package_root_fs.exists() && !checkpoint.existed() {
            bail!(
                "destination directory already exists, refusing to overwrite: {}",
                package_root.display()
            );
        }
        fs::create_dir_all(&package_root_fs)
            .await
            .with_context(|| format!("failed to create package root {}", package_root.display()))?;
    }

    let mut total_files = 0_u64;
    let mut total_directories = if session.root_kind == PackageItemKind::Directory { 1_u64 } else { 0_u64 };
    let mut total_bytes = 0_u64;
    let mut completed_files = 0_u64;
    let mut completed_bytes = 0_u64;
    let mut completed_chunks = 0_u64;
    let mut failed_files = 0_u64;
    let mut issues = Vec::new();
    let mut aggregate_hasher = Sha256State::new();

    loop {
        let message = control.read_message().await?;
        match message {
            SenderControlMessage::DirectoryEntry(StreamDirectoryEntry { relative_path }) => {
                total_directories = total_directories.saturating_add(1);
                let destination = destination_path(
                    &output_dir,
                    session.root_kind,
                    &session.root_name,
                    &relative_path,
                )?;
                let destination_fs = filesystem_path(&destination);
                fs::create_dir_all(&destination_fs).await.with_context(|| {
                    format!("failed to create destination directory {}", destination.display())
                })?;
                reporter.set_current_path(Some(display_current_path(
                    &session.root_name,
                    session.root_kind,
                    &relative_path,
                )));
            }
            SenderControlMessage::FileEntry(file_entry) => {
                total_files = total_files.saturating_add(1);
                total_bytes = total_bytes.saturating_add(file_entry.file_size);
                reporter.set_totals(total_bytes, total_files);
                reporter.set_current_path(Some(display_current_path(
                    &session.root_name,
                    session.root_kind,
                    &file_entry.relative_path,
                )));

                let destination = match destination_path(
                    &output_dir,
                    session.root_kind,
                    &session.root_name,
                    &file_entry.relative_path,
                ) {
                    Ok(path) => path,
                    Err(error) => {
                        let message = format!("receiver could not prepare {}: {error:#}", file_entry.relative_path);
                        control
                            .send_message(&ReceiverControlMessage::FileDisposition(StreamFileDisposition {
                                file_id: file_entry.file_id,
                                needed: false,
                            }))
                            .await?;
                        control
                            .send_message(&ReceiverControlMessage::FileResult(StreamFileResult {
                                file_id: file_entry.file_id,
                                success: false,
                                message: message.clone(),
                            }))
                            .await?;
                        record_transfer_issue(&mut issues, &mut failed_files, &file_entry.relative_path, message);
                        continue;
                    }
                };
                let destination_fs = filesystem_path(&destination);

                if let Some(parent) = destination.parent() {
                    let parent_fs = filesystem_path(parent);
                    if let Err(error) = fs::create_dir_all(&parent_fs).await {
                        let message = format!(
                            "failed to create destination parent directory {}: {error:#}",
                            parent.display()
                        );
                        control
                            .send_message(&ReceiverControlMessage::FileDisposition(StreamFileDisposition {
                                file_id: file_entry.file_id,
                                needed: false,
                            }))
                            .await?;
                        control
                            .send_message(&ReceiverControlMessage::FileResult(StreamFileResult {
                                file_id: file_entry.file_id,
                                success: false,
                                message: message.clone(),
                            }))
                            .await?;
                        record_transfer_issue(&mut issues, &mut failed_files, &file_entry.relative_path, message);
                        continue;
                    }
                }

                let already_complete = file_entry
                    .file_sha256
                    .as_ref()
                    .map(|file_sha256| {
                        checkpoint.is_file_complete(
                            &file_entry.relative_path,
                            file_entry.file_size,
                            file_sha256,
                        ) && destination_fs.exists()
                    })
                    .unwrap_or(false);

                control
                    .send_message(&ReceiverControlMessage::FileDisposition(StreamFileDisposition {
                        file_id: file_entry.file_id,
                        needed: !already_complete,
                    }))
                    .await?;

                if already_complete {
                    reporter.advance(file_entry.file_size);
                    completed_files = completed_files.saturating_add(1);
                    completed_bytes = completed_bytes.saturating_add(file_entry.file_size);
                    completed_chunks = completed_chunks.saturating_add(file_entry.chunk_count);
                    aggregate_hasher.update(file_entry.file_sha256.as_ref().expect("already complete files require known hash"));
                    reporter.set_completed_files(completed_files);
                    control
                        .send_message(&ReceiverControlMessage::FileResult(StreamFileResult {
                            file_id: file_entry.file_id,
                            success: true,
                            message: "already present".to_owned(),
                        }))
                        .await?;
                    continue;
                }

                let receive_result = receive_file_chunks(
                    &incoming,
                    &destination,
                    &file_entry,
                    &session,
                    &mut reporter,
                )
                .await;

                let file_sha256 = match control.read_message().await? {
                    SenderControlMessage::FileComplete(StreamFileComplete { file_id, file_sha256 })
                        if file_id == file_entry.file_id => file_sha256,
                    other => bail!(
                        "expected file-complete for file {} but got {other:?}",
                        file_entry.file_id
                    ),
                };

                if let Err(error) = receive_result {
                    let message = format!("failed to receive {}: {error:#}", file_entry.relative_path);
                    control
                        .send_message(&ReceiverControlMessage::FileResult(StreamFileResult {
                            file_id: file_entry.file_id,
                            success: false,
                            message: message.clone(),
                        }))
                        .await?;
                    record_transfer_issue(&mut issues, &mut failed_files, &file_entry.relative_path, message);
                    continue;
                }

                let destination_for_hash = filesystem_path(&destination);
                let actual_hash = match spawn_blocking(move || sha256_file(&destination_for_hash)).await {
                    Ok(Ok(hash)) => hash,
                    Ok(Err(error)) => {
                        let message = format!("receiver failed to hash {}: {error:#}", file_entry.relative_path);
                        control
                            .send_message(&ReceiverControlMessage::FileResult(StreamFileResult {
                                file_id: file_entry.file_id,
                                success: false,
                                message: message.clone(),
                            }))
                            .await?;
                        record_transfer_issue(&mut issues, &mut failed_files, &file_entry.relative_path, message);
                        continue;
                    }
                    Err(error) => {
                        let message = format!("receiver file hashing task panicked for {}: {error}", file_entry.relative_path);
                        control
                            .send_message(&ReceiverControlMessage::FileResult(StreamFileResult {
                                file_id: file_entry.file_id,
                                success: false,
                                message: message.clone(),
                            }))
                            .await?;
                        record_transfer_issue(&mut issues, &mut failed_files, &file_entry.relative_path, message);
                        continue;
                    }
                };
                if !verify_sha256(&actual_hash, &file_sha256) {
                    let message = format!(
                        "integrity mismatch: expected {}, got {}",
                        format_sha256(&file_sha256),
                        format_sha256(&actual_hash)
                    );
                    control
                        .send_message(&ReceiverControlMessage::FileResult(StreamFileResult {
                            file_id: file_entry.file_id,
                            success: false,
                            message: message.clone(),
                        }))
                        .await?;
                    record_transfer_issue(&mut issues, &mut failed_files, &file_entry.relative_path, message);
                    continue;
                }

                if let Err(error) = checkpoint.mark_file_complete(
                    file_entry.relative_path.clone(),
                    file_entry.file_size,
                    file_sha256,
                ) {
                    let message = format!(
                        "failed to persist receiver streaming checkpoint for {}: {error:#}",
                        file_entry.relative_path
                    );
                    control
                        .send_message(&ReceiverControlMessage::FileResult(StreamFileResult {
                            file_id: file_entry.file_id,
                            success: false,
                            message: message.clone(),
                        }))
                        .await?;
                    record_transfer_issue(&mut issues, &mut failed_files, &file_entry.relative_path, message);
                    continue;
                }

                completed_files = completed_files.saturating_add(1);
                completed_bytes = completed_bytes.saturating_add(file_entry.file_size);
                completed_chunks = completed_chunks.saturating_add(file_entry.chunk_count);
                aggregate_hasher.update(&file_sha256);
                reporter.set_completed_files(completed_files);
                control
                    .send_message(&ReceiverControlMessage::FileResult(StreamFileResult {
                        file_id: file_entry.file_id,
                        success: true,
                        message: "verified".to_owned(),
                    }))
                    .await?;
            }
            SenderControlMessage::TransferComplete(transfer_complete) => {
                if transfer_complete.total_files != completed_files
                    || transfer_complete.total_directories != total_directories
                    || transfer_complete.total_bytes != completed_bytes
                {
                    control
                        .send_message(&ReceiverControlMessage::TransferStatus(StreamTransferStatus {
                            complete: false,
                        }))
                        .await?;
                    bail!("sender transfer summary did not match receiver observations");
                }

                let aggregate = aggregate_hasher.finalize();
                if aggregate != transfer_complete.aggregate_sha256 {
                    control
                        .send_message(&ReceiverControlMessage::TransferStatus(StreamTransferStatus {
                            complete: false,
                        }))
                        .await?;
                    bail!(
                        "transfer aggregate hash mismatch: expected {}, got {}",
                        format_sha256(&transfer_complete.aggregate_sha256),
                        format_sha256(&aggregate)
                    );
                }

                control
                    .send_message(&ReceiverControlMessage::TransferStatus(StreamTransferStatus {
                        complete: true,
                    }))
                    .await?;
                let _ = control.finish();

                checkpoint.remove().with_context(|| {
                    format!(
                        "failed to remove receiver streaming checkpoint for {}",
                        session.root_name
                    )
                })?;

                reporter.set_phase("sending");
                reporter.set_completed_files(completed_files);
                let snapshot = reporter.finish();

                return Ok(ReceivedTransfer {
                    summary: TransferSummary {
                        file_name: session.root_name.clone(),
                        bytes_transferred: snapshot.bytes_transferred,
                        elapsed: snapshot.elapsed,
                        average_mib_per_sec: snapshot.average_mib_per_sec,
                        completed_chunks: completed_chunks,
                        completed_files,
                        total_files,
                        failed_files,
                        total_directories,
                        sha256_hex: format_sha256(&aggregate),
                        integrity_verified: failed_files == 0,
                        issues,
                    },
                    saved_path: package_root_path(&output_dir, session.root_kind, &session.root_name),
                    remote_address,
                });
            }
            SenderControlMessage::SessionStart(_) => {
                bail!("received duplicate session-start control message")
            }
            SenderControlMessage::FileComplete(StreamFileComplete { file_id, .. }) => {
                bail!("received unexpected file-complete for file id {file_id}")
            }
        }
    }
}


async fn stream_directory(
    source_root: &Path,
    root_name: &str,
    request: &SendRequest,
    transport: &Arc<QuicSender>,
    control: &mut quic_transport::StreamingSenderControlStream,
    checkpoint: &mut PersistentFileResumeState,
    reporter: &mut ProgressReporter,
    next_file_id: &mut u32,
    total_files: &mut u64,
    total_directories: &mut u64,
    total_bytes: &mut u64,
    completed_files: &mut u64,
    completed_bytes: &mut u64,
    completed_chunks: &mut u64,
    failed_files: &mut u64,
    issues: &mut Vec<TransferIssue>,
    aggregate_hasher: &mut Sha256State,
    transfer_control: Option<&TransferControl>,
    resume_from_existing_checkpoint: bool,
) -> Result<()> {
    let mut stack = vec![DirectoryFrame {
        entries: read_sorted_entries(source_root)?,
        index: 0,
    }];

    while let Some(frame) = stack.last_mut() {
        wait_for_transfer_control(transfer_control).await?;
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
        let relative_path = normalize_relative_path(relative)?;

        if entry.is_dir {
            control
                .send_message(&SenderControlMessage::DirectoryEntry(StreamDirectoryEntry {
                    relative_path: relative_path.clone(),
                }))
                .await?;
            *total_directories = total_directories.saturating_add(1);
            reporter.set_current_path(Some(display_current_path(
                root_name,
                PackageItemKind::Directory,
                &relative_path,
            )));
            stack.push(DirectoryFrame {
                entries: read_sorted_entries(&entry.path)?,
                index: 0,
            });
            continue;
        }

        if entry.is_file {
            process_file(
                &entry.path,
                relative_path,
                request,
                transport,
                control,
                checkpoint,
                reporter,
                next_file_id,
                total_files,
                total_bytes,
                completed_files,
                completed_bytes,
                completed_chunks,
                failed_files,
                issues,
                aggregate_hasher,
                transfer_control,
                resume_from_existing_checkpoint,
            )
            .await?;
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn process_file(
    source_path: &Path,
    relative_path: String,
    request: &SendRequest,
    transport: &Arc<QuicSender>,
    control: &mut quic_transport::StreamingSenderControlStream,
    checkpoint: &mut PersistentFileResumeState,
    reporter: &mut ProgressReporter,
    next_file_id: &mut u32,
    total_files: &mut u64,
    total_bytes: &mut u64,
    completed_files: &mut u64,
    completed_bytes: &mut u64,
    completed_chunks: &mut u64,
    failed_files: &mut u64,
    issues: &mut Vec<TransferIssue>,
    aggregate_hasher: &mut Sha256State,
    transfer_control: Option<&TransferControl>,
    resume_from_existing_checkpoint: bool,
) -> Result<()> {
    wait_for_transfer_control(transfer_control).await?;
    let metadata = match stdfs::metadata(source_path) {
        Ok(metadata) => metadata,
        Err(error) => {
            record_transfer_issue(
                issues,
                failed_files,
                &relative_path,
                format!("sender could not read source metadata: {error}"),
            );
            return Ok(());
        }
    };
    if !metadata.is_file() {
        record_transfer_issue(
            issues,
            failed_files,
            &relative_path,
            format!(
                "expected file while scanning {}, found non-file item",
                source_path.display()
            ),
        );
        return Ok(());
    }

    let file_size = metadata.len();
    let known_file_sha256 = if resume_from_existing_checkpoint {
        let source_for_hash = source_path.to_path_buf();
        Some(match spawn_blocking(move || sha256_file(&source_for_hash)).await {
            Ok(Ok(hash)) => hash,
            Ok(Err(error)) => {
                record_transfer_issue(
                    issues,
                    failed_files,
                    &relative_path,
                    format!("sender failed to hash {relative_path}: {error:#}"),
                );
                return Ok(());
            }
            Err(error) => {
                record_transfer_issue(
                    issues,
                    failed_files,
                    &relative_path,
                    format!("sender file hashing task panicked for {relative_path}: {error}"),
                );
                return Ok(());
            }
        })
    } else {
        None
    };

    let descriptors = FixedChunker::new(file_size, request.chunk_size.max(1)).descriptors();
    let chunk_count = descriptors.len() as u64;
    let file_id = *next_file_id;
    *next_file_id = next_file_id.saturating_add(1);

    *total_files = total_files.saturating_add(1);
    *total_bytes = total_bytes.saturating_add(file_size);

    reporter.set_totals(*total_bytes, *total_files);
    reporter.set_current_path(Some(display_current_path(
        &checkpoint.metadata.root_name,
        checkpoint.metadata.root_kind,
        &relative_path,
    )));

    control
        .send_message(&SenderControlMessage::FileEntry(StreamFileEntry {
            file_id,
            relative_path: relative_path.clone(),
            file_size,
            file_sha256: known_file_sha256,
            chunk_count,
        }))
        .await?;

    let disposition = match control.read_message().await? {
        ReceiverControlMessage::FileDisposition(disposition) if disposition.file_id == file_id => {
            disposition
        }
        other => bail!("expected file-disposition for file {} but got {other:?}", file_id),
    };

    let file_sha256 = if disposition.needed {
        reporter.set_phase("sending");
        let file_sha256 = send_file_chunks(
            source_path,
            file_id,
            &descriptors,
            request.parallelism,
            transport,
            reporter,
            transfer_control,
        )
        .await?;

        control
            .send_message(&SenderControlMessage::FileComplete(StreamFileComplete { file_id, file_sha256 }))
            .await?;
        file_sha256
    } else {
        known_file_sha256.ok_or_else(|| anyhow::anyhow!("receiver skipped file without a known sender hash"))?
    };

    let result = match control.read_message().await? {
        ReceiverControlMessage::FileResult(result) if result.file_id == file_id => result,
        other => bail!("expected file-result for file {} but got {other:?}", file_id),
    };

    if !result.success {
        record_transfer_issue(
            issues,
            failed_files,
            &relative_path,
            format!("receiver rejected file {}: {}", relative_path, result.message),
        );
        return Ok(());
    }

    checkpoint
        .mark_file_complete(relative_path.clone(), file_size, file_sha256)
        .with_context(|| {
            format!(
                "failed to persist sender streaming checkpoint for {}",
                relative_path
            )
        })?;

    if !disposition.needed {
        reporter.set_phase("sending");
        reporter.advance(file_size);
    }

    *completed_files = completed_files.saturating_add(1);
    *completed_bytes = completed_bytes.saturating_add(file_size);
    *completed_chunks = completed_chunks.saturating_add(chunk_count);
    aggregate_hasher.update(&file_sha256);
    reporter.set_completed_files(*completed_files);

    Ok(())
}

async fn send_file_chunks(
    source_path: &Path,
    file_id: u32,
    descriptors: &[ChunkDescriptor],
    parallelism: usize,
    transport: &Arc<QuicSender>,
    reporter: &mut ProgressReporter,
    transfer_control: Option<&TransferControl>,
) -> Result<integrity::Sha256Hash> {
    if descriptors.is_empty() {
        return Ok([0_u8; 32]);
    }

    let concurrency = parallelism.max(1).min(descriptors.len().max(1));
    let mut file_hasher = Sha256State::new();
    let semaphore = Arc::new(Semaphore::new(concurrency));
    let mut join_set = JoinSet::new();

    let mut file = File::open(source_path)
        .await
        .with_context(|| format!("failed to open source file {}", source_path.display()))?;
    let mut current_offset = 0_u64;

    for descriptor in descriptors {
        wait_for_transfer_control(transfer_control).await?;
        let descriptor = descriptor.clone();
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .context("failed to acquire sender chunk scheduling permit")?;

        if descriptor.offset != current_offset {
            file.seek(SeekFrom::Start(descriptor.offset))
                .await
                .with_context(|| {
                    format!(
                        "failed to seek source file {} to offset {}",
                        source_path.display(),
                        descriptor.offset
                    )
                })?;
            current_offset = descriptor.offset;
        }

        let mut payload = vec![0_u8; descriptor.size as usize];
        file.read_exact(&mut payload)
            .await
            .with_context(|| format!("failed to read source chunk {}", descriptor.index))?;
        current_offset = current_offset.saturating_add(u64::from(descriptor.size));
        file_hasher.update(&payload);

        let transport = Arc::clone(transport);
        join_set.spawn(async move {
            let _permit = permit;
            let header = ChunkStreamHeader {
                file_index: file_id,
                descriptor: descriptor.clone(),
                chunk_sha256: sha256_bytes(&payload),
            };
            transport.send_chunk(&header, &payload).await?;
            Ok::<u64, anyhow::Error>(u64::from(descriptor.size))
        });
    }

    while let Some(result) = join_set.join_next().await {
        wait_for_transfer_control(transfer_control).await?;
        let sent_size = result.context("sender chunk task panicked")??;
        reporter.advance(sent_size);
    }

    Ok(file_hasher.finalize())
}

async fn receive_file_chunks(
    incoming: &IncomingConnection,
    destination: &Path,
    file_entry: &StreamFileEntry,
    session: &StreamSessionStart,
    reporter: &mut ProgressReporter,
) -> Result<()> {
    let descriptors = FixedChunker::new(file_entry.file_size, session.chunk_size.max(1)).descriptors();
    if descriptors.len() as u64 != file_entry.chunk_count {
        bail!(
            "file {} advertised {} chunks but receiver planned {}",
            file_entry.relative_path,
            file_entry.chunk_count,
            descriptors.len()
        );
    }

    let destination_fs = filesystem_path(destination);
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&destination_fs)
        .await
        .with_context(|| format!("failed to open destination file {}", destination.display()))?;
    file.set_len(file_entry.file_size)
        .await
        .with_context(|| format!("failed to size destination file {}", destination.display()))?;

    let mut seen_chunks = HashSet::with_capacity(descriptors.len());
    let destination_path = destination.to_path_buf();
    let mut join_set = JoinSet::new();

    for _ in 0..descriptors.len() {
        let chunk = incoming.accept_chunk_stream().await?;
        validate_incoming_chunk(file_entry.file_id, &descriptors, &chunk.header)?;

        if !seen_chunks.insert(chunk.header.descriptor.index) {
            bail!(
                "received duplicate chunk {} for {}",
                chunk.header.descriptor.index,
                file_entry.relative_path
            );
        }

        let header = chunk.header.clone();
        let mut stream = chunk.stream;
        let destination_path = destination_path.clone();
        join_set.spawn(async move {
            write_chunk_stream(destination_path.as_path(), &header, &mut stream).await?;
            Ok::<u32, anyhow::Error>(header.descriptor.size)
        });
    }

    while let Some(result) = join_set.join_next().await {
        let chunk_size = result.context("receiver chunk task panicked")??;
        reporter.set_phase("sending");
        reporter.advance(u64::from(chunk_size));
    }

    if seen_chunks.len() != descriptors.len() {
        bail!(
            "receiver did not observe all chunks for {}",
            file_entry.relative_path
        );
    }

    Ok(())
}

fn validate_incoming_chunk(
    file_id: u32,
    descriptors: &[ChunkDescriptor],
    header: &ChunkStreamHeader,
) -> Result<()> {
    if header.file_index != file_id {
        bail!(
            "chunk {} referenced unexpected file id {} (expected {})",
            header.descriptor.index,
            header.file_index,
            file_id,
        );
    }

    let chunk_index = usize::try_from(header.descriptor.index)
        .context("chunk descriptor index exceeded usize")?;
    let expected = descriptors
        .get(chunk_index)
        .with_context(|| format!("chunk index {} out of range", header.descriptor.index))?;

    if header.descriptor.offset != expected.offset || header.descriptor.size != expected.size {
        bail!(
            "chunk {} descriptor mismatch: expected offset {} size {}, got offset {} size {}",
            header.descriptor.index,
            expected.offset,
            expected.size,
            header.descriptor.offset,
            header.descriptor.size,
        );
    }

    Ok(())
}

async fn write_chunk_stream<R>(
    destination: &Path,
    header: &ChunkStreamHeader,
    reader: &mut R,
) -> Result<()>
where
    R: AsyncRead + Unpin,
{
    let descriptor = &header.descriptor;
    let mut remaining = u64::from(descriptor.size);
    let mut current_offset = descriptor.offset;
    let mut buffer = vec![0_u8; BUFFER_SIZE.min(descriptor.size as usize).max(1)];
    let mut hasher = Sha256State::new();

    let destination_fs = filesystem_path(destination);
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&destination_fs)
        .await
        .with_context(|| format!("failed to open destination file {}", destination.display()))?;

    while remaining > 0 {
        let read_len = remaining.min(buffer.len() as u64) as usize;
        reader
            .read_exact(&mut buffer[..read_len])
            .await
            .with_context(|| format!("failed to read payload for chunk {}", descriptor.index))?;
        hasher.update(&buffer[..read_len]);

        file.seek(SeekFrom::Start(current_offset))
            .await
            .with_context(|| {
                format!("failed to seek destination file for chunk {}", descriptor.index)
            })?;
        file.write_all(&buffer[..read_len])
            .await
            .with_context(|| {
                format!(
                    "failed to write destination bytes for chunk {}",
                    descriptor.index
                )
            })?;

        current_offset += read_len as u64;
        remaining -= read_len as u64;
    }

    let mut trailing = [0_u8; 1];
    let trailing_bytes = reader
        .read(&mut trailing)
        .await
        .with_context(|| {
            format!(
                "failed while checking for trailing bytes in chunk {}",
                descriptor.index
            )
        })?;
    if trailing_bytes != 0 {
        bail!(
            "chunk {} contained trailing data beyond its declared size",
            descriptor.index
        );
    }

    let actual_hash = hasher.finalize();
    if !verify_sha256(&actual_hash, &header.chunk_sha256) {
        bail!(
            "chunk {} failed SHA-256 verification: expected {}, got {}",
            descriptor.index,
            format_sha256(&header.chunk_sha256),
            format_sha256(&actual_hash),
        );
    }

    file.sync_data()
        .await
        .with_context(|| format!("failed to flush destination file {}", destination.display()))?;

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

fn destination_path(
    output_dir: &Path,
    root_kind: PackageItemKind,
    root_name: &str,
    relative_path: &str,
) -> Result<PathBuf> {
    let safe_relative = safe_relative_path(relative_path)?;

    Ok(match root_kind {
        PackageItemKind::Directory => output_dir.join(root_name).join(safe_relative),
        PackageItemKind::File => {
            if relative_path != root_name {
                bail!(
                    "file transfer relative path mismatch: expected {}, got {}",
                    root_name,
                    relative_path
                );
            }
            output_dir.join(root_name)
        }
    })
}

fn package_root_path(output_dir: &Path, root_kind: PackageItemKind, root_name: &str) -> PathBuf {
    match root_kind {
        PackageItemKind::Directory => output_dir.join(root_name),
        PackageItemKind::File => output_dir.join(root_name),
    }
}

fn filesystem_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        if !path.is_absolute() {
            return path.to_path_buf();
        }

        let text = path.as_os_str().to_string_lossy();
        if text.starts_with(r"\\?\") {
            return path.to_path_buf();
        }

        if let Some(stripped) = text.strip_prefix(r"\\") {
            return PathBuf::from(format!(r"\\?\UNC\{}", stripped));
        }

        return PathBuf::from(format!(r"\\?\{}", text));
    }

    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

fn display_current_path(root_name: &str, root_kind: PackageItemKind, relative_path: &str) -> String {
    match root_kind {
        PackageItemKind::Directory => format!("{root_name}/{relative_path}"),
        PackageItemKind::File => relative_path.to_owned(),
    }
}

fn root_name(source_path: &Path) -> Result<String> {
    source_path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .with_context(|| {
            format!(
                "failed to derive a valid root name from {}",
                source_path.display()
            )
        })
}







