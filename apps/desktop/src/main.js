
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
  newTransfer: $("#new-transfer"),
  networkBadge: $("#network-badge"),
  networkSummary: $("#network-summary"),
  statusLeft: $("#status-left"),
  statusCenter: $("#status-center"),
  statusRight: $("#status-right"),

  sourcePath: $("#source-path"),
  pickSourceFile: $("#pick-source-file"),
  pickSourceFolder: $("#pick-source-folder"),
  deviceSearch: $("#device-search"),
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
  actionTargetName: $("#action-target-name"),

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
  autoTune: $("#auto-tune"),
  autoTuneSummary: $("#auto-tune-summary"),
  sendButton: $("#send-button"),

  sendBadge: $("#send-badge"),
  sendMessage: $("#send-message"),
  sendProgressBar: $("#send-progress-bar"),
  sendProgressText: $("#send-progress-text"),
  sendSpeed: $("#send-speed"),
  sendSummaryFile: $("#send-summary-file"),
  sendSummaryMeta: $("#send-summary-meta"),
  sendSummaryBytes: $("#send-summary-bytes"),
  sendSummaryChunks: $("#send-summary-chunks"),
  sendSummaryHash: $("#send-summary-hash"),

  deviceName: $("#device-name"),
  receiverToggle: $("#receiver-toggle"),
  browseOutput: $("#browse-output"),
  receiverOutput: $("#receiver-output"),
  receiverOutputLabel: $("#receiver-output-label"),
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
    subtitle: "Choose a nearby device, pick a file or folder, and send it over secure LAN.",
  },
  receive: {
    title: "Receive",
    subtitle: "Listen for nearby transfers and keep recent received items within reach.",
  },
  transfers: {
    title: "Transfers",
    subtitle: "Track active and completed transfers in one compact queue.",
  },
  devices: {
    title: "Devices",
    subtitle: "Review nearby devices, trust state, and LAN identity details.",
  },
  settings: {
    title: "Settings",
    subtitle: "Adjust receive location, discovery behavior, and advanced desktop options.",
  },
};

const appState = {
  currentView: "send",
  devices: [],
  deviceSearch: "",
  selectedPeerId: null,
  sourceSummary: null,
  sendBusy: false,
  receiverActive: false,
  receiverState: "idle",
  sendMessage: "Ready.",
  receiverMessage: "Receiver stopped.",
  receiveOutputDir: null,
  receiveOutputLabel: "Downloads/FastTransfer",
  receiverBindAddr: "Not listening",
  receiverCertificateName: "Pending",
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
  const parts = String(path).split(/[\\/]/).filter(Boolean);
  return parts.at(-1) ?? path;
}

function simplifiedPathLabel(path) {
  if (!path) return "Downloads/FastTransfer";
  const normalized = String(path).replaceAll("\\", "/");
  const downloadsMatch = normalized.match(/(?:^|\/)(Downloads\/FastTransfer.*)$/i);
  if (downloadsMatch) return downloadsMatch[1];
  const desktopMatch = normalized.match(/(?:^|\/)(Desktop\/FastTransfer.*)$/i);
  if (desktopMatch) return desktopMatch[1];
  const parts = normalized.split("/").filter(Boolean);
  return parts.slice(-2).join("/") || normalized;
}

function formatTime(timestamp) {
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    month: "short",
    day: "numeric",
  }).format(new Date(timestamp));
}

function inferDeviceType(device) {
  const text = `${device.deviceName ?? ""} ${device.serviceName ?? ""}`.toLowerCase();
  if (/(pixel|iphone|phone|android|mobile)/.test(text)) return "phone";
  if (/(studio|desktop|pc|tower|workstation)/.test(text)) return "desktop";
  return "laptop";
}

function deviceGlyph(device) {
  const type = inferDeviceType(device);
  if (type === "phone") return "PH";
  if (type === "desktop") return "PC";
  return "LT";
}

function setPill(node, label, kind = "idle") {
  node.textContent = label;
  node.className = `pill pill-${kind}`;
}

function trustKind(trustState) {
  if (trustState === "known_device") return "success";
  if (trustState === "trusted_for_session") return "active";
  if (trustState === "fingerprint_mismatch") return "error";
  return "idle";
}

function trustLabel(trustState) {
  if (trustState === "known_device") return "Known device";
  if (trustState === "trusted_for_session") return "Trusted for this session";
  if (trustState === "fingerprint_mismatch") return "Fingerprint changed";
  return "Unverified";
}

function packageKindLabel(rootKind) {
  if (rootKind === "directory") return "Folder";
  if (rootKind === "file") return "File";
  return "Unknown";
}

function packageKindBadgeLabel(rootKind) {
  if (rootKind === "directory") return "Folder package";
  if (rootKind === "file") return "Single file";
  return "Unknown";
}

function recommendedTransferTuning(summary) {
  const defaultTuning = {
    chunkSize: 4194304,
    parallelism: 8,
    profile: "mixed",
    reason: "Balanced defaults for local gigabit LAN.",
  };

  if (!summary) {
    return defaultTuning;
  }

  return {
    chunkSize: Math.max(1, Number(summary.recommendedChunkSize ?? defaultTuning.chunkSize)),
    parallelism: Math.max(1, Number(summary.recommendedParallelism ?? defaultTuning.parallelism)),
    profile: String(summary.tuningProfile ?? defaultTuning.profile),
    reason: String(summary.tuningReason ?? defaultTuning.reason),
  };
}

function tuningProfileLabel(profile) {
  if (profile === "tiny_transfer") return "Tiny transfer";
  if (profile === "many_small_files") return "Many small files";
  if (profile === "large_files") return "Large files";
  if (profile === "mixed") return "Mixed workload";
  return "Auto";
}

function activeTransferTuning() {
  const recommended = recommendedTransferTuning(appState.sourceSummary);
  const auto = elements.autoTune.checked;

  if (auto) {
    return {
      chunkSize: recommended.chunkSize,
      parallelism: recommended.parallelism,
      mode: "auto",
      profile: recommended.profile,
      reason: recommended.reason,
    };
  }

  return {
    chunkSize: Math.max(1, Number(elements.chunkSize.value || recommended.chunkSize)),
    parallelism: Math.max(1, Number(elements.parallelism.value || recommended.parallelism)),
    mode: "manual",
    profile: recommended.profile,
    reason: recommended.reason,
  };
}

function renderTransferTuning() {
  const recommended = recommendedTransferTuning(appState.sourceSummary);
  const auto = elements.autoTune.checked;

  if (auto) {
    elements.chunkSize.value = String(recommended.chunkSize);
    elements.parallelism.value = String(recommended.parallelism);
  }

  elements.chunkSize.disabled = auto;
  elements.parallelism.disabled = auto;

  const chunkText = formatBytes(recommended.chunkSize);
  if (!appState.sourceSummary) {
    elements.autoTuneSummary.textContent = auto
      ? `Auto tune is active. Waiting for a selected package to pick chunk size and parallelism.`
      : `Manual mode is active. Recommended baseline is ${chunkText} chunks with parallelism ${recommended.parallelism}.`;
    return;
  }

  const profileText = tuningProfileLabel(recommended.profile);
  if (auto) {
    elements.autoTuneSummary.textContent = `Auto tune active (${profileText}): using ${chunkText} chunks, parallelism ${recommended.parallelism}. ${recommended.reason}`;
    return;
  }

  elements.autoTuneSummary.textContent = `Manual override active. Current values are ${formatBytes(Math.max(1, Number(elements.chunkSize.value || recommended.chunkSize)))} chunks and parallelism ${Math.max(1, Number(elements.parallelism.value || recommended.parallelism))}. Auto recommendation (${profileText}) is ${chunkText} / ${recommended.parallelism}.`;
}

function stateLabel(state) {
  if (state === "starting") return "Starting";
  if (state === "scanning") return "Scanning";
  if (state === "sending") return "Sending";
  if (state === "receiving") return "Receiving";
  if (state === "completed") return "Completed";
  if (state === "error") return "Error";
  if (state === "listening") return "Listening";
  if (state === "stopped") return "Stopped";
  if (state === "stopping") return "Stopping";
  return "Idle";
}

function stateKind(state) {
  if (state === "completed") return "success";
  if (state === "error") return "error";
  if (["starting", "scanning", "sending", "receiving", "listening", "stopping"].includes(state)) return "active";
  return "idle";
}

function statusChipClass(status) {
  const lower = String(status).toLowerCase();
  if (lower.includes("complete")) return "complete";
  if (["scanning", "sending", "receiving", "listening", "starting", "stopping"].includes(lower)) return lower;
  if (["queued", "idle"].includes(lower)) return lower;
  return "error";
}

function selectedDevice() {
  return appState.devices.find((device) => device.peerId === appState.selectedPeerId) ?? null;
}

function filteredDevices() {
  const search = appState.deviceSearch.trim().toLowerCase();
  if (!search) return appState.devices;
  return appState.devices.filter((device) => {
    const haystack = [
      device.deviceName,
      device.addresses?.join(" "),
      device.shortFingerprint,
      trustLabel(device.trustState),
    ]
      .filter(Boolean)
      .join(" ")
      .toLowerCase();
    return haystack.includes(search);
  });
}

function selectDefaultDevice() {
  const current = selectedDevice();
  if (current) return;
  const preferred = appState.devices.find((device) => device.trustState !== "fingerprint_mismatch");
  appState.selectedPeerId = preferred?.peerId ?? null;
}

function syncReceiveFolderDisplays(label, absolutePath = null) {
  appState.receiveOutputLabel = label || "Downloads/FastTransfer";
  if (absolutePath !== null) {
    appState.receiveOutputDir = absolutePath;
  }
  const display = simplifiedPathLabel(appState.receiveOutputLabel);
  elements.receiverOutputLabel.textContent = display;
  elements.receiverOutput.value = appState.receiveOutputDir ?? appState.receiveOutputLabel;
  elements.settingsReceiveFolder.value = appState.receiveOutputDir ?? appState.receiveOutputLabel;
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
    elements.packageRootName.textContent = "Pending";
    elements.packageRootKind.textContent = "Pending";
    elements.packageTotalFiles.textContent = "0";
    elements.packageTotalFolders.textContent = "0";
    elements.packageTotalBytes.textContent = "0 B";
    elements.packageKindBadge.textContent = "Nothing selected";
    elements.packageSummaryMessage.textContent = "Choose a file or folder to preview the package structure before sending.";
    elements.sendSummaryFile.textContent = "Waiting";
    elements.sendSummaryMeta.textContent = "Not started";
    return;
  }

  elements.packageRootName.textContent = summary.rootName;
  elements.packageRootKind.textContent = packageKindLabel(summary.rootKind);
  elements.packageTotalFiles.textContent = Number(summary.totalFiles).toLocaleString();
  elements.packageTotalFolders.textContent = Number(summary.totalDirectories).toLocaleString();
  elements.packageTotalBytes.textContent = formatBytes(summary.totalBytes);
  elements.packageKindBadge.textContent = packageKindBadgeLabel(summary.rootKind);
  elements.packageSummaryMessage.textContent = `Ready to send ${summary.rootName} with ${Number(summary.totalFiles).toLocaleString()} file${Number(summary.totalFiles) === 1 ? "" : "s"}.`;
  if (!appState.sendBusy) {
    elements.sendSummaryFile.textContent = summary.rootName;
    elements.sendSummaryMeta.textContent = `${formatBytes(summary.totalBytes)} - ${packageKindLabel(summary.rootKind)}`;
  }
}

async function setSourcePath(path) {
  elements.sourcePath.value = path;
  appState.sourceSummary = null;
  renderPackageSummary();
  renderTransferTuning();

  try {
    const summary = await invoke("inspect_source", { sourcePath: path });
    appState.sourceSummary = summary;
    renderAll();
  } catch (error) {
    appState.sourceSummary = null;
    renderAll();
    applySendStatus({ state: "error", message: `Failed to inspect source: ${error}` });
  }
}
function renderSelectedTarget() {
  if (elements.manualMode.checked) {
    const address = elements.targetAddress.value.trim();
    const hasCert = Boolean(elements.certificatePath.value);
    setPill(elements.sendTargetBadge, "Manual", address ? "active" : "idle");
    elements.sendTargetName.textContent = "Manual target";
    elements.sendTargetAddress.textContent = address || "Pending";
    elements.sendTargetMessage.textContent = "Manual target mode is enabled for direct receiver entry.";
    elements.sendTargetFingerprint.textContent = hasCert ? basename(elements.certificatePath.value) : "Pending";
    elements.sendTargetTrust.textContent = hasCert ? "Manual certificate" : "Awaiting certificate";
    elements.actionTargetName.textContent = address || "manual target";
    return;
  }

  const device = selectedDevice();
  if (!device) {
    setPill(elements.sendTargetBadge, "No target", "idle");
    elements.sendTargetName.textContent = "No device selected";
    elements.sendTargetAddress.textContent = "Pending";
    elements.sendTargetMessage.textContent = "Choose a nearby receiver or use manual mode.";
    elements.sendTargetFingerprint.textContent = "Pending";
    elements.sendTargetTrust.textContent = "Pending";
    elements.actionTargetName.textContent = "selected device";
    return;
  }

  setPill(elements.sendTargetBadge, trustLabel(device.trustState), trustKind(device.trustState));
  elements.sendTargetName.textContent = device.deviceName;
  elements.sendTargetAddress.textContent = device.addresses[0] ?? "Pending";
  elements.sendTargetMessage.textContent = device.trustMessage;
  elements.sendTargetFingerprint.textContent = device.shortFingerprint;
  elements.sendTargetTrust.textContent = trustLabel(device.trustState);
  elements.actionTargetName.textContent = device.deviceName;
}

function renderDeviceIdentity() {
  const device = selectedDevice();
  if (!device) {
    setPill(elements.deviceDetailBadge, "None", "idle");
    elements.deviceDetailMessage.textContent = "Select a nearby device to inspect its identity information.";
    elements.deviceDetailName.textContent = "Pending";
    elements.deviceDetailAddress.textContent = "Pending";
    elements.deviceDetailFingerprint.textContent = "Pending";
    elements.deviceDetailTrust.textContent = "Pending";
    return;
  }

  setPill(elements.deviceDetailBadge, trustLabel(device.trustState), trustKind(device.trustState));
  elements.deviceDetailMessage.textContent = device.trustMessage;
  elements.deviceDetailName.textContent = device.deviceName;
  elements.deviceDetailAddress.textContent = device.addresses[0] ?? "Pending";
  elements.deviceDetailFingerprint.textContent = device.shortFingerprint;
  elements.deviceDetailTrust.textContent = trustLabel(device.trustState);
}

function sendDeviceCardMarkup(device, index) {
  const selected = device.peerId === appState.selectedPeerId;
  const mismatch = device.trustState === "fingerprint_mismatch";
  return `
    <button class="device-list-card${selected ? " selected" : ""}" type="button" data-device-index="${index}" ${mismatch ? "disabled" : ""}>
      <div class="device-main">
        <div class="device-icon">${escapeHtml(deviceGlyph(device))}</div>
        <div>
          <div class="device-name">${escapeHtml(device.deviceName)}</div>
          <div class="device-address">${escapeHtml(device.addresses[0] ?? "No address")}</div>
          <div class="device-badges">
            <span class="pill pill-${trustKind(device.trustState)}">${escapeHtml(trustLabel(device.trustState))}</span>
            <span class="pill">${escapeHtml(device.shortFingerprint)}</span>
          </div>
        </div>
      </div>
      <span class="pill pill-${selected ? "active" : "idle"}">${selected ? "Selected" : "Use"}</span>
    </button>
  `;
}

function devicesGridMarkup(device, index) {
  const selected = device.peerId === appState.selectedPeerId;
  return `
    <button class="device-grid-card${selected ? " selected" : ""}" type="button" data-device-index="${index}">
      <div class="device-grid-top">
        <div class="device-main">
          <div class="device-icon">${escapeHtml(deviceGlyph(device))}</div>
          <div>
            <div class="device-name">${escapeHtml(device.deviceName)}</div>
            <div class="device-address">${escapeHtml(device.addresses[0] ?? "No address")}</div>
          </div>
        </div>
        <span class="pill pill-${trustKind(device.trustState)}">${escapeHtml(trustLabel(device.trustState))}</span>
      </div>
      <div class="device-grid-meta">
        <div>
          <span>Fingerprint</span>
          <strong>${escapeHtml(device.shortFingerprint)}</strong>
        </div>
        <div>
          <span>Transport</span>
          <strong>${escapeHtml(device.transport ?? "QUIC")}</strong>
        </div>
      </div>
    </button>
  `;
}

function bindDeviceButtons(container, sourceList) {
  for (const button of container.querySelectorAll("[data-device-index]")) {
    button.addEventListener("click", () => {
      const device = sourceList[Number(button.dataset.deviceIndex)];
      if (!device || device.trustState === "fingerprint_mismatch") return;
      appState.selectedPeerId = device.peerId;
      elements.manualMode.checked = false;
      renderAll();
    });
  }
}

function renderDeviceLists() {
  const visibleDevices = filteredDevices();
  if (!visibleDevices.length) {
    const empty = appState.devices.length
      ? "No devices match your search."
      : "No nearby receivers found. Start a receiver on the same LAN and refresh.";
    elements.sendDeviceList.className = "device-list empty-state";
    elements.sendDeviceList.textContent = empty;
    elements.devicesNearbyList.className = "device-grid empty-state";
    elements.devicesNearbyList.textContent = empty;
  } else {
    elements.sendDeviceList.className = "device-list";
    elements.sendDeviceList.innerHTML = visibleDevices.map(sendDeviceCardMarkup).join("");
    bindDeviceButtons(elements.sendDeviceList, visibleDevices);

    elements.devicesNearbyList.className = "device-grid";
    elements.devicesNearbyList.innerHTML = visibleDevices.map(devicesGridMarkup).join("");
    bindDeviceButtons(elements.devicesNearbyList, visibleDevices);
  }

  const trusted = appState.devices.filter((device) => device.trustState !== "unknown");
  if (!trusted.length) {
    elements.trustedDeviceList.className = "trusted-list empty-state";
    elements.trustedDeviceList.textContent = "No trusted devices yet.";
  } else {
    elements.trustedDeviceList.className = "trusted-list";
    elements.trustedDeviceList.innerHTML = trusted
      .map(
        (device) => `
          <div class="trusted-item">
            <strong>${escapeHtml(device.deviceName)}</strong>
            <span>${escapeHtml(trustLabel(device.trustState))}</span>
            <span>${escapeHtml(device.shortFingerprint)} - ${escapeHtml(device.addresses[0] ?? "No address")}</span>
          </div>
        `,
      )
      .join("");
  }
}

function renderRecentReceived() {
  if (!appState.recentReceived.length) {
    elements.recentReceivedList.className = "received-list empty-state";
    elements.recentReceivedList.textContent = "No received items yet.";
    return;
  }

  elements.recentReceivedList.className = "received-list";
  elements.recentReceivedList.innerHTML = appState.recentReceived
    .map(
      (item) => `
        <div class="received-item">
          <strong>${escapeHtml(item.name)}</strong>
          <span>${escapeHtml(item.when)}</span>
          <span>${escapeHtml(item.path)}</span>
        </div>
      `,
    )
    .join("");
}

function createTransfer(direction, kind, name, destination) {
  const id = `transfer-${++appState.transferCounter}`;
  appState.transfers.unshift({
    id,
    direction,
    kind,
    name,
    destination,
    status: "Idle",
    progress: "0%",
    speed: "0.00 MiB/s",
    updatedAt: Date.now(),
  });
  appState.transfers = appState.transfers.slice(0, 30);
  return id;
}

function updateTransfer(id, patch) {
  const transfer = appState.transfers.find((entry) => entry.id === id);
  if (!transfer) return;
  Object.assign(transfer, patch, { updatedAt: Date.now() });
}

function ensureTransfer(slotKey, direction, kind, name, destination) {
  let id = appState[slotKey];
  if (!id) {
    id = createTransfer(direction, kind, name, destination);
    appState[slotKey] = id;
  }
  return id;
}

function renderTransfers() {
  if (!appState.transfers.length) {
    elements.transfersTableBody.innerHTML = `
      <tr>
        <td colspan="6" class="table-empty">No transfer activity yet.</td>
      </tr>
    `;
    return;
  }

  elements.transfersTableBody.innerHTML = [...appState.transfers]
    .sort((a, b) => b.updatedAt - a.updatedAt)
    .map(
      (transfer) => `
        <tr>
          <td>${escapeHtml(transfer.name)}</td>
          <td>${escapeHtml(transfer.kind)}</td>
          <td>${escapeHtml(transfer.destination)}</td>
          <td><span class="status-chip ${statusChipClass(transfer.status)}">${escapeHtml(transfer.status)}</span></td>
          <td>${escapeHtml(transfer.progress)}</td>
          <td>${escapeHtml(transfer.speed)}</td>
        </tr>
      `,
    )
    .join("");
}

function setReceiverToggle() {
  const busy = ["starting", "stopping"].includes(appState.receiverState);
  elements.receiverToggle.disabled = busy;
  if (appState.receiverActive) {
    elements.receiverToggle.textContent = "Stop receiver";
    elements.receiverToggle.className = "danger-button";
  } else {
    elements.receiverToggle.textContent = "Start receiver";
    elements.receiverToggle.className = "primary-button";
  }
}

function renderNetworkState() {
  const discoveryEnabled = elements.settingsLanDiscovery.checked;
  if (!discoveryEnabled) {
    setPill(elements.networkBadge, "Offline", "idle");
    elements.networkSummary.textContent = "LAN discovery is disabled.";
    return;
  }

  setPill(elements.networkBadge, "Online", appState.devices.length ? "success" : "active");
  elements.networkSummary.textContent = `${appState.devices.length} device${appState.devices.length === 1 ? "" : "s"} found on this network`;
}

function syncOverflowTitles() {
  const textSelectors = [
    "#send-target-name",
    "#send-target-address",
    "#send-target-message",
    "#action-target-name",
    "#package-root-name",
    "#package-root-kind",
    "#package-summary-message",
    "#auto-tune-summary",
    "#send-summary-file",
    "#send-summary-meta",
    "#send-summary-hash",
    "#send-message",
    "#receiver-bind",
    "#receiver-message",
    "#receiver-file",
    "#receiver-saved-path",
    "#receiver-certificate",
    "#receiver-output-label",
    "#device-detail-name",
    "#device-detail-address",
    "#device-detail-fingerprint",
    "#device-detail-trust",
    "#status-left",
    "#status-center",
    "#status-right",
    "#network-summary",
    ".device-name",
    ".device-address",
    ".received-item span",
    ".trusted-item span",
    "#transfers-table-body td",
  ];

  for (const selector of textSelectors) {
    for (const node of document.querySelectorAll(selector)) {
      const text = (node.textContent ?? "").trim();
      if (text) {
        node.title = text;
      } else {
        node.removeAttribute("title");
      }
    }
  }

  const inputSelectors = ["#source-path", "#receiver-output", "#settings-receive-folder", "#certificate-path"];
  for (const selector of inputSelectors) {
    const node = document.querySelector(selector);
    if (!node) continue;
    const value = (node.value ?? "").trim();
    if (value) {
      node.title = value;
    } else {
      node.removeAttribute("title");
    }
  }
}

function renderSettings() {
  elements.settingsLanDiscovery.checked = true;
  elements.settingsAutoTrust.checked = true;
  elements.settingsBindAddress.value = "0.0.0.0:5000";
  if (!elements.settingsDiscoveryTimeout.value) {
    elements.settingsDiscoveryTimeout.value = "3";
  }
  if (!elements.settingsTheme.value) {
    elements.settingsTheme.value = "System (placeholder)";
  }
  syncReceiveFolderDisplays(appState.receiveOutputLabel, appState.receiveOutputDir);
}

function updateStatusBar() {
  const activeProgress = [...appState.transfers].sort((a, b) => b.updatedAt - a.updatedAt)[0];
  elements.statusLeft.textContent = `${appState.devices.length} device${appState.devices.length === 1 ? "" : "s"} on LAN`;
  elements.statusCenter.textContent = appState.receiverActive
    ? `Receiver listening on ${appState.receiverBindAddr}`
    : "Receiver stopped";
  elements.statusRight.textContent = activeProgress
    ? `${activeProgress.status} - ${activeProgress.speed}`
    : (elements.manualMode.checked ? (elements.targetAddress.value.trim() || "Manual target") : (selectedDevice()?.deviceName || "No target selected"));
}

function renderAll() {
  renderPackageSummary();
  renderTransferTuning();
  renderSelectedTarget();
  renderDeviceIdentity();
  renderDeviceLists();
  renderRecentReceived();
  renderTransfers();
  renderSettings();
  renderNetworkState();
  setReceiverToggle();
  updateStatusBar();
  syncOverflowTitles();
}

async function refreshDevices() {
  const timeout = Number(elements.settingsDiscoveryTimeout.value || 3);
  const discoveryEnabled = elements.settingsLanDiscovery.checked;
  elements.globalRefresh.disabled = true;
  elements.refreshDevicesSend.disabled = true;
  elements.refreshDevicesView.disabled = true;
  try {
    if (!discoveryEnabled) {
      appState.devices = [];
      appState.selectedPeerId = null;
      renderAll();
      return;
    }
    appState.devices = await invoke("discover_nearby_receivers", { timeoutSecs: timeout });
    selectDefaultDevice();
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
      syncReceiveFolderDisplays(simplifiedPathLabel(selected), selected);
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
    renderAll();
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
    applySendStatus({ state: "error", message: "Choose a file or folder before sending." });
    return;
  }

  if (elements.manualMode.checked) {
    if (!elements.targetAddress.value.trim() || !elements.certificatePath.value) {
      applySendStatus({ state: "error", message: "Manual mode requires both a receiver address and a receiver certificate." });
      return;
    }
  } else if (!appState.selectedPeerId) {
    applySendStatus({ state: "error", message: "Select a nearby receiver or enable manual mode." });
    return;
  }

  const tuning = activeTransferTuning();

  try {
    await invoke("start_send", {
      request: {
        sourcePath: elements.sourcePath.value,
        selectedPeerId: elements.manualMode.checked ? null : appState.selectedPeerId,
        targetAddr: elements.manualMode.checked ? elements.targetAddress.value.trim() : null,
        certificatePath: elements.manualMode.checked ? elements.certificatePath.value : null,
        serverName: elements.serverName.value.trim() || "fasttransfer.local",
        chunkSize: tuning.chunkSize,
        parallelism: tuning.parallelism,
      },
    });

    applySendStatus({
      state: "starting",
      message: elements.manualMode.checked
        ? `Connecting to ${elements.targetAddress.value.trim()}...`
        : `Connecting to ${selectedDevice()?.deviceName ?? "selected receiver"}...`,
    });
    switchView("send");
  } catch (error) {
    applySendStatus({ state: "error", message: `${error}` });
  }
}

function applyProgress(bar, textNode, speedNode, progress) {
  const percent = Number(progress?.percent ?? 0);
  bar.style.width = `${percent}%`;
  textNode.textContent = `${percent.toFixed(1)}%`;
  speedNode.textContent = `${Number(progress?.averageMibPerSec ?? 0).toFixed(2)} MiB/s`;
}

function applySendStatus(payload) {
  const state = payload.state ?? "idle";
  appState.sendBusy = ["starting", "scanning", "sending"].includes(state);
  appState.sendMessage = payload.message ?? "Ready.";

  elements.sendButton.disabled = appState.sendBusy;
  elements.sendMessage.textContent = appState.sendMessage;
  setPill(elements.sendBadge, stateLabel(state), stateKind(state));

  if (payload.progress) {
    applyProgress(elements.sendProgressBar, elements.sendProgressText, elements.sendSpeed, payload.progress);
    elements.sendSummaryFile.textContent = appState.sourceSummary?.rootName ?? basename(elements.sourcePath.value) ?? "Outgoing package";
    elements.sendSummaryMeta.textContent = `${formatBytes(payload.progress.totalBytes)} - ${selectedDevice()?.deviceName ?? (elements.targetAddress.value.trim() || "Manual target")}`;
    elements.sendSummaryBytes.textContent = `${formatBytes(payload.progress.transferredBytes)} / ${formatBytes(payload.progress.totalBytes)}`;
    elements.sendSummaryChunks.textContent = `${payload.progress.completedFiles} / ${payload.progress.totalFiles}`;
    elements.sendSummaryHash.textContent = payload.progress.currentPath || (payload.progress.phase === "scanning" ? "Scanning files..." : "Preparing files");
  }

  if (payload.summary) {
    elements.sendSummaryFile.textContent = payload.summary.fileName;
    elements.sendSummaryMeta.textContent = `${formatBytes(payload.summary.bytesTransferred)} - ${payload.summary.elapsedSecs.toFixed(1)}s`;
    elements.sendSummaryBytes.textContent = formatBytes(payload.summary.bytesTransferred);
    elements.sendSummaryChunks.textContent = `${payload.summary.completedFiles} file${payload.summary.completedFiles === 1 ? "" : "s"}`;
    elements.sendSummaryHash.textContent = payload.summary.integrityVerified ? "Verified" : payload.summary.sha256Hex;
    if (state === "completed") {
      elements.sendProgressBar.style.width = "100%";
      elements.sendProgressText.textContent = "100.0%";
      elements.sendSpeed.textContent = `${Number(payload.summary.averageMibPerSec ?? 0).toFixed(2)} MiB/s`;
    }
  }

  const transferName = appState.sourceSummary?.rootName || basename(elements.sourcePath.value) || "Outgoing package";
  const transferKind = appState.sourceSummary ? packageKindLabel(appState.sourceSummary.rootKind) : "Package";
  const destination = elements.manualMode.checked
    ? elements.targetAddress.value.trim() || "Manual target"
    : selectedDevice()?.deviceName || "Nearby device";

  if (["starting", "scanning", "sending", "completed", "error"].includes(state)) {
    const id = ensureTransfer("currentSendTransferId", "Send", transferKind, transferName, destination);
    updateTransfer(id, {
      name: transferName,
      kind: transferKind,
      destination,
      status: stateLabel(state),
      progress: payload.progress ? `${Number(payload.progress.percent ?? 0).toFixed(1)}%` : state === "completed" ? "100%" : "0%",
      speed: payload.progress
        ? `${Number(payload.progress.averageMibPerSec ?? 0).toFixed(2)} MiB/s`
        : payload.summary
          ? `${Number(payload.summary.averageMibPerSec ?? 0).toFixed(2)} MiB/s`
          : "0.00 MiB/s",
    });
    if (["completed", "error"].includes(state)) {
      appState.currentSendTransferId = null;
    }
  }

  renderTransfers();
  updateStatusBar();
  syncOverflowTitles();
}

function applyReceiverStatus(payload) {
  const state = payload.state ?? "idle";
  appState.receiverState = state;
  appState.receiverActive = ["starting", "listening", "receiving"].includes(state);
  appState.receiverMessage = payload.message ?? "Receiver stopped.";
  elements.receiverMessage.textContent = appState.receiverMessage;
  setPill(elements.receiverBadge, stateLabel(state), stateKind(state));

  if (payload.outputDirLabel || payload.outputDir) {
    syncReceiveFolderDisplays(payload.outputDirLabel ?? payload.outputDir, payload.outputDir ?? appState.receiveOutputDir);
  }
  if (payload.bindAddr) {
    appState.receiverBindAddr = payload.bindAddr;
  } else if (!appState.receiverActive && ["idle", "stopped", "error"].includes(state)) {
    appState.receiverBindAddr = "Not listening";
  }
  elements.receiverBind.textContent = appState.receiverBindAddr;

  if (payload.certificatePath) {
    appState.receiverCertificateName = basename(payload.certificatePath);
  }
  elements.receiverCertificate.textContent = appState.receiverCertificateName;

  if (payload.savedPathLabel || payload.savedPath) {
    elements.receiverSavedPath.textContent = payload.savedPathLabel ?? simplifiedPathLabel(payload.savedPath);
  }

  if (payload.progress) {
    applyProgress(elements.receiverProgressBar, elements.receiverProgressText, elements.receiverSpeed, payload.progress);
    elements.receiverFile.textContent = payload.progress.currentPath || "Incoming file";
    const destination = payload.outputDirLabel ?? payload.outputDir ?? appState.receiveOutputLabel;
    const id = ensureTransfer(
      "currentReceiveTransferId",
      "Receive",
      "Incoming",
      payload.progress.currentPath || "Incoming transfer",
      destination,
    );
    updateTransfer(id, {
      name: payload.progress.currentPath || "Incoming transfer",
      kind: "Incoming",
      destination,
      status: stateLabel(state),
      progress: `${Number(payload.progress.percent ?? 0).toFixed(1)}%`,
      speed: `${Number(payload.progress.averageMibPerSec ?? 0).toFixed(2)} MiB/s`,
    });
  }

  if (payload.summary) {
    const destination = payload.savedPathLabel ?? payload.savedPath ?? appState.receiveOutputLabel;
    const fileName = payload.summary.fileName;
    elements.receiverFile.textContent = fileName;
    elements.receiverSavedPath.textContent = destination;
    const id = ensureTransfer("currentReceiveTransferId", "Receive", "Received", fileName, destination);
    updateTransfer(id, {
      name: fileName,
      kind: "Received",
      destination,
      status: stateLabel(state),
      progress: state === "completed" ? "100%" : "Failed",
      speed: `${Number(payload.summary.averageMibPerSec ?? 0).toFixed(2)} MiB/s`,
    });
    appState.currentReceiveTransferId = null;

    if (state === "completed") {
      elements.receiverProgressBar.style.width = "100%";
      elements.receiverProgressText.textContent = "100.0%";
      elements.receiverSpeed.textContent = `${Number(payload.summary.averageMibPerSec ?? 0).toFixed(2)} MiB/s`;
      appState.recentReceived.unshift({
        name: fileName,
        when: formatTime(Date.now()),
        path: destination,
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
    appState.receiverActive = false;
  }

  if (["stopped", "idle"].includes(state)) {
    appState.receiverActive = false;
    elements.receiverProgressBar.style.width = "0%";
    elements.receiverProgressText.textContent = "0%";
    elements.receiverSpeed.textContent = "0.00 MiB/s";
  }

  setReceiverToggle();
  renderRecentReceived();
  renderTransfers();
  updateStatusBar();
  syncOverflowTitles();
}

function wireEvents() {
  for (const button of elements.navItems) {
    button.addEventListener("click", () => switchView(button.dataset.view));
  }

  elements.globalRefresh.addEventListener("click", refreshDevices);
  elements.refreshDevicesSend.addEventListener("click", refreshDevices);
  elements.refreshDevicesView.addEventListener("click", refreshDevices);
  elements.newTransfer.addEventListener("click", () => switchView("send"));

  elements.deviceSearch.addEventListener("input", () => {
    appState.deviceSearch = elements.deviceSearch.value;
    renderDeviceLists();
  });

  elements.pickSourceFile.addEventListener("click", async () => {
    const selected = await invoke("pick_source_file");
    if (selected) {
      await setSourcePath(selected);
      switchView("send");
    }
  });

  elements.pickSourceFolder.addEventListener("click", async () => {
    const selected = await invoke("pick_source_folder");
    if (selected) {
      await setSourcePath(selected);
      switchView("send");
    }
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
  elements.autoTune.addEventListener("change", renderTransferTuning);
  elements.chunkSize.addEventListener("input", () => {
    if (!elements.autoTune.checked) {
      renderTransferTuning();
    }
  });
  elements.parallelism.addEventListener("input", () => {
    if (!elements.autoTune.checked) {
      renderTransferTuning();
    }
  });

  elements.sendButton.addEventListener("click", startSend);
  elements.receiverToggle.addEventListener("click", toggleReceiver);
  elements.browseOutput.addEventListener("click", browseReceiveOutput);
  elements.settingsBrowseOutput.addEventListener("click", browseReceiveOutput);
  elements.settingsLanDiscovery.addEventListener("change", refreshDevices);
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






