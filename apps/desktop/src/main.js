import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const $ = (selector) => document.querySelector(selector);
const $$ = (selector) => Array.from(document.querySelectorAll(selector));

const elements = {
  navItems: $$(".nav-item"),
  views: $$('[data-screen]'),
  viewTitle: $("#view-title"),
  viewSubtitle: $("#view-subtitle"),
  globalRefresh: $("#global-refresh"),
  statusLeft: $("#status-left"),
  statusCenter: $("#status-center"),
  statusRight: $("#status-right"),
  sidebarReceiverState: $("#sidebar-receiver-state"),
  sidebarSelectedDevice: $("#sidebar-selected-device"),

  sourcePath: $("#source-path"),
  pickSourceFile: $("#pick-source-file"),
  pickSourceFolder: $("#pick-source-folder"),
  refreshDevicesSend: $("#refresh-devices-send"),
  refreshDevicesView: $("#refresh-devices-view"),
  sendDeviceList: $("#send-device-list"),
  devicesNearbyList: $("#devices-nearby-list"),
  trustedDeviceList: $("#trusted-device-list"),

  sendTargetBadge: $("#send-target-badge"),
  sendTargetMessage: $("#send-target-message"),
  sendTargetName: $("#send-target-name"),
  sendTargetAddress: $("#send-target-address"),
  sendTargetFingerprint: $("#send-target-fingerprint"),
  sendTargetTrust: $("#send-target-trust"),

  packageKindBadge: $("#package-kind-badge"),
  packageSummaryMessage: $("#package-summary-message"),
  packageRootName: $("#package-root-name"),
  packageRootKind: $("#package-root-kind"),
  packageTotalFiles: $("#package-total-files"),
  packageTotalFolders: $("#package-total-folders"),
  packageTotalBytes: $("#package-total-bytes"),

  manualMode: $("#manual-mode"),
  targetAddress: $("#target-address"),
  certificatePath: $("#certificate-path"),
  pickCertificate: $("#pick-certificate"),
  serverName: $("#server-name"),
  chunkSize: $("#chunk-size"),
  parallelism: $("#parallelism"),
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

  deviceName: $("#device-name"),
  receiverToggle: $("#receiver-toggle"),
  browseOutput: $("#browse-output"),
  receiverOutput: $("#receiver-output"),
  receiverBadge: $("#receiver-badge"),
  receiverMessage: $("#receiver-message"),
  receiverProgressBar: $("#receiver-progress-bar"),
  receiverProgressText: $("#receiver-progress-text"),
  receiverSpeed: $("#receiver-speed"),
  receiverBind: $("#receiver-bind"),
  receiverFile: $("#receiver-file"),
  receiverSavedPath: $("#receiver-saved-path"),
  receiverCertificate: $("#receiver-certificate"),
  recentReceivedList: $("#recent-received-list"),

  transfersTableBody: $("#transfers-table-body"),

  deviceDetailBadge: $("#device-detail-badge"),
  deviceDetailMessage: $("#device-detail-message"),
  deviceDetailName: $("#device-detail-name"),
  deviceDetailAddress: $("#device-detail-address"),
  deviceDetailFingerprint: $("#device-detail-fingerprint"),
  deviceDetailTrust: $("#device-detail-trust"),

  settingsReceiveFolder: $("#settings-receive-folder"),
  settingsBrowseOutput: $("#settings-browse-output"),
  settingsLanDiscovery: $("#settings-lan-discovery"),
  settingsAutoTrust: $("#settings-auto-trust"),
  settingsBindAddress: $("#settings-bind-address"),
  settingsDiscoveryTimeout: $("#settings-discovery-timeout"),
  settingsTheme: $("#settings-theme"),
};

const viewMeta = {
  send: {
    title: "Send",
    subtitle: "Choose a file or folder and send it to a nearby receiver.",
  },
  receive: {
    title: "Receive",
    subtitle: "Control the receiver and review recent incoming transfers.",
  },
  transfers: {
    title: "Transfers",
    subtitle: "Track active and completed send and receive activity.",
  },
  devices: {
    title: "Devices",
    subtitle: "Inspect nearby receivers and their LAN trust identities.",
  },
  settings: {
    title: "Settings",
    subtitle: "Review default folders and desktop transfer preferences.",
  },
};

const appState = {
  currentView: "send",
  devices: [],
  selectedPeerId: null,
  sourceSummary: null,
  sendBusy: false,
  receiverActive: false,
  receiverState: "idle",
  sendMessage: "Ready.",
  receiverMessage: "Receiver stopped.",
  receiveOutputDir: null,
  receiveOutputLabel: "Downloads/FastTransfer",
  transfers: [],
  transferCounter: 0,
  currentSendTransferId: null,
  currentReceiveTransferId: null,
  recentReceived: [],
};

function escapeHtml(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function formatBytes(bytes) {
  if (!bytes) return "0 B";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let value = Number(bytes);
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value.toFixed(index === 0 ? 0 : 2)} ${units[index]}`;
}

function basename(path) {
  if (!path) return "";
  const parts = String(path).split(/[/\\]/).filter(Boolean);
  return parts.at(-1) ?? path;
}

function formatTime(timestamp) {
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    month: "short",
    day: "numeric",
  }).format(new Date(timestamp));
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

function labelForTransferState(state) {
  if (state === "starting") return "Starting";
  if (state === "sending") return "Sending";
  if (state === "receiving") return "Receiving";
  if (state === "completed") return "Completed";
  if (state === "error") return "Error";
  if (state === "listening") return "Listening";
  if (state === "stopped") return "Stopped";
  return "Idle";
}

function kindForTransferState(state) {
  if (state === "completed") return "success";
  if (state === "error") return "error";
  if (state === "starting" || state === "sending" || state === "receiving" || state === "listening") return "active";
  return "idle";
}

function selectedDevice() {
  return appState.devices.find((device) => device.peerId === appState.selectedPeerId) ?? null;
}

function syncReceiveFolderDisplays(label, absolutePath = null) {
  appState.receiveOutputLabel = label || "Downloads/FastTransfer";
  if (absolutePath !== null) {
    appState.receiveOutputDir = absolutePath;
  }
  elements.receiverOutput.value = appState.receiveOutputLabel;
  elements.settingsReceiveFolder.value = absolutePath ?? appState.receiveOutputLabel;
}

function switchView(view) {
  appState.currentView = view;
  const meta = viewMeta[view];
  elements.viewTitle.textContent = meta.title;
  elements.viewSubtitle.textContent = meta.subtitle;
  for (const button of elements.navItems) {
    button.classList.toggle("active", button.dataset.view === view);
  }
  for (const panel of elements.views) {
    const active = panel.dataset.screen === view;
    panel.classList.toggle("active", active);
    panel.hidden = !active;
  }
  updateStatusBar();
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

function renderSelectedTarget() {
  if (elements.manualMode.checked) {
    const address = elements.targetAddress.value.trim();
    const hasCert = Boolean(elements.certificatePath.value);
    setBadge(elements.sendTargetBadge, "Manual", address ? "active" : "idle");
    elements.sendTargetMessage.textContent = "Manual target mode is enabled for direct receiver entry.";
    elements.sendTargetName.textContent = "Manual target";
    elements.sendTargetAddress.textContent = address || "Pending";
    elements.sendTargetFingerprint.textContent = hasCert ? basename(elements.certificatePath.value) : "Pending";
    elements.sendTargetTrust.textContent = hasCert ? "Manual certificate" : "Awaiting certificate";
    elements.sidebarSelectedDevice.textContent = address || "Manual target";
    return;
  }

  const device = selectedDevice();
  if (!device) {
    setBadge(elements.sendTargetBadge, "No target", "idle");
    elements.sendTargetMessage.textContent = "Select a nearby receiver or use manual target mode.";
    elements.sendTargetName.textContent = "Pending";
    elements.sendTargetAddress.textContent = "Pending";
    elements.sendTargetFingerprint.textContent = "Pending";
    elements.sendTargetTrust.textContent = "Pending";
    elements.sidebarSelectedDevice.textContent = "None";
    return;
  }

  setBadge(elements.sendTargetBadge, labelForTrust(device.trustState), badgeKindForTrust(device.trustState));
  elements.sendTargetMessage.textContent = device.trustMessage;
  elements.sendTargetName.textContent = device.deviceName;
  elements.sendTargetAddress.textContent = device.addresses[0] ?? "Pending";
  elements.sendTargetFingerprint.textContent = device.shortFingerprint;
  elements.sendTargetTrust.textContent = labelForTrust(device.trustState);
  elements.sidebarSelectedDevice.textContent = device.deviceName;
}

function renderDeviceIdentity() {
  const device = selectedDevice();
  if (!device) {
    setBadge(elements.deviceDetailBadge, "None", "idle");
    elements.deviceDetailMessage.textContent = "Select a nearby device to inspect its identity information.";
    elements.deviceDetailName.textContent = "Pending";
    elements.deviceDetailAddress.textContent = "Pending";
    elements.deviceDetailFingerprint.textContent = "Pending";
    elements.deviceDetailTrust.textContent = "Pending";
    return;
  }

  setBadge(elements.deviceDetailBadge, labelForTrust(device.trustState), badgeKindForTrust(device.trustState));
  elements.deviceDetailMessage.textContent = device.trustMessage;
  elements.deviceDetailName.textContent = device.deviceName;
  elements.deviceDetailAddress.textContent = device.addresses[0] ?? "Pending";
  elements.deviceDetailFingerprint.textContent = device.shortFingerprint;
  elements.deviceDetailTrust.textContent = labelForTrust(device.trustState);
}

function deviceRowMarkup(device, index) {
  const selected = device.peerId === appState.selectedPeerId;
  const trustLabel = labelForTrust(device.trustState);
  const buttonLabel = selected ? "Selected" : "Use";
  const disabled = device.trustState === "fingerprint_mismatch" ? "disabled" : "";
  return `
    <div class="list-item${selected ? " selected" : ""}">
      <div>
        <strong>${escapeHtml(device.deviceName)}</strong>
        <p>${escapeHtml(device.addresses.join(", ") || "No address")}</p>
        <div class="list-item-meta">
          <span class="badge ${badgeKindForTrust(device.trustState)}">${escapeHtml(trustLabel)}</span>
          <span class="badge idle">${escapeHtml(device.shortFingerprint)}</span>
        </div>
      </div>
      <button type="button" data-device-index="${index}" ${disabled}>${buttonLabel}</button>
    </div>
  `;
}

function bindDeviceButtons(container) {
  for (const button of container.querySelectorAll("[data-device-index]")) {
    button.addEventListener("click", () => {
      const device = appState.devices[Number(button.dataset.deviceIndex)];
      if (!device || device.trustState === "fingerprint_mismatch") return;
      appState.selectedPeerId = device.peerId;
      elements.manualMode.checked = false;
      renderAll();
    });
  }
}

function renderDeviceLists() {
  if (!appState.devices.length) {
    const message = "No nearby receivers were found. Start a receiver on the same LAN and refresh.";
    elements.sendDeviceList.className = "list-panel empty-state";
    elements.sendDeviceList.textContent = message;
    elements.devicesNearbyList.className = "list-panel empty-state";
    elements.devicesNearbyList.textContent = message;
  } else {
    const markup = appState.devices.map(deviceRowMarkup).join("");
    elements.sendDeviceList.className = "list-panel";
    elements.sendDeviceList.innerHTML = markup;
    elements.devicesNearbyList.className = "list-panel";
    elements.devicesNearbyList.innerHTML = markup;
    bindDeviceButtons(elements.sendDeviceList);
    bindDeviceButtons(elements.devicesNearbyList);
  }

  const trusted = appState.devices.filter((device) => device.trustState !== "unknown");
  if (!trusted.length) {
    elements.trustedDeviceList.className = "list-panel empty-state";
    elements.trustedDeviceList.textContent = "No trusted devices are available in the current LAN scan.";
  } else {
    elements.trustedDeviceList.className = "list-panel";
    elements.trustedDeviceList.innerHTML = trusted
      .map(
        (device) => `
          <div class="trusted-item">
            <strong>${escapeHtml(device.deviceName)}</strong>
            <span>${escapeHtml(labelForTrust(device.trustState))} · ${escapeHtml(device.shortFingerprint)}</span>
            <span>${escapeHtml(device.addresses[0] ?? "No address")}</span>
          </div>
        `,
      )
      .join("");
  }
}

function renderRecentReceived() {
  if (!appState.recentReceived.length) {
    elements.recentReceivedList.className = "list-panel empty-state";
    elements.recentReceivedList.textContent = "No received items yet.";
    return;
  }

  elements.recentReceivedList.className = "list-panel";
  elements.recentReceivedList.innerHTML = appState.recentReceived
    .map(
      (item) => `
        <div class="recent-item">
          <strong>${escapeHtml(item.name)}</strong>
          <span>${escapeHtml(item.path)}</span>
          <span>${escapeHtml(item.remote)} · ${escapeHtml(formatTime(item.timestamp))}</span>
        </div>
      `,
    )
    .join("");
}

function createTransfer(direction, name, destination) {
  const id = `transfer-${++appState.transferCounter}`;
  appState.transfers.unshift({
    id,
    direction,
    name,
    status: "Idle",
    progress: "0%",
    speed: "0.00 MiB/s",
    destination,
    updatedAt: Date.now(),
  });
  appState.transfers = appState.transfers.slice(0, 20);
  return id;
}

function updateTransfer(id, patch) {
  const transfer = appState.transfers.find((entry) => entry.id === id);
  if (!transfer) return;
  Object.assign(transfer, patch, { updatedAt: Date.now() });
}

function ensureTransfer(slotKey, direction, name, destination) {
  let id = appState[slotKey];
  if (!id) {
    id = createTransfer(direction, name, destination);
    appState[slotKey] = id;
  }
  return id;
}

function renderTransfers() {
  if (!appState.transfers.length) {
    elements.transfersTableBody.innerHTML = `
      <tr>
        <td colspan="5" class="table-empty">No transfer activity yet.</td>
      </tr>
    `;
    return;
  }

  const rows = [...appState.transfers]
    .sort((a, b) => b.updatedAt - a.updatedAt)
    .map(
      (transfer) => `
        <tr>
          <td>${escapeHtml(`${transfer.direction}: ${transfer.name}`)}</td>
          <td><span class="status-pill">${escapeHtml(transfer.status)}</span></td>
          <td>${escapeHtml(transfer.progress)}</td>
          <td>${escapeHtml(transfer.speed)}</td>
          <td>${escapeHtml(transfer.destination)}</td>
        </tr>
      `,
    )
    .join("");

  elements.transfersTableBody.innerHTML = rows;
}

function setReceiverToggle() {
  elements.receiverToggle.disabled = appState.receiverState === "starting";
  elements.receiverToggle.textContent = appState.receiverActive ? "Stop receiver" : "Start receiver";
}

function updateStatusBar() {
  elements.statusLeft.textContent = appState.currentView === "receive" ? appState.receiverMessage : appState.sendMessage;
  elements.statusCenter.textContent = `Receiver ${labelForTransferState(appState.receiverState).toLowerCase()}`;
  if (elements.manualMode.checked) {
    elements.statusRight.textContent = elements.targetAddress.value.trim() || "Manual target";
  } else {
    elements.statusRight.textContent = selectedDevice()?.deviceName ?? "No target selected";
  }
  elements.sidebarReceiverState.textContent = labelForTransferState(appState.receiverState);
}

function renderAll() {
  renderPackageSummary();
  renderSelectedTarget();
  renderDeviceIdentity();
  renderDeviceLists();
  renderRecentReceived();
  renderTransfers();
  renderSettings();
  setReceiverToggle();
  updateStatusBar();
}

function renderSettings() {
  elements.settingsLanDiscovery.checked = true;
  elements.settingsAutoTrust.checked = true;
  elements.settingsTheme.value = "System (placeholder)";
  elements.settingsBindAddress.value = "0.0.0.0:5000";
  if (!elements.settingsDiscoveryTimeout.value) {
    elements.settingsDiscoveryTimeout.value = "3";
  }
  syncReceiveFolderDisplays(appState.receiveOutputLabel, appState.receiveOutputDir);
}

async function refreshDevices() {
  const timeout = Number(elements.settingsDiscoveryTimeout.value || 3);
  elements.globalRefresh.disabled = true;
  elements.refreshDevicesSend.disabled = true;
  elements.refreshDevicesView.disabled = true;
  try {
    appState.devices = await invoke("discover_nearby_receivers", { timeoutSecs: timeout });
    renderAll();
  } catch (error) {
    appState.devices = [];
    appState.selectedPeerId = null;
    renderAll();
    applySendStatus({ state: "error", message: `Device discovery failed: ${error}` });
  } finally {
    elements.globalRefresh.disabled = false;
    elements.refreshDevicesSend.disabled = false;
    elements.refreshDevicesView.disabled = false;
  }
}

async function browseReceiveOutput() {
  try {
    const selected = await invoke("pick_receive_folder");
    if (selected) {
      syncReceiveFolderDisplays(selected, selected);
      renderAll();
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
    syncReceiveFolderDisplays(response.outputDirLabel ?? response.outputDir, response.outputDir);
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

async function stopReceiver() {
  try {
    await invoke("stop_receiver");
    appState.receiverState = "stopping";
    appState.receiverMessage = "Stopping receiver...";
    updateStatusBar();
  } catch (error) {
    applyReceiverStatus({ state: "error", message: `${error}` });
  }
}

async function toggleReceiver() {
  if (appState.receiverActive) {
    await stopReceiver();
  } else {
    await startReceiver();
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

function applyProgress(bar, text, speed, progress) {
  const percent = Number(progress?.percent ?? 0);
  bar.style.width = `${percent}%`;
  text.textContent = `${percent.toFixed(1)}%`;
  speed.textContent = `${Number(progress?.averageMibPerSec ?? 0).toFixed(2)} MiB/s`;
}

function applySendStatus(payload) {
  const state = payload.state ?? "idle";
  appState.sendBusy = state === "starting" || state === "sending";
  appState.sendMessage = payload.message ?? "Ready.";
  elements.sendButton.disabled = appState.sendBusy;
  elements.sendMessage.textContent = appState.sendMessage;
  setBadge(elements.sendBadge, labelForTransferState(state), kindForTransferState(state));

  if (payload.progress) {
    applyProgress(elements.sendProgressBar, elements.sendProgressText, elements.sendSpeed, payload.progress);
    elements.sendSummaryBytes.textContent = `${formatBytes(payload.progress.transferredBytes)} / ${formatBytes(payload.progress.totalBytes)}`;
    elements.sendSummaryChunks.textContent = `${payload.progress.completedFiles} / ${payload.progress.totalFiles}`;
    elements.sendSummaryHash.textContent = payload.progress.currentPath || "In progress";
  }

  if (payload.summary) {
    elements.sendSummaryFile.textContent = payload.summary.fileName;
    elements.sendSummaryBytes.textContent = formatBytes(payload.summary.bytesTransferred);
    elements.sendSummaryChunks.textContent = String(payload.summary.completedFiles);
    elements.sendSummaryHash.textContent = payload.summary.integrityVerified ? "Verified" : payload.summary.sha256Hex;
  }

  const transferName = appState.sourceSummary?.rootName || basename(elements.sourcePath.value) || "Outgoing package";
  const destination = elements.manualMode.checked
    ? elements.targetAddress.value.trim() || "Manual target"
    : selectedDevice()?.deviceName || "Nearby device";

  if (["starting", "sending", "completed", "error"].includes(state)) {
    const id = ensureTransfer("currentSendTransferId", "Send", transferName, destination);
    updateTransfer(id, {
      name: transferName,
      destination,
      status: labelForTransferState(state),
      progress: payload.progress ? `${Number(payload.progress.percent ?? 0).toFixed(1)}%` : state === "completed" ? "100%" : "0%",
      speed: payload.progress ? `${Number(payload.progress.averageMibPerSec ?? 0).toFixed(2)} MiB/s` : state === "completed" && payload.summary ? `${Number(payload.summary.averageMibPerSec ?? 0).toFixed(2)} MiB/s` : "0.00 MiB/s",
    });
    if (state === "completed" || state === "error") {
      appState.currentSendTransferId = null;
    }
  }

  renderTransfers();
  updateStatusBar();
}

function applyReceiverStatus(payload) {
  const state = payload.state ?? "idle";
  appState.receiverState = state;
  appState.receiverActive = ["starting", "listening", "receiving"].includes(state);
  appState.receiverMessage = payload.message ?? "Receiver stopped.";
  elements.receiverMessage.textContent = appState.receiverMessage;
  setBadge(elements.receiverBadge, labelForTransferState(state), kindForTransferState(state));
  setReceiverToggle();

  if (payload.outputDirLabel || payload.outputDir) {
    syncReceiveFolderDisplays(payload.outputDirLabel ?? payload.outputDir, payload.outputDir ?? appState.receiveOutputDir);
  }
  if (payload.bindAddr) elements.receiverBind.textContent = payload.bindAddr;
  if (payload.savedPathLabel || payload.savedPath) {
    elements.receiverSavedPath.textContent = payload.savedPathLabel ?? payload.savedPath;
  }
  if (payload.certificatePath) {
    elements.receiverCertificate.textContent = basename(payload.certificatePath);
  }
  if (payload.progress) {
    applyProgress(elements.receiverProgressBar, elements.receiverProgressText, elements.receiverSpeed, payload.progress);
    elements.receiverFile.textContent = payload.progress.currentPath || "Incoming file";
    const id = ensureTransfer(
      "currentReceiveTransferId",
      "Receive",
      payload.progress.currentPath || "Incoming transfer",
      payload.outputDirLabel ?? payload.outputDir ?? appState.receiveOutputLabel,
    );
    updateTransfer(id, {
      name: payload.progress.currentPath || "Incoming transfer",
      destination: payload.outputDirLabel ?? payload.outputDir ?? appState.receiveOutputLabel,
      status: labelForTransferState(state),
      progress: `${Number(payload.progress.percent ?? 0).toFixed(1)}%`,
      speed: `${Number(payload.progress.averageMibPerSec ?? 0).toFixed(2)} MiB/s`,
    });
  }

  if (payload.summary) {
    elements.receiverFile.textContent = payload.summary.fileName;
    const destination = payload.savedPathLabel ?? payload.savedPath ?? appState.receiveOutputLabel;
    const id = ensureTransfer("currentReceiveTransferId", "Receive", payload.summary.fileName, destination);
    updateTransfer(id, {
      name: payload.summary.fileName,
      destination,
      status: labelForTransferState(state),
      progress: state === "completed" ? "100%" : "Error",
      speed: `${Number(payload.summary.averageMibPerSec ?? 0).toFixed(2)} MiB/s`,
    });
    appState.currentReceiveTransferId = null;

    if (state === "completed") {
      appState.recentReceived.unshift({
        name: payload.summary.fileName,
        path: destination,
        remote: payload.remoteAddress ?? "Unknown sender",
        timestamp: Date.now(),
      });
      appState.recentReceived = appState.recentReceived.slice(0, 8);
    }
  }

  if (state === "error" && appState.currentReceiveTransferId) {
    updateTransfer(appState.currentReceiveTransferId, {
      status: "Error",
      progress: "Failed",
      speed: "0.00 MiB/s",
    });
    appState.currentReceiveTransferId = null;
  }

  renderRecentReceived();
  renderTransfers();
  updateStatusBar();
}

function wireEvents() {
  for (const button of elements.navItems) {
    button.addEventListener("click", () => switchView(button.dataset.view));
  }

  elements.globalRefresh.addEventListener("click", refreshDevices);
  elements.refreshDevicesSend.addEventListener("click", refreshDevices);
  elements.refreshDevicesView.addEventListener("click", refreshDevices);

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
    if (selected) {
      elements.certificatePath.value = selected;
      renderSelectedTarget();
      updateStatusBar();
    }
  });

  elements.manualMode.addEventListener("change", renderAll);
  elements.targetAddress.addEventListener("input", () => {
    renderSelectedTarget();
    updateStatusBar();
  });

  elements.sendButton.addEventListener("click", startSend);
  elements.receiverToggle.addEventListener("click", toggleReceiver);
  elements.browseOutput.addEventListener("click", browseReceiveOutput);
  elements.settingsBrowseOutput.addEventListener("click", browseReceiveOutput);
}

async function bootstrap() {
  wireEvents();
  await listen("send-status", (event) => applySendStatus(event.payload));
  await listen("receiver-status", (event) => applyReceiverStatus(event.payload));
  syncReceiveFolderDisplays(appState.receiveOutputLabel, appState.receiveOutputDir);
  switchView("send");
  renderAll();
  await refreshDevices();
}

bootstrap().catch((error) => {
  applySendStatus({ state: "error", message: `App bootstrap failed: ${error}` });
  applyReceiverStatus({ state: "error", message: `App bootstrap failed: ${error}` });
});
