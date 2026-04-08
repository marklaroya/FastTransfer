#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::{Context, Result};
use discovery::{default_device_name, discover_receivers, NearbyReceiver};
use rfd::FileDialog;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use transfer_core::{
    bind_receiver, send_file_with_progress, ReceiveRequest, SendRequest, TransferProgress,
    TransferSummary, DEFAULT_CHUNK_SIZE, DEFAULT_PARALLELISM,
};

const DEFAULT_BIND_ADDR: &str = "0.0.0.0:5000";
const DEFAULT_SERVER_NAME: &str = "fasttransfer.local";

#[derive(Default)]
struct DesktopState {
    sender_running: Arc<AtomicBool>,
    receiver_running: Arc<AtomicBool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReceiverListItem {
    device_name: String,
    peer_id: String,
    addresses: Vec<String>,
    transport: String,
    service_name: String,
}

impl From<NearbyReceiver> for ReceiverListItem {
    fn from(value: NearbyReceiver) -> Self {
        Self {
            device_name: value.device_name,
            peer_id: value.peer_id,
            addresses: value.addresses.into_iter().map(|addr| addr.to_string()).collect(),
            transport: value.transport,
            service_name: value.service_name,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProgressPayload {
    label: String,
    total_bytes: u64,
    transferred_bytes: u64,
    percent: f64,
    average_mib_per_sec: f64,
    completed: bool,
}

impl From<TransferProgress> for ProgressPayload {
    fn from(value: TransferProgress) -> Self {
        Self {
            label: value.label,
            total_bytes: value.total_bytes,
            transferred_bytes: value.transferred_bytes,
            percent: value.percent_complete,
            average_mib_per_sec: value.average_mib_per_sec,
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
            sha256_hex: value.sha256_hex,
            integrity_verified: value.integrity_verified,
        }
    }
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
    progress: Option<ProgressPayload>,
    summary: Option<TransferSummaryPayload>,
    saved_path: Option<String>,
    remote_address: Option<String>,
    certificate_path: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReceiverStartPayload {
    bind_addr: String,
    output_dir: String,
    certificate_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendDesktopRequest {
    source_path: String,
    target_addr: String,
    certificate_path: String,
    server_name: Option<String>,
    chunk_size: Option<u32>,
    parallelism: Option<usize>,
}

impl SendDesktopRequest {
    fn into_transfer_request(self) -> Result<SendRequest> {
        Ok(SendRequest {
            server_addr: self
                .target_addr
                .parse::<SocketAddr>()
                .with_context(|| format!("invalid target address: {}", self.target_addr))?,
            source_path: PathBuf::from(self.source_path),
            certificate_path: PathBuf::from(self.certificate_path),
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
fn pick_certificate_file() -> Option<String> {
    FileDialog::new()
        .add_filter("DER certificate", &["der"])
        .set_title("Choose receiver certificate")
        .pick_file()
        .map(|path| path.display().to_string())
}

#[tauri::command]
async fn discover_nearby_receivers(timeout_secs: Option<u64>) -> Result<Vec<ReceiverListItem>, String> {
    let timeout = Duration::from_secs(timeout_secs.unwrap_or(3).max(1));
    discover_receivers(timeout)
        .map(|receivers| receivers.into_iter().map(ReceiverListItem::from).collect())
        .map_err(format_error)
}

#[tauri::command]
async fn start_receiver(
    app: AppHandle,
    state: State<'_, DesktopState>,
    device_name: Option<String>,
) -> Result<ReceiverStartPayload, String> {
    let running = Arc::clone(&state.receiver_running);
    if running.swap(true, Ordering::SeqCst) {
        return Err("The receiver is already running.".to_owned());
    }

    let bind_addr = DEFAULT_BIND_ADDR
        .parse::<SocketAddr>()
        .map_err(|error| format!("invalid default bind address: {error}"))?;
    let runtime_dir = desktop_runtime_dir().map_err(format_error)?;
    let cert_dir = runtime_dir.join("certs");
    let output_dir = runtime_dir.join("received");
    std::fs::create_dir_all(&cert_dir).map_err(format_error)?;
    std::fs::create_dir_all(&output_dir).map_err(format_error)?;

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
    let app_handle = app.clone();
    let running_for_task = Arc::clone(&running);
    let request_for_task = request.clone();
    let advertised_name_for_task = advertised_name.clone();
    let output_dir_for_task = output_dir.clone();
    let certificate_path_for_task = certificate_path.clone();

    tauri::async_runtime::spawn(async move {
        let result = async {
            emit_receiver_status(
                &app_handle,
                ReceiverStatusPayload {
                    state: "starting".to_owned(),
                    message: format!("Preparing receiver for {advertised_name_for_task}..."),
                    bind_addr: Some(bind_addr.to_string()),
                    output_dir: Some(output_dir_for_task.display().to_string()),
                    progress: None,
                    summary: None,
                    saved_path: None,
                    remote_address: None,
                    certificate_path: Some(request_for_task.cert_path.display().to_string()),
                },
            );

            let receiver = bind_receiver(request_for_task.clone())?;
            let ready = receiver.ready();
            emit_receiver_status(
                &app_handle,
                ReceiverStatusPayload {
                    state: "listening".to_owned(),
                    message: format!(
                        "Listening on {} and advertising as {}.",
                        ready.bind_addr, advertised_name_for_task
                    ),
                    bind_addr: Some(ready.bind_addr.to_string()),
                    output_dir: Some(output_dir_for_task.display().to_string()),
                    progress: None,
                    summary: None,
                    saved_path: None,
                    remote_address: None,
                    certificate_path: Some(ready.cert_path.display().to_string()),
                },
            );

            let app_for_progress = app_handle.clone();
            let cert_path_for_progress = request_for_task.cert_path.display().to_string();
            let bind_addr_for_progress = ready.bind_addr.to_string();
            let output_dir_for_progress = output_dir_for_task.display().to_string();
            let received = receiver
                .receive_once_with_progress(move |progress| {
                    emit_receiver_status(
                        &app_for_progress,
                        ReceiverStatusPayload {
                            state: "receiving".to_owned(),
                            message: progress.label.clone(),
                            bind_addr: Some(bind_addr_for_progress.clone()),
                            output_dir: Some(output_dir_for_progress.clone()),
                            progress: Some(progress.into()),
                            summary: None,
                            saved_path: None,
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
                    progress: None,
                    summary: Some(received.summary.into()),
                    saved_path: Some(received.saved_path.display().to_string()),
                    remote_address: Some(received.remote_address.to_string()),
                    certificate_path: Some(ready.cert_path.display().to_string()),
                },
            );

            Ok::<(), anyhow::Error>(())
        }
        .await;

        if let Err(error) = result {
            emit_receiver_status(
                &app_handle,
                ReceiverStatusPayload {
                    state: "error".to_owned(),
                    message: format!("Receiver failed: {error:#}"),
                    bind_addr: Some(bind_addr.to_string()),
                    output_dir: Some(output_dir_for_task.display().to_string()),
                    progress: None,
                    summary: None,
                    saved_path: None,
                    remote_address: None,
                    certificate_path: Some(certificate_path_for_task.clone()),
                },
            );
        }

        running_for_task.store(false, Ordering::SeqCst);
    });

    Ok(ReceiverStartPayload {
        bind_addr: bind_addr.to_string(),
        output_dir: output_dir.display().to_string(),
        certificate_path: certificate_path,
    })
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

    let send_request = request.into_transfer_request().map_err(format_error)?;
    let app_handle = app.clone();
    let running_for_task = Arc::clone(&running);
    let source_path = send_request.source_path.display().to_string();
    let target_addr = send_request.server_addr.to_string();

    tauri::async_runtime::spawn(async move {
        emit_send_status(
            &app_handle,
            SendStatusPayload {
                state: "starting".to_owned(),
                message: format!("Connecting to {target_addr}..."),
                progress: None,
                summary: None,
            },
        );

        let app_for_progress = app_handle.clone();
        let result = send_file_with_progress(send_request, move |progress| {
            emit_send_status(
                &app_for_progress,
                SendStatusPayload {
                    state: "sending".to_owned(),
                    message: progress.label.clone(),
                    progress: Some(progress.into()),
                    summary: None,
                },
            );
        })
        .await;

        match result {
            Ok(summary) => emit_send_status(
                &app_handle,
                SendStatusPayload {
                    state: "completed".to_owned(),
                    message: format!("Transfer to {target_addr} completed successfully."),
                    progress: None,
                    summary: Some(summary.into()),
                },
            ),
            Err(error) => emit_send_status(
                &app_handle,
                SendStatusPayload {
                    state: "error".to_owned(),
                    message: format!("Failed to send {source_path}: {error:#}"),
                    progress: None,
                    summary: None,
                },
            ),
        }

        running_for_task.store(false, Ordering::SeqCst);
    });

    Ok(())
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

fn format_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn main() {
    tauri::Builder::default()
        .manage(DesktopState::default())
        .invoke_handler(tauri::generate_handler![
            pick_source_file,
            pick_certificate_file,
            discover_nearby_receivers,
            start_receiver,
            start_send
        ])
        .run(tauri::generate_context!())
        .expect("error while running FastTransfer desktop");
}


