#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
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
    recommend_transfer_tuning,
    send_file_with_progress, DiscoveredSendRequest, ReceiveRequest, ReceiverTrustReport,
    ReceiverTrustState, SendRequest, TransferProgress, TransferSummary, DEFAULT_CHUNK_SIZE,
    DEFAULT_PARALLELISM,
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
    let result = send_file_with_progress(send_request, move |progress| {
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

fn main() {
    tauri::Builder::default()
        .manage(DesktopState::default())
        .invoke_handler(tauri::generate_handler![
            pick_source_file,
            pick_source_folder,
            inspect_source,
            pick_certificate_file,
            pick_receive_folder,
            discover_nearby_receivers,
            start_receiver,
            stop_receiver,
            start_send
        ])
        .run(tauri::generate_context!())
        .expect("error while running FastTransfer desktop");
}
