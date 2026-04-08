import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const $ = (selector) => document.querySelector(selector);
const elements = {
  sourcePath: $("#source-path"),
  nearbyDevices: $("#nearby-devices"),
  refreshDevices: $("#refresh-devices"),
  pickSource: $("#pick-source"),
  sendButton: $("#send-button"),
  sendBadge: $("#send-badge"),
  sendMessage: $("#send-message"),
  sendProgressBar: $("#send-progress-bar"),
  sendProgressText: $("#send-progress-text"),
  sendSpeed: $("#send-speed"),
  sendSummaryFile: $("#send-summary-file"),
  sendSummaryBytes: $("#send-summary-bytes"),
  sendSummaryChunks: $("#send-summary-chunks"),
  sendSummaryHash: $("#send-summary-hash"),
  selectedDeviceBadge: $("#selected-device-badge"),
  selectedDeviceMessage: $("#selected-device-message"),
  selectedDeviceName: $("#selected-device-name"),
  selectedDeviceAddress: $("#selected-device-address"),
  selectedDeviceFingerprint: $("#selected-device-fingerprint"),
  selectedDeviceTrust: $("#selected-device-trust"),
  manualMode: $("#manual-mode"),
  targetAddress: $("#target-address"),
  certificatePath: $("#certificate-path"),
  pickCertificate: $("#pick-certificate"),
  serverName: $("#server-name"),
  chunkSize: $("#chunk-size"),
  parallelism: $("#parallelism"),
  deviceName: $("#device-name"),
  startReceiver: $("#start-receiver"),
  receiverBadge: $("#receiver-badge"),
  receiverMessage: $("#receiver-message"),
  receiverProgressBar: $("#receiver-progress-bar"),
  receiverProgressText: $("#receiver-progress-text"),
  receiverSpeed: $("#receiver-speed"),
  receiverBind: $("#receiver-bind"),
  receiverOutput: $("#receiver-output"),
  receiverFile: $("#receiver-file"),
  receiverSavedPath: $("#receiver-saved-path"),
};

const appState = { devices: [], selectedPeerId: null, sendBusy: false, receiverBusy: false };

function formatBytes(bytes) {
  if (!bytes) return "0 B";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let value = bytes;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value.toFixed(index === 0 ? 0 : 2)} ${units[index]}`;
}

function setBadge(node, label, kind) {
  node.textContent = label;
  node.className = `badge ${kind}`;
}

function badgeKindForTrust(trustState) {
  if (trustState === "known_device") return "success";
  if (trustState === "trusted_for_session") return "active";
  if (trustState === "fingerprint_mismatch") return "error";
  return "idle";
}

function labelForTrust(trustState) {
  if (trustState === "known_device") return "Known device";
  if (trustState === "trusted_for_session") return "Trusted this session";
  if (trustState === "fingerprint_mismatch") return "Fingerprint changed";
  return "Unknown";
}

function selectedDevice() {
  return appState.devices.find((device) => device.peerId === appState.selectedPeerId) ?? null;
}

function renderSelectedDevice() {
  const device = selectedDevice();
  if (!device) {
    setBadge(elements.selectedDeviceBadge, "None", "idle");
    elements.selectedDeviceMessage.textContent = "Refresh nearby devices and choose a receiver for automatic LAN trust.";
    elements.selectedDeviceName.textContent = "No receiver selected";
    elements.selectedDeviceAddress.textContent = "Pending";
    elements.selectedDeviceFingerprint.textContent = "Pending";
    elements.selectedDeviceTrust.textContent = "Pending";
    return;
  }

  setBadge(elements.selectedDeviceBadge, labelForTrust(device.trustState), badgeKindForTrust(device.trustState));
  elements.selectedDeviceMessage.textContent = device.trustMessage;
  elements.selectedDeviceName.textContent = device.deviceName;
  elements.selectedDeviceAddress.textContent = device.addresses[0] ?? "Pending";
  elements.selectedDeviceFingerprint.textContent = device.shortFingerprint;
  elements.selectedDeviceTrust.textContent = labelForTrust(device.trustState);
}

function renderDevices() {
  if (appState.selectedPeerId && !selectedDevice()) {
    appState.selectedPeerId = null;
  }

  if (!appState.devices.length) {
    elements.nearbyDevices.className = "device-list empty-state";
    elements.nearbyDevices.textContent = "No nearby receivers were found. Make sure the receiver is running on the same LAN and refresh again.";
    renderSelectedDevice();
    return;
  }

  elements.nearbyDevices.className = "device-list";
  elements.nearbyDevices.innerHTML = appState.devices
    .map((device, index) => {
      const selected = device.peerId === appState.selectedPeerId ? " selected" : "";
      const trustLabel = labelForTrust(device.trustState);
      const disabled = device.trustState === "fingerprint_mismatch" ? "disabled" : "";
      const buttonLabel = device.peerId === appState.selectedPeerId ? "Selected" : "Use device";
      return `
        <div class="device-card${selected}">
          <div>
            <h3>${device.deviceName}</h3>
            <p>${device.addresses.join(", ")}</p>
            <p>${device.peerId} via ${device.transport}</p>
            <div class="device-meta">
              <span class="badge ${badgeKindForTrust(device.trustState)}">${trustLabel}</span>
              <span class="badge idle">${device.shortFingerprint}</span>
            </div>
            <p>${device.trustMessage}</p>
          </div>
          <button type="button" data-index="${index}" ${disabled}>${buttonLabel}</button>
        </div>
      `;
    })
    .join("");

  for (const button of elements.nearbyDevices.querySelectorAll("button")) {
    button.addEventListener("click", () => {
      const device = appState.devices[Number(button.dataset.index)];
      if (!device || device.trustState === "fingerprint_mismatch") return;
      appState.selectedPeerId = device.peerId;
      elements.manualMode.checked = false;
      renderDevices();
    });
  }

  renderSelectedDevice();
}

function applyProgress(bar, text, speed, progress) {
  const percent = Number(progress?.percent ?? 0);
  bar.style.width = `${percent}%`;
  text.textContent = `${percent.toFixed(1)}%`;
  speed.textContent = `${Number(progress?.averageMibPerSec ?? 0).toFixed(2)} MiB/s`;
}

function applySendStatus(payload) {
  const state = payload.state ?? "idle";
  appState.sendBusy = state === "starting" || state === "sending";
  elements.sendButton.disabled = appState.sendBusy;
  elements.sendMessage.textContent = payload.message ?? "Ready.";

  if (state === "completed") setBadge(elements.sendBadge, "Completed", "success");
  else if (state === "error") setBadge(elements.sendBadge, "Error", "error");
  else if (appState.sendBusy) setBadge(elements.sendBadge, "Sending", "active");
  else setBadge(elements.sendBadge, "Idle", "idle");

  if (payload.progress) {
    applyProgress(elements.sendProgressBar, elements.sendProgressText, elements.sendSpeed, payload.progress);
    elements.sendSummaryBytes.textContent = `${formatBytes(payload.progress.transferredBytes)} / ${formatBytes(payload.progress.totalBytes)}`;
  }

  if (payload.summary) {
    elements.sendSummaryFile.textContent = payload.summary.fileName;
    elements.sendSummaryBytes.textContent = formatBytes(payload.summary.bytesTransferred);
    elements.sendSummaryChunks.textContent = String(payload.summary.completedChunks);
    elements.sendSummaryHash.textContent = payload.summary.sha256Hex;
  }
}

function applyReceiverStatus(payload) {
  const state = payload.state ?? "idle";
  appState.receiverBusy = state === "starting" || state === "listening" || state === "receiving";
  elements.startReceiver.disabled = appState.receiverBusy;
  elements.receiverMessage.textContent = payload.message ?? "Receiver idle.";

  if (state === "completed") setBadge(elements.receiverBadge, "Received", "success");
  else if (state === "error") setBadge(elements.receiverBadge, "Error", "error");
  else if (appState.receiverBusy) setBadge(elements.receiverBadge, state === "receiving" ? "Receiving" : "Listening", "active");
  else setBadge(elements.receiverBadge, "Idle", "idle");

  if (payload.bindAddr) elements.receiverBind.textContent = payload.bindAddr;
  if (payload.outputDir) elements.receiverOutput.textContent = payload.outputDir;
  if (payload.savedPath) elements.receiverSavedPath.textContent = payload.savedPath;
  if (payload.summary) elements.receiverFile.textContent = payload.summary.fileName;
  if (payload.progress) applyProgress(elements.receiverProgressBar, elements.receiverProgressText, elements.receiverSpeed, payload.progress);
}

async function refreshDevices() {
  elements.refreshDevices.disabled = true;
  elements.refreshDevices.textContent = "Scanning...";
  try {
    appState.devices = await invoke("discover_nearby_receivers", { timeoutSecs: 3 });
    renderDevices();
  } catch (error) {
    appState.devices = [];
    appState.selectedPeerId = null;
    renderDevices();
    applySendStatus({ state: "error", message: `Device discovery failed: ${error}` });
  } finally {
    elements.refreshDevices.disabled = false;
    elements.refreshDevices.textContent = "Refresh devices";
  }
}

async function startReceiver() {
  try {
    const response = await invoke("start_receiver", { deviceName: elements.deviceName.value.trim() || null });
    applyReceiverStatus({ state: "starting", message: `Preparing receiver on ${response.bindAddr}`, bindAddr: response.bindAddr, outputDir: response.outputDir });
  } catch (error) {
    applyReceiverStatus({ state: "error", message: `${error}` });
  }
}

async function startSend() {
  if (!elements.sourcePath.value) {
    applySendStatus({ state: "error", message: "Choose a source file before sending." });
    return;
  }

  if (elements.manualMode.checked) {
    if (!elements.targetAddress.value.trim() || !elements.certificatePath.value) {
      applySendStatus({ state: "error", message: "Manual target mode requires both a receiver address and a receiver certificate." });
      return;
    }
  } else if (!appState.selectedPeerId) {
    applySendStatus({ state: "error", message: "Select a nearby receiver or enable manual target mode." });
    return;
  }

  try {
    await invoke("start_send", {
      request: {
        sourcePath: elements.sourcePath.value,
        selectedPeerId: elements.manualMode.checked ? null : appState.selectedPeerId,
        targetAddr: elements.manualMode.checked ? elements.targetAddress.value.trim() : null,
        certificatePath: elements.manualMode.checked ? elements.certificatePath.value : null,
        serverName: elements.serverName.value.trim() || "fasttransfer.local",
        chunkSize: Number(elements.chunkSize.value || 1048576),
        parallelism: Number(elements.parallelism.value || 4),
      },
    });
    applySendStatus({
      state: "starting",
      message: elements.manualMode.checked
        ? `Connecting to ${elements.targetAddress.value.trim()} using manual target mode...`
        : `Connecting to ${selectedDevice()?.deviceName ?? "selected receiver"}...`,
    });
  } catch (error) {
    applySendStatus({ state: "error", message: `${error}` });
  }
}

function wireEvents() {
  elements.pickSource.addEventListener("click", async () => {
    const selected = await invoke("pick_source_file");
    if (selected) elements.sourcePath.value = selected;
  });
  elements.pickCertificate.addEventListener("click", async () => {
    const selected = await invoke("pick_certificate_file");
    if (selected) elements.certificatePath.value = selected;
  });
  elements.refreshDevices.addEventListener("click", refreshDevices);
  elements.startReceiver.addEventListener("click", startReceiver);
  elements.sendButton.addEventListener("click", startSend);
  elements.manualMode.addEventListener("change", () => {
    if (elements.manualMode.checked) {
      appState.selectedPeerId = null;
      renderSelectedDevice();
      renderDevices();
    }
  });
}

async function bootstrap() {
  wireEvents();
  await listen("send-status", (event) => applySendStatus(event.payload));
  await listen("receiver-status", (event) => applyReceiverStatus(event.payload));
  renderDevices();
  await startReceiver();
  await refreshDevices();
}

bootstrap().catch((error) => {
  applySendStatus({ state: "error", message: `App bootstrap failed: ${error}` });
  applyReceiverStatus({ state: "error", message: `App bootstrap failed: ${error}` });
});
