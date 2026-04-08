#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use anyhow::{Context, Result};
use discovery::{advertise_receiver, default_device_name, discover_receivers, NearbyReceiver};
use rfd::FileDialog;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::oneshot;
use transfer_core::{
    assess_discovered_receiver, bind_receiver, ensure_preferred_receive_dir,
    inspect_transfer_source, prepare_discovered_send_request, preferred_receive_dir_label,
    recommend_transfer_tuning, run_destination_sink_with_progress,
    DestinationSink, DiscoveredSendRequest, LanSink, LocalCopyRequest, LocalDestinationKind,
    LocalFolderSink, ReceiveRequest, ReceiverTrustReport, ReceiverTrustState, SendRequest,
    TransferProgress, TransferSummary, UsbDriveSink, DEFAULT_CHUNK_SIZE, DEFAULT_PARALLELISM,
};

const DEFAULT_BIND_ADDR: &str = "0.0.0.0:5000";
const DEFAULT_SERVER_NAME: &str = "fasttransfer.local";

#[derive(Default)]
struct DesktopState {
    sender_running: Arc<AtomicBool>,
    receiver_running: Arc<AtomicBool>,
    receiver_stop: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    discovered_receivers: Mutex<HashMap<String, NearbyReceiver>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReceiverListItem {
    device_name: String,
    peer_id: String,
    addresses: Vec<String>,
    transport: String,
    service_name: String,
    short_fingerprint: String,
    trust_state: String,
    trust_message: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProgressPayload {
    label: String,
    phase: String,
    total_bytes: u64,
    transferred_bytes: u64,
    percent: f64,
    average_mib_per_sec: f64,
    completed_files: u64,
    total_files: u64,
    current_path: Option<String>,
    completed: bool,
}

impl From<TransferProgress> for ProgressPayload {
    fn from(value: TransferProgress) -> Self {
        Self {
            label: value.label,
            phase: value.phase,
            total_bytes: value.total_bytes,
            transferred_bytes: value.transferred_bytes,
            percent: value.percent_complete,
            average_mib_per_sec: value.average_mib_per_sec,
            completed_files: value.completed_files,
            total_files: value.total_files,
            current_path: value.current_path,
            completed: value.completed,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TransferSummaryPayload {
    file_name: String,
    bytes_transferred: u64,
    elapsed_secs: f64,
    average_mib_per_sec: f64,
    completed_chunks: u64,
    completed_files: u64,
    total_directories: u64,
    sha256_hex: String,
    integrity_verified: bool,
}

impl From<TransferSummary> for TransferSummaryPayload {
    fn from(value: TransferSummary) -> Self {
        Self {
            file_name: value.file_name,
            bytes_transferred: value.bytes_transferred,
            elapsed_secs: value.elapsed.as_secs_f64(),
            average_mib_per_sec: value.average_mib_per_sec,
            completed_chunks: value.completed_chunks,
            completed_files: value.completed_files,
            total_directories: value.total_directories,
            sha256_hex: value.sha256_hex,
            integrity_verified: value.integrity_verified,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PackageSummaryPayload {
    root_name: String,
    root_kind: String,
    total_files: u64,
    total_directories: u64,
    total_bytes: u64,
    recommended_chunk_size: u32,
    recommended_parallelism: usize,
    tuning_profile: String,
    tuning_reason: String,
}
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SendStatusPayload {
    state: String,
    message: String,
    progress: Option<ProgressPayload>,
    summary: Option<TransferSummaryPayload>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ReceiverStatusPayload {
    state: String,
    message: String,
    bind_addr: Option<String>,
    output_dir: Option<String>,
    output_dir_label: Option<String>,
    progress: Option<ProgressPayload>,
    summary: Option<TransferSummaryPayload>,
    saved_path: Option<String>,
    saved_path_label: Option<String>,
    remote_address: Option<String>,
    certificate_path: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReceiverStartPayload {
    bind_addr: String,
    output_dir: String,
    output_dir_label: String,
    certificate_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendDesktopRequest {
    source_path: String,
    selected_peer_id: Option<String>,
    target_addr: Option<String>,
    certificate_path: Option<String>,
    server_name: Option<String>,
    chunk_size: Option<u32>,
    parallelism: Option<usize>,
}

impl SendDesktopRequest {
    fn into_manual_transfer_request(self) -> Result<SendRequest> {
        let target_addr = self
            .target_addr
            .filter(|value| !value.trim().is_empty())
            .context("manual target mode requires a receiver address")?;
        let certificate_path = self
            .certificate_path
            .filter(|value| !value.trim().is_empty())
            .context("manual target mode requires a receiver certificate")?;

        Ok(SendRequest {
            server_addr: target_addr
                .parse::<SocketAddr>()
                .with_context(|| format!("invalid target address: {target_addr}"))?,
            source_path: PathBuf::from(self.source_path),
            certificate_path: PathBuf::from(certificate_path),
            server_name: self.server_name.unwrap_or_else(|| DEFAULT_SERVER_NAME.to_owned()),
            chunk_size: self.chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE).max(1),
            parallelism: self.parallelism.unwrap_or(DEFAULT_PARALLELISM).max(1),
        })
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RemovableDrivePayload {
    id: String,
    drive_letter: String,
    label: String,
    mount_path: String,
    file_system: Option<String>,
    free_bytes: u64,
    total_bytes: u64,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum DesktopLocalDestinationKind {
    LocalFolder,
    UsbDrive,
}

impl DesktopLocalDestinationKind {
    fn to_transfer_kind(self) -> LocalDestinationKind {
        match self {
            Self::LocalFolder => LocalDestinationKind::LocalFolder,
            Self::UsbDrive => LocalDestinationKind::UsbDrive,
        }
    }

    fn as_label(self) -> &'static str {
        match self {
            Self::LocalFolder => "local folder",
            Self::UsbDrive => "USB or external drive",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartLocalCopyRequest {
    source_path: String,
    destination_path: String,
    destination_kind: DesktopLocalDestinationKind,
    chunk_size: Option<u32>,
    parallelism: Option<usize>,
}

impl StartLocalCopyRequest {
    fn into_local_copy_request(self) -> LocalCopyRequest {
        LocalCopyRequest {
            source_path: PathBuf::from(self.source_path),
            destination_dir: PathBuf::from(self.destination_path),
            chunk_size: self.chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE).max(1),
            parallelism: self.parallelism.unwrap_or(DEFAULT_PARALLELISM).max(1),
            destination_kind: self.destination_kind.to_transfer_kind(),
        }
    }
}

#[tauri::command]
fn pick_source_file() -> Option<String> {
    FileDialog::new()
        .set_title("Choose a file to send")
        .pick_file()
        .map(|path| path.display().to_string())
}

#[tauri::command]
fn pick_source_folder() -> Option<String> {
    FileDialog::new()
        .set_title("Choose a folder to send")
        .pick_folder()
        .map(|path| path.display().to_string())
}

#[tauri::command]
fn pick_receive_folder() -> Option<String> {
    FileDialog::new()
        .set_title("Choose where received files should be saved")
        .pick_folder()
        .map(|path| path.display().to_string())
}

#[tauri::command]
fn pick_local_destination_folder() -> Option<String> {
    FileDialog::new()
        .set_title("Choose destination folder")
        .pick_folder()
        .map(|path| path.display().to_string())
}

#[tauri::command]
fn list_removable_drives() -> Result<Vec<RemovableDrivePayload>, String> {
    query_removable_drives().map_err(format_error)
}

#[tauri::command]
async fn inspect_source(source_path: String) -> Result<PackageSummaryPayload, String> {
    let source_path = PathBuf::from(source_path);
    let summary = inspect_transfer_source(source_path.as_path())
        .await
        .map_err(format_error)?;
    let tuning = recommend_transfer_tuning(&summary);
    Ok(PackageSummaryPayload {
        root_name: summary.root_name,
        root_kind: format!("{:?}", summary.root_kind).to_lowercase(),
        total_files: summary.total_files,
        total_directories: summary.total_directories,
        total_bytes: summary.total_bytes,
        recommended_chunk_size: tuning.chunk_size,
        recommended_parallelism: tuning.parallelism,
        tuning_profile: tuning.profile.as_str().to_owned(),
        tuning_reason: tuning.reason,
    })
}

#[tauri::command]
fn pick_certificate_file() -> Option<String> {
    FileDialog::new()
        .add_filter("DER certificate", &["der"])
        .set_title("Choose receiver certificate")
        .pick_file()
        .map(|path| path.display().to_string())
}

#[tauri::command]
async fn discover_nearby_receivers(
    state: State<'_, DesktopState>,
    timeout_secs: Option<u64>,
) -> Result<Vec<ReceiverListItem>, String> {
    let timeout = Duration::from_secs(timeout_secs.unwrap_or(3).max(1));
    let trust_dir = trust_cache_dir().map_err(format_error)?;
    let receivers = discover_receivers(timeout).map_err(format_error)?;

    let mut cache = state
        .discovered_receivers
        .lock()
        .map_err(|_| "failed to access the discovered receiver cache".to_owned())?;
    cache.clear();

    let mut items = Vec::with_capacity(receivers.len());
    for receiver in receivers {
        let trust = assess_discovered_receiver(&receiver, &trust_dir).map_err(format_error)?;
        cache.insert(receiver.peer_id.clone(), receiver.clone());
        items.push(ReceiverListItem {
            device_name: receiver.device_name,
            peer_id: receiver.peer_id,
            addresses: receiver.addresses.into_iter().map(|addr| addr.to_string()).collect(),
            transport: receiver.transport,
            service_name: receiver.service_name,
            short_fingerprint: trust.short_fingerprint,
            trust_state: trust.state.as_str().to_owned(),
            trust_message: trust.message,
        });
    }

    Ok(items)
}

#[tauri::command]
async fn start_receiver(
    app: AppHandle,
    state: State<'_, DesktopState>,
    device_name: Option<String>,
    output_dir: Option<String>,
) -> Result<ReceiverStartPayload, String> {
    let running = Arc::clone(&state.receiver_running);
    let stop_handle = Arc::clone(&state.receiver_stop);
    if running.swap(true, Ordering::SeqCst) {
        return Err("The receiver is already running.".to_owned());
    }

    let bind_addr = DEFAULT_BIND_ADDR
        .parse::<SocketAddr>()
        .map_err(|error| format!("invalid default bind address: {error}"))?;
    let runtime_dir = desktop_runtime_dir().map_err(format_error)?;
    let cert_dir = runtime_dir.join("certs");
    let output_dir = match output_dir.filter(|value| !value.trim().is_empty()) {
        Some(path) => PathBuf::from(path),
        None => ensure_preferred_receive_dir().map_err(format_error)?,
    };
    let output_dir_label = preferred_receive_dir_label(&output_dir);
    std::fs::create_dir_all(&cert_dir).map_err(format_error)?;

    let request = ReceiveRequest {
        bind_addr,
        output_dir: output_dir.clone(),
        cert_path: cert_dir.join("receiver-cert.der"),
        key_path: cert_dir.join("receiver-key.der"),
    };
    let certificate_path = request.cert_path.display().to_string();
    let advertised_name = device_name
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(default_device_name);
    let (stop_tx, mut stop_rx) = oneshot::channel();
    {
        let mut stop_slot = stop_handle
            .lock()
            .map_err(|_| "failed to prepare the receiver stop handle".to_owned())?;
        *stop_slot = Some(stop_tx);
    }

    let app_handle = app.clone();
    let running_for_task = Arc::clone(&running);
    let stop_handle_for_task = Arc::clone(&stop_handle);
    let request_for_task = request.clone();
    let advertised_name_for_task = advertised_name.clone();
    let output_dir_for_task = output_dir.clone();
    let output_dir_label_for_task = output_dir_label.clone();
    let certificate_path_for_task = certificate_path.clone();

    tauri::async_runtime::spawn(async move {
        let receiver_task = async {
            emit_receiver_status(
                &app_handle,
                ReceiverStatusPayload {
                    state: "starting".to_owned(),
                    message: format!("Preparing receiver for {advertised_name_for_task}..."),
                    bind_addr: Some(bind_addr.to_string()),
                    output_dir: Some(output_dir_for_task.display().to_string()),
                    output_dir_label: Some(output_dir_label_for_task.clone()),
                    progress: None,
                    summary: None,
                    saved_path: None,
                    saved_path_label: None,
                    remote_address: None,
                    certificate_path: Some(certificate_path_for_task.clone()),
                },
            );

            let receiver = bind_receiver(request_for_task.clone())?;
            let ready = receiver.ready();
            let _advertiser = advertise_receiver(
                ready.bind_addr,
                advertised_name_for_task.clone(),
                ready.cert_path.as_path(),
            )?;
            emit_receiver_status(
                &app_handle,
                ReceiverStatusPayload {
                    state: "listening".to_owned(),
                    message: format!(
                        "Listening on {} and advertising as {} with automatic LAN trust. Incoming files go to {}.",
                        ready.bind_addr, advertised_name_for_task, output_dir_label_for_task
                    ),
                    bind_addr: Some(ready.bind_addr.to_string()),
                    output_dir: Some(output_dir_for_task.display().to_string()),
                    output_dir_label: Some(output_dir_label_for_task.clone()),
                    progress: None,
                    summary: None,
                    saved_path: None,
                    saved_path_label: None,
                    remote_address: None,
                    certificate_path: Some(ready.cert_path.display().to_string()),
                },
            );

            let app_for_progress = app_handle.clone();
            let cert_path_for_progress = request_for_task.cert_path.display().to_string();
            let bind_addr_for_progress = ready.bind_addr.to_string();
            let output_dir_for_progress = output_dir_for_task.display().to_string();
            let output_dir_label_for_progress = output_dir_label_for_task.clone();
            let received = receiver
                .receive_once_with_progress(move |progress| {
                    emit_receiver_status(
                        &app_for_progress,
                        ReceiverStatusPayload {
                            state: "receiving".to_owned(),
                            message: progress.label.clone(),
                            bind_addr: Some(bind_addr_for_progress.clone()),
                            output_dir: Some(output_dir_for_progress.clone()),
                            output_dir_label: Some(output_dir_label_for_progress.clone()),
                            progress: Some(progress.into()),
                            summary: None,
                            saved_path: None,
                            saved_path_label: None,
                            remote_address: None,
                            certificate_path: Some(cert_path_for_progress.clone()),
                        },
                    );
                })
                .await?;

            emit_receiver_status(
                &app_handle,
                ReceiverStatusPayload {
                    state: "completed".to_owned(),
                    message: format!("Saved {} from {}.", received.summary.file_name, received.remote_address),
                    bind_addr: Some(ready.bind_addr.to_string()),
                    output_dir: Some(output_dir_for_task.display().to_string()),
                    output_dir_label: Some(output_dir_label_for_task.clone()),
                    progress: None,
                    summary: Some(received.summary.into()),
                    saved_path: Some(received.saved_path.display().to_string()),
                    saved_path_label: Some(preferred_receive_dir_label(&received.saved_path)),
                    remote_address: Some(received.remote_address.to_string()),
                    certificate_path: Some(ready.cert_path.display().to_string()),
                },
            );

            Ok::<(), anyhow::Error>(())
        };

        let result = tokio::select! {
            _ = &mut stop_rx => {
                emit_receiver_status(
                    &app_handle,
                    ReceiverStatusPayload {
                        state: "stopped".to_owned(),
                        message: format!("Receiver stopped. Files stay in {}.", output_dir_label_for_task),
                        bind_addr: Some(bind_addr.to_string()),
                        output_dir: Some(output_dir_for_task.display().to_string()),
                        output_dir_label: Some(output_dir_label_for_task.clone()),
                        progress: None,
                        summary: None,
                        saved_path: None,
                        saved_path_label: None,
                        remote_address: None,
                        certificate_path: Some(certificate_path_for_task.clone()),
                    },
                );
                Ok::<(), anyhow::Error>(())
            }
            result = receiver_task => result,
        };

        if let Err(error) = result {
            emit_receiver_status(
                &app_handle,
                ReceiverStatusPayload {
                    state: "error".to_owned(),
                    message: format!("Receiver failed: {error:#}"),
                    bind_addr: Some(bind_addr.to_string()),
                    output_dir: Some(output_dir_for_task.display().to_string()),
                    output_dir_label: Some(output_dir_label_for_task.clone()),
                    progress: None,
                    summary: None,
                    saved_path: None,
                    saved_path_label: None,
                    remote_address: None,
                    certificate_path: Some(certificate_path_for_task.clone()),
                },
            );
        }

        if let Ok(mut stop_slot) = stop_handle_for_task.lock() {
            *stop_slot = None;
        }
        running_for_task.store(false, Ordering::SeqCst);
    });

    Ok(ReceiverStartPayload {
        bind_addr: bind_addr.to_string(),
        output_dir: output_dir.display().to_string(),
        output_dir_label,
        certificate_path,
    })
}

#[tauri::command]
fn stop_receiver(state: State<'_, DesktopState>) -> Result<(), String> {
    let sender = state
        .receiver_stop
        .lock()
        .map_err(|_| "failed to access the receiver stop handle".to_owned())?
        .take();

    match sender {
        Some(sender) => {
            let _ = sender.send(());
            Ok(())
        }
        None if state.receiver_running.load(Ordering::SeqCst) => {
            Err("Receiver shutdown is already in progress.".to_owned())
        }
        None => Err("The receiver is not running.".to_owned()),
    }
}

#[tauri::command]
async fn start_local_copy_transfer(
    app: AppHandle,
    state: State<'_, DesktopState>,
    request: StartLocalCopyRequest,
) -> Result<(), String> {
    let running = Arc::clone(&state.sender_running);
    if running.swap(true, Ordering::SeqCst) {
        return Err("A send operation is already in progress.".to_owned());
    }

    let source_path = PathBuf::from(request.source_path.clone());
    let destination_path = PathBuf::from(request.destination_path.clone());
    if request.destination_path.trim().is_empty() {
        running.store(false, Ordering::SeqCst);
        return Err("Choose a destination folder or drive before sending.".to_owned());
    }

    let destination_kind = request.destination_kind;
    let summary = match inspect_transfer_source(source_path.as_path()).await {
        Ok(summary) => summary,
        Err(error) => {
            running.store(false, Ordering::SeqCst);
            return Err(format_error(error));
        }
    };

    let volume_info = match resolve_volume_for_path(destination_path.as_path()) {
        Ok(volume) => volume,
        Err(error) => {
            running.store(false, Ordering::SeqCst);
            return Err(format_error(error));
        }
    };

    if destination_kind == DesktopLocalDestinationKind::UsbDrive {
        if let Some(info) = &volume_info {
            if info.drive_type != Some(2) {
                running.store(false, Ordering::SeqCst);
                return Err(format!(
                    "Destination {} is not reported as a removable drive.",
                    info.mount_path
                ));
            }
        }
    }

    if let Some(info) = &volume_info {
        if info.free_bytes < summary.total_bytes {
            running.store(false, Ordering::SeqCst);
            return Err(format!(
                "Insufficient free space on {}: free {}, required {}.",
                info.mount_path,
                format_bytes(info.free_bytes),
                format_bytes(summary.total_bytes)
            ));
        }

        if info
            .file_system
            .as_deref()
            .is_some_and(|fs| fs.eq_ignore_ascii_case("FAT32"))
        {
            const FAT32_LIMIT: u64 = (4 * 1024 * 1024 * 1024) - 1;
            match tokio::task::spawn_blocking({
                let source_path = source_path.clone();
                move || find_first_file_exceeding(source_path.as_path(), FAT32_LIMIT)
            })
            .await
            {
                Ok(Ok(Some((path, size)))) => {
                    running.store(false, Ordering::SeqCst);
                    return Err(format!(
                        "FAT32 destination {} cannot store {} ({}). Files must be smaller than 4 GiB.",
                        info.mount_path,
                        path.display(),
                        format_bytes(size)
                    ));
                }
                Ok(Ok(None)) => {}
                Ok(Err(error)) => {
                    running.store(false, Ordering::SeqCst);
                    return Err(format_error(error));
                }
                Err(join_error) => {
                    running.store(false, Ordering::SeqCst);
                    return Err(format!("failed to validate FAT32 constraints: {join_error}"));
                }
            }
        }
    }

    println!(
        "[DEST] Selected destination: {} ({})",
        destination_path.display(),
        destination_kind.as_label()
    );

    // Route the same transfer pipeline through a destination sink so LAN and local targets
    // share progress reporting and orchestration behavior.
    let local_request = request.into_local_copy_request();
    let sink = match destination_kind {
        DesktopLocalDestinationKind::LocalFolder => {
            DestinationSink::LocalFolder(LocalFolderSink { request: local_request })
        }
        DesktopLocalDestinationKind::UsbDrive => {
            DestinationSink::UsbDrive(UsbDriveSink { request: local_request })
        }
    };

    let app_handle = app.clone();
    let running_for_task = Arc::clone(&running);
    let destination_label = compact_path_label(destination_path.as_path());
    let source_display = source_path.display().to_string();

    tauri::async_runtime::spawn(async move {
        run_local_copy_task(
            &app_handle,
            sink,
            &source_display,
            &destination_label,
            destination_kind,
            running_for_task,
        )
        .await;
    });

    Ok(())
}

#[tauri::command]
async fn start_send(
    app: AppHandle,
    state: State<'_, DesktopState>,
    request: SendDesktopRequest,
) -> Result<(), String> {
    let running = Arc::clone(&state.sender_running);
    if running.swap(true, Ordering::SeqCst) {
        return Err("A send operation is already in progress.".to_owned());
    }

    let app_handle = app.clone();
    let running_for_task = Arc::clone(&running);
    let source_path = request.source_path.clone();
    let selected_peer_id = request.selected_peer_id.clone().filter(|value| !value.trim().is_empty());

    let prepared = if let Some(peer_id) = selected_peer_id {
        let receiver = state
            .discovered_receivers
            .lock()
            .map_err(|_| "failed to access the discovered receiver cache".to_owned())?
            .get(&peer_id)
            .cloned()
            .with_context(|| format!("receiver {peer_id} is no longer in the discovery cache; refresh nearby devices"))
            .map_err(format_error)?;
        let trust_dir = trust_cache_dir().map_err(format_error)?;
        let (send_request, trust_report) = prepare_discovered_send_request(DiscoveredSendRequest {
            receiver,
            source_path: PathBuf::from(&request.source_path),
            trust_cache_dir: trust_dir,
            server_addr: None,
            server_name: request.server_name.clone().unwrap_or_else(|| DEFAULT_SERVER_NAME.to_owned()),
            chunk_size: request.chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE).max(1),
            parallelism: request.parallelism.unwrap_or(DEFAULT_PARALLELISM).max(1),
        })
        .map_err(format_error)?;
        PreparedSend::Discovered { send_request, trust_report }
    } else {
        PreparedSend::Manual {
            send_request: request.into_manual_transfer_request().map_err(format_error)?,
        }
    };

    tauri::async_runtime::spawn(async move {
        match prepared {
            PreparedSend::Discovered { send_request, trust_report } => {
                run_send_task(
                    &app_handle,
                    send_request,
                    &source_path,
                    Arc::clone(&running_for_task),
                    Some(trust_report),
                )
                .await;
            }
            PreparedSend::Manual { send_request } => {
                run_send_task(
                    &app_handle,
                    send_request,
                    &source_path,
                    Arc::clone(&running_for_task),
                    None,
                )
                .await;
            }
        }
    });

    Ok(())
}

enum PreparedSend {
    Discovered {
        send_request: SendRequest,
        trust_report: ReceiverTrustReport,
    },
    Manual {
        send_request: SendRequest,
    },
}

async fn run_send_task(
    app_handle: &AppHandle,
    send_request: SendRequest,
    source_path: &str,
    running: Arc<AtomicBool>,
    trust_report: Option<ReceiverTrustReport>,
) {
    let target_addr = send_request.server_addr.to_string();
    emit_send_status(
        app_handle,
        SendStatusPayload {
            state: "starting".to_owned(),
            message: send_start_message(&target_addr, trust_report.as_ref()),
            progress: None,
            summary: None,
        },
    );

    let app_for_progress = app_handle.clone();
    let progress_report = trust_report.clone();
    let sink = DestinationSink::Lan(LanSink { request: send_request });
    let result = run_destination_sink_with_progress(sink, move |progress| {
        let progress_payload: ProgressPayload = progress.into();
        let state = if progress_payload.phase == "scanning" {
            "scanning"
        } else {
            "sending"
        };

        emit_send_status(
            &app_for_progress,
            SendStatusPayload {
                state: state.to_owned(),
                message: send_progress_message(progress_report.as_ref(), &progress_payload.phase),
                progress: Some(progress_payload),
                summary: None,
            },
        );
    })
    .await;

    match result {
        Ok(summary) => emit_send_status(
            app_handle,
            SendStatusPayload {
                state: "completed".to_owned(),
                message: send_success_message(&target_addr, trust_report.as_ref()),
                progress: None,
                summary: Some(summary.into()),
            },
        ),
        Err(error) => emit_send_status(
            app_handle,
            SendStatusPayload {
                state: "error".to_owned(),
                message: format!("Failed to send {source_path}: {error:#}"),
                progress: None,
                summary: None,
            },
        ),
    }

    running.store(false, Ordering::SeqCst);
}

async fn run_local_copy_task(
    app_handle: &AppHandle,
    sink: DestinationSink,
    source_path: &str,
    destination_label: &str,
    destination_kind: DesktopLocalDestinationKind,
    running: Arc<AtomicBool>,
) {
    emit_send_status(
        app_handle,
        SendStatusPayload {
            state: "starting".to_owned(),
            message: format!(
                "Preparing local copy to {} ({})...",
                destination_label,
                destination_kind.as_label()
            ),
            progress: None,
            summary: None,
        },
    );

    let app_for_progress = app_handle.clone();
    let destination_label_for_progress = destination_label.to_owned();
    let kind_label = destination_kind.as_label().to_owned();
    let result = run_destination_sink_with_progress(sink, move |progress| {
        let progress_payload: ProgressPayload = progress.into();
        let state = if progress_payload.phase == "scanning" {
            "scanning"
        } else {
            "sending"
        };

        emit_send_status(
            &app_for_progress,
            SendStatusPayload {
                state: state.to_owned(),
                message: format!(
                    "{} to {} ({}).",
                    if state == "scanning" {
                        "Scanning package"
                    } else {
                        "Copying"
                    },
                    destination_label_for_progress,
                    kind_label
                ),
                progress: Some(progress_payload),
                summary: None,
            },
        );
    })
    .await;

    match result {
        Ok(summary) => emit_send_status(
            app_handle,
            SendStatusPayload {
                state: "completed".to_owned(),
                message: format!("Copy to {} completed successfully.", destination_label),
                progress: None,
                summary: Some(summary.into()),
            },
        ),
        Err(error) => emit_send_status(
            app_handle,
            SendStatusPayload {
                state: "error".to_owned(),
                message: format!("Failed to copy {source_path} to {destination_label}: {error:#}"),
                progress: None,
                summary: None,
            },
        ),
    }

    running.store(false, Ordering::SeqCst);
}

fn send_start_message(target_addr: &str, trust_report: Option<&ReceiverTrustReport>) -> String {
    match trust_report {
        Some(report) => format!(
            "Connecting to {} ({}) at {}. {}",
            report.device_name, report.short_fingerprint, target_addr, report.message
        ),
        None => format!("Connecting to {target_addr} using the manual certificate path..."),
    }
}

fn send_progress_message(trust_report: Option<&ReceiverTrustReport>, phase: &str) -> String {
    let action = if phase == "scanning" {
        "Scanning and preparing files"
    } else {
        "Sending"
    };

    match trust_report {
        Some(report) => format!(
            "{} to {} ({}) with {}.",
            action,
            report.device_name,
            report.short_fingerprint,
            trust_state_label(report.state)
        ),
        None => format!("{} via manual target mode.", action),
    }
}

fn send_success_message(target_addr: &str, trust_report: Option<&ReceiverTrustReport>) -> String {
    match trust_report {
        Some(report) => format!(
            "Transfer to {} ({}) at {} completed successfully.",
            report.device_name, report.short_fingerprint, target_addr
        ),
        None => format!("Transfer to {target_addr} completed successfully."),
    }
}

fn trust_state_label(state: ReceiverTrustState) -> &'static str {
    match state {
        ReceiverTrustState::TrustedForSession => "trust-on-first-use for this session",
        ReceiverTrustState::KnownDevice => "a cached trusted identity",
        ReceiverTrustState::FingerprintMismatch => "a fingerprint mismatch",
    }
}

fn emit_send_status(app: &AppHandle, payload: SendStatusPayload) {
    let _ = app.emit("send-status", payload);
}

fn emit_receiver_status(app: &AppHandle, payload: ReceiverStatusPayload) {
    let _ = app.emit("receiver-status", payload);
}

fn desktop_runtime_dir() -> Result<PathBuf> {
    let current_dir = std::env::current_dir().context("failed to resolve the current working directory")?;
    Ok(current_dir.join(".fasttransfer-desktop"))
}

fn trust_cache_dir() -> Result<PathBuf> {
    Ok(desktop_runtime_dir()?.join("trust"))
}

fn format_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[derive(Debug, Clone)]
struct LogicalDiskInfo {
    device_id: String,
    volume_name: Option<String>,
    file_system: Option<String>,
    total_bytes: u64,
    free_bytes: u64,
    drive_type: Option<u32>,
}

#[derive(Debug, Clone)]
struct VolumeInfo {
    mount_path: String,
    file_system: Option<String>,
    free_bytes: u64,
    drive_type: Option<u32>,
}

fn query_removable_drives() -> Result<Vec<RemovableDrivePayload>> {
    let mut drives = query_logical_disks()?
        .into_iter()
        .filter(|disk| disk.drive_type == Some(2))
        .map(|disk| {
            let mount_path = format!("{}\\", disk.device_id.trim_end_matches('\\'));
            let label = disk
                .volume_name
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| "Removable drive".to_owned());

            RemovableDrivePayload {
                id: disk.device_id.clone(),
                drive_letter: disk.device_id,
                label,
                mount_path,
                file_system: disk.file_system,
                free_bytes: disk.free_bytes,
                total_bytes: disk.total_bytes,
            }
        })
        .collect::<Vec<_>>();

    drives.sort_by(|left, right| left.drive_letter.cmp(&right.drive_letter));
    Ok(drives)
}

fn resolve_volume_for_path(path: &Path) -> Result<Option<VolumeInfo>> {
    #[cfg(target_os = "windows")]
    {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .context("failed to read current directory while resolving destination")?
                .join(path)
        };

        let Some(drive_id) = drive_id_from_path(absolute.as_path()) else {
            return Ok(None);
        };

        let disk = query_logical_disks()?
            .into_iter()
            .find(|entry| entry.device_id.eq_ignore_ascii_case(&drive_id));
        Ok(disk.map(|entry| VolumeInfo {
            mount_path: format!("{}\\", entry.device_id.trim_end_matches('\\')),
            file_system: entry.file_system,
            free_bytes: entry.free_bytes,
            drive_type: entry.drive_type,
        }))
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Ok(None)
    }
}

#[cfg(target_os = "windows")]
fn query_logical_disks() -> Result<Vec<LogicalDiskInfo>> {
    let script = "Get-CimInstance Win32_LogicalDisk | Select-Object DeviceID,VolumeName,FileSystem,Size,FreeSpace,DriveType | ConvertTo-Json -Compress";
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .context("failed to query logical disks via PowerShell")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "PowerShell drive query failed with status {}: {}",
            output.status,
            stderr.trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let rows = parse_json_rows(stdout.as_ref())?;

    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let device_id = parse_json_string(&row, "DeviceID")?;
            let normalized_device = device_id.trim().trim_end_matches('\\').to_uppercase();
            if normalized_device.is_empty() {
                return None;
            }

            Some(LogicalDiskInfo {
                device_id: normalized_device,
                volume_name: parse_json_string(&row, "VolumeName"),
                file_system: parse_json_string(&row, "FileSystem"),
                total_bytes: parse_json_u64(&row, "Size").unwrap_or(0),
                free_bytes: parse_json_u64(&row, "FreeSpace").unwrap_or(0),
                drive_type: parse_json_u32(&row, "DriveType"),
            })
        })
        .collect())
}

#[cfg(not(target_os = "windows"))]
fn query_logical_disks() -> Result<Vec<LogicalDiskInfo>> {
    Ok(Vec::new())
}

#[cfg(target_os = "windows")]
fn parse_json_rows(raw: &str) -> Result<Vec<serde_json::Value>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Ok(Vec::new());
    }

    let value: serde_json::Value =
        serde_json::from_str(trimmed).context("failed to parse logical disk JSON payload")?;

    Ok(match value {
        serde_json::Value::Array(items) => items,
        serde_json::Value::Object(_) => vec![value],
        _ => Vec::new(),
    })
}

#[cfg(target_os = "windows")]
fn parse_json_string(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(target_os = "windows")]
fn parse_json_u64(value: &serde_json::Value, key: &str) -> Option<u64> {
    match value.get(key) {
        Some(serde_json::Value::Number(number)) => number.as_u64(),
        Some(serde_json::Value::String(text)) => text.parse::<u64>().ok(),
        _ => None,
    }
}

#[cfg(target_os = "windows")]
fn parse_json_u32(value: &serde_json::Value, key: &str) -> Option<u32> {
    match value.get(key) {
        Some(serde_json::Value::Number(number)) => number.as_u64().and_then(|n| u32::try_from(n).ok()),
        Some(serde_json::Value::String(text)) => text.parse::<u32>().ok(),
        _ => None,
    }
}

fn drive_id_from_path(path: &Path) -> Option<String> {
    let raw = path.to_string_lossy();
    let bytes = raw.as_bytes();
    if bytes.len() < 2 || bytes[1] != b':' {
        return None;
    }

    Some(raw[..2].to_uppercase())
}

fn find_first_file_exceeding(path: &Path, limit: u64) -> Result<Option<(PathBuf, u64)>> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("failed to read metadata for {}", path.display()))?;

    if metadata.is_file() {
        return if metadata.len() > limit {
            Ok(Some((path.to_path_buf(), metadata.len())))
        } else {
            Ok(None)
        };
    }

    if !metadata.is_dir() {
        return Ok(None);
    }

    let mut children = std::fs::read_dir(path)
        .with_context(|| format!("failed to read directory {}", path.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("failed to enumerate directory {}", path.display()))?;
    children.sort_by_key(|entry| entry.file_name());

    for child in children {
        let child_path = child.path();
        if let Some(found) = find_first_file_exceeding(child_path.as_path(), limit)? {
            return Ok(Some(found));
        }
    }

    Ok(None)
}

fn compact_path_label(path: &Path) -> String {
    let normalized = path.display().to_string().replace('\\', "/");
    let lowered = normalized.to_ascii_lowercase();

    if let Some(index) = lowered.find("/downloads/") {
        let start = index + 1;
        if start < normalized.len() {
            return normalized[start..].to_owned();
        }
    }

    if let Some(index) = lowered.find("/desktop/") {
        let start = index + 1;
        if start < normalized.len() {
            return normalized[start..].to_owned();
        }
    }

    normalized
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes == 0 {
        return "0 B".to_owned();
    }

    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

fn main() {
    tauri::Builder::default()
        .manage(DesktopState::default())
        .invoke_handler(tauri::generate_handler![
            pick_source_file,
            pick_source_folder,
            inspect_source,
            pick_certificate_file,
            pick_receive_folder,
            pick_local_destination_folder,
            list_removable_drives,
            discover_nearby_receivers,
            start_receiver,
            stop_receiver,
            start_send,
            start_local_copy_transfer
        ])
        .run(tauri::generate_context!())
        .expect("error while running FastTransfer desktop");
}

