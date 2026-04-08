import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const $ = (selector) => document.querySelector(selector);
const elements = {
  sourcePath: $("#source-path"),
  nearbyDevices: $("#nearby-devices"),
  refreshDevices: $("#refresh-devices"),
  pickSourceFile: $("#pick-source-file"),
  pickSourceFolder: $("#pick-source-folder"),
  packageKindBadge: $("#package-kind-badge"),
  packageSummaryMessage: $("#package-summary-message"),
  packageRootName: $("#package-root-name"),
  packageRootKind: $("#package-root-kind"),
  packageTotalFiles: $("#package-total-files"),
  packageTotalFolders: $("#package-total-folders"),
  packageTotalBytes: $("#package-total-bytes"),
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
  receiverOutput: $("#receiver-output"),
  browseOutput: $("#browse-output"),
  startReceiver: $("#start-receiver"),
  startReceiverTop: $("#start-receiver-top"),
  receiverBadge: $("#receiver-badge"),
  receiverMessage: $("#receiver-message"),
  receiverProgressBar: $("#receiver-progress-bar"),
  receiverProgressText: $("#receiver-progress-text"),
  receiverSpeed: $("#receiver-speed"),
  receiverBind: $("#receiver-bind"),
  receiverFile: $("#receiver-file"),
  receiverSavedPath: $("#receiver-saved-path"),
  receiverCertificate: $("#receiver-certificate"),
};

const appState = {
  devices: [],
  selectedPeerId: null,
  sendBusy: false,
  receiverBusy: false,
  sourceSummary: null,
  receiveOutputDir: null,
};

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

function labelForPackageKind(rootKind) {
  return rootKind === "directory" ? "Folder package" : rootKind === "file" ? "Single file" : "Unknown";
}

function setReceiverActionDisabled(disabled) {
  elements.startReceiver.disabled = disabled;
  elements.startReceiverTop.disabled = disabled;
}

function selectedDevice() {
  return appState.devices.find((device) => device.peerId === appState.selectedPeerId) ?? null;
}

function renderPackageSummary() {
  const summary = appState.sourceSummary;
  if (!summary) {
    setBadge(elements.packageKindBadge, "Nothing selected", "idle");
    elements.packageSummaryMessage.textContent = "Choose a file or folder to preview the package structure before sending.";
    elements.packageRootName.textContent = "Pending";
    elements.packageRootKind.textContent = "Pending";
    elements.packageTotalFiles.textContent = "0";
    elements.packageTotalFolders.textContent = "0";
    elements.packageTotalBytes.textContent = "0 B";
    elements.sendSummaryFile.textContent = "Not started";
    return;
  }

  const kindLabel = labelForPackageKind(summary.rootKind);
  setBadge(elements.packageKindBadge, kindLabel, summary.rootKind === "directory" ? "active" : "success");
  elements.packageSummaryMessage.textContent = `Ready to send ${summary.rootName} with ${summary.totalFiles.toLocaleString()} file${summary.totalFiles === 1 ? "" : "s"}.`;
  elements.packageRootName.textContent = summary.rootName;
  elements.packageRootKind.textContent = kindLabel;
  elements.packageTotalFiles.textContent = summary.totalFiles.toLocaleString();
  elements.packageTotalFolders.textContent = summary.totalDirectories.toLocaleString();
  elements.packageTotalBytes.textContent = formatBytes(summary.totalBytes);
  elements.sendSummaryFile.textContent = summary.rootName;
}

async function setSourcePath(path) {
  elements.sourcePath.value = path;
  appState.sourceSummary = null;
  renderPackageSummary();

  try {
    const summary = await invoke("inspect_source", { sourcePath: path });
    appState.sourceSummary = summary;
    renderPackageSummary();
  } catch (error) {
    appState.sourceSummary = null;
    renderPackageSummary();
    applySendStatus({ state: "error", message: `Failed to inspect source: ${error}` });
  }
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
      const buttonLabel = device.peerId === appState.selectedPeerId ? "Selected" : "Select";
      return `
        <div class="device-card${selected}">
          <div>
            <h3>${device.deviceName}</h3>
            <p>${device.addresses.join(", ")}</p>
            <div class="device-meta">
              <span class="badge ${badgeKindForTrust(device.trustState)}">${trustLabel}</span>
              <span class="badge idle">${device.shortFingerprint}</span>
            </div>
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
    elements.sendSummaryChunks.textContent = `${payload.progress.completedFiles} / ${payload.progress.totalFiles}`;
    if (payload.progress.currentPath) elements.sendSummaryHash.textContent = payload.progress.currentPath;
  }

  if (payload.summary) {
    elements.sendSummaryFile.textContent = payload.summary.fileName;
    elements.sendSummaryBytes.textContent = formatBytes(payload.summary.bytesTransferred);
    elements.sendSummaryChunks.textContent = String(payload.summary.completedFiles);
    elements.sendSummaryHash.textContent = "Verified";
  }
}

function applyReceiverStatus(payload) {
  const state = payload.state ?? "idle";
  appState.receiverBusy = state === "starting" || state === "listening" || state === "receiving";
  setReceiverActionDisabled(appState.receiverBusy);
  elements.receiverMessage.textContent = payload.message ?? "Start the receiver to advertise this device and accept incoming transfers";

  if (state === "completed") setBadge(elements.receiverBadge, "Received", "success");
  else if (state === "error") setBadge(elements.receiverBadge, "Error", "error");
  else if (appState.receiverBusy) setBadge(elements.receiverBadge, state === "receiving" ? "Receiving" : "Listening", "active");
  else setBadge(elements.receiverBadge, "Idle", "idle");

  if (payload.outputDirLabel || payload.outputDir) {
    elements.receiverOutput.value = payload.outputDirLabel ?? payload.outputDir;
  }
  if (payload.bindAddr) {
    elements.receiverBind.textContent = payload.bindAddr;
  }
  if (payload.savedPathLabel || payload.savedPath) {
    elements.receiverSavedPath.textContent = payload.savedPathLabel ?? payload.savedPath;
  }
  if (payload.certificatePath) {
    elements.receiverCertificate.textContent = payload.certificatePath.split(/[\\/]/).pop() ?? payload.certificatePath;
  }
  if (payload.progress) {
    applyProgress(elements.receiverProgressBar, elements.receiverProgressText, elements.receiverSpeed, payload.progress);
    if (payload.progress.currentPath) elements.receiverFile.textContent = payload.progress.currentPath;
  }
  if (payload.summary) {
    elements.receiverFile.textContent = payload.summary.fileName;
  }
}

async function refreshDevices() {
  elements.refreshDevices.disabled = true;
  elements.refreshDevices.textContent = "Refreshing...";
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
    elements.refreshDevices.textContent = "Refresh";
  }
}

async function browseReceiveOutput() {
  try {
    const selected = await invoke("pick_receive_folder");
    if (selected) {
      appState.receiveOutputDir = selected;
      elements.receiverOutput.value = selected;
    }
  } catch (error) {
    applyReceiverStatus({ state: "error", message: `${error}` });
  }
}

async function startReceiver() {
  try {
    const response = await invoke("start_receiver", {
      deviceName: elements.deviceName.value.trim() || null,
      outputDir: appState.receiveOutputDir,
    });
    appState.receiveOutputDir = response.outputDir;
    elements.receiverOutput.value = response.outputDirLabel ?? response.outputDir;
    applyReceiverStatus({
      state: "starting",
      message: `Preparing receiver on ${response.bindAddr}`,
      bindAddr: response.bindAddr,
      outputDir: response.outputDir,
      outputDirLabel: response.outputDirLabel,
      certificatePath: response.certificatePath,
    });
  } catch (error) {
    applyReceiverStatus({ state: "error", message: `${error}` });
  }
}

async function startSend() {
  if (!elements.sourcePath.value) {
    applySendStatus({ state: "error", message: "Choose a source file or folder before sending." });
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
  elements.pickSourceFile.addEventListener("click", async () => {
    const selected = await invoke("pick_source_file");
    if (selected) await setSourcePath(selected);
  });
  elements.pickSourceFolder.addEventListener("click", async () => {
    const selected = await invoke("pick_source_folder");
    if (selected) await setSourcePath(selected);
  });
  elements.pickCertificate.addEventListener("click", async () => {
    const selected = await invoke("pick_certificate_file");
    if (selected) elements.certificatePath.value = selected;
  });
  elements.browseOutput.addEventListener("click", browseReceiveOutput);
  elements.refreshDevices.addEventListener("click", refreshDevices);
  elements.startReceiver.addEventListener("click", startReceiver);
  elements.startReceiverTop.addEventListener("click", startReceiver);
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
  renderSelectedDevice();
  renderPackageSummary();
  await startReceiver();
  await refreshDevices();
}

bootstrap().catch((error) => {
  applySendStatus({ state: "error", message: `App bootstrap failed: ${error}` });
  applyReceiverStatus({ state: "error", message: `App bootstrap failed: ${error}` });
});
