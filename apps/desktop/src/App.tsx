import { useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  DEFAULT_SERVER_NAME,
  discoverNearbyReceivers,
  inspectSource,
  isTauriRuntime,
  listRemovableDrives,
  listenReceiverStatus,
  listenSendStatus,
  pickCertificateFile,
  pickLocalDestinationFolder,
  pickReceiveFolder,
  pickSourceFiles,
  pickSourceFolder,
  startLocalCopyTransfer,
  startReceiver as startReceiverCommand,
  startSend as startSendCommand,
  stopReceiver as stopReceiverCommand,
  stopSend as stopSendCommand,
  pauseSend as pauseSendCommand,
  resumeSend as resumeSendCommand,
  type PackageSummary,
  type ReceiverListItem,
  type ReceiverStartPayload,
  type ReceiverStatusPayload,
  type RemovableDrive,
  type SendStatusPayload,
  type TauriUnlisten,
} from "./lib/tauriClient";

type DestinationMode = "nearby" | "manual" | "usb" | "local";
type WorkspaceTab = "send" | "receive" | "history" | "settings";
type ThemePreference = "system" | "light" | "dark";
type SourcePickerMode = "files" | "folder";

const DESTINATION_MODES: Array<{ key: DestinationMode; label: string }> = [
  { key: "nearby", label: "Nearby Device" },
  { key: "manual", label: "Manual Address" },
  { key: "usb", label: "USB / External Drive" },
  { key: "local", label: "Local Folder" },
];

const WORKSPACE_TABS: Array<{ key: WorkspaceTab; label: string }> = [
  { key: "send", label: "Send" },
  { key: "receive", label: "Receive" },
  { key: "history", label: "History" },
  { key: "settings", label: "Settings" },
];
const WORKSPACE_CONTEXT: Record<
  WorkspaceTab,
  { subtitle: string; hint: string }
> = {
  send: {
    subtitle: "Prepare outgoing transfers and choose how files are sent.",
    hint: "Tip: choose a destination mode first, then review source and send.",
  },
  receive: {
    subtitle: "Accept incoming transfers and monitor receiver health.",
    hint: "Tip: keep receiver running when you expect repeated sends.",
  },
  history: {
    subtitle: "Review completed, stopped, and failed transfers.",
    hint: "Tip: use Reuse Settings on a send entry to restart quickly.",
  },
  settings: {
    subtitle: "Manage defaults for transfer behavior and receiver identity.",
    hint: "Tip: set defaults once, then stay in Send for daily use.",
  },
};
const DESTINATION_MODE_HINTS: Record<DestinationMode, string> = {
  nearby: "Best for LAN transfers between trusted devices.",
  manual: "Use direct address + certificate for explicit targeting.",
  usb: "Copy to removable drives and external storage.",
  local: "Copy to folders on this computer.",
};

type TransferDirection = "send" | "receive";
type TransferResult = "completed" | "completed_with_issues" | "error" | "stopped";

type TransferIssueRecord = {
  path: string;
  message: string;
};

type TransferHistoryEntry = {
  id: string;
  createdAt: string;
  direction: TransferDirection;
  result: TransferResult;
  message: string;
  sourcePath?: string;
  destinationMode?: DestinationMode;
  destinationPath?: string;
  selectedPeerId?: string;
  targetAddr?: string;
  certificatePath?: string;
  chunkSize?: number;
  parallelism?: number;
  bytesTransferred?: number;
  elapsedSecs?: number;
  averageMibPerSec?: number;
  totalFiles?: number;
  failedFiles?: number;
  integrityVerified?: boolean;
  issues?: TransferIssueRecord[];
};

type SendAttemptContext = {
  attemptId: string;
  startedAt: number;
  sourcePath: string;
  destinationMode: DestinationMode;
  destinationPath?: string;
  selectedPeerId?: string;
  targetAddr?: string;
  certificatePath?: string;
  chunkSize: number;
  parallelism: number;
};

type BatchSendPlan = {
  sources: string[];
  currentIndex: number;
  destinationMode: DestinationMode;
  destinationPath?: string;
  selectedPeerId?: string;
  targetAddr?: string;
  certificatePath?: string;
  serverName: string;
  chunkSize: number;
  parallelism: number;
};

const HISTORY_STORAGE_KEY = "fasttransfer.desktop.history.v1";
const THEME_STORAGE_KEY = "fasttransfer.desktop.theme.v1";
const MAX_HISTORY_ENTRIES = 40;
const DRIVE_SCAN_TIMEOUT_MS = 6000;
const FILE_PICKER_TIMEOUT_MS = 20000;
const SEND_START_RETRY_ATTEMPTS = 10;
const SEND_START_RETRY_DELAY_MS = 120;

function asErrorMessage(error: unknown): string {
  if (error instanceof Error && error.message.trim().length > 0) {
    return error.message;
  }

  if (typeof error === "string" && error.trim().length > 0) {
    return error;
  }

  return "Unexpected error.";
}

function toOptionalText(value: string): string | undefined {
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : undefined;
}

function toPositiveInteger(value: string): number | undefined {
  const parsed = Number.parseInt(value, 10);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    return undefined;
  }
  return parsed;
}
async function withTimeout<T>(
  promise: Promise<T>,
  timeoutMs: number,
  timeoutMessage: string
): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = window.setTimeout(() => {
      reject(new Error(timeoutMessage));
    }, timeoutMs);

    promise
      .then((value) => {
        window.clearTimeout(timer);
        resolve(value);
      })
      .catch((error) => {
        window.clearTimeout(timer);
        reject(error);
      });
  });
}

function isSenderBusyError(error: unknown): boolean {
  return asErrorMessage(error).toLowerCase().includes("already in progress");
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => {
    window.setTimeout(resolve, ms);
  });
}

async function invokeWithSendStartRetry<T>(operation: () => Promise<T>): Promise<T> {
  let lastError: unknown;

  for (let attempt = 0; attempt < SEND_START_RETRY_ATTEMPTS; attempt += 1) {
    try {
      return await operation();
    } catch (error) {
      lastError = error;
      if (!isSenderBusyError(error) || attempt + 1 >= SEND_START_RETRY_ATTEMPTS) {
        throw error;
      }
      await sleep(SEND_START_RETRY_DELAY_MS);
    }
  }

  throw lastError ?? new Error("Could not start transfer.");
}
function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return "0 B";
  }

  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let unitIndex = 0;
  let value = bytes;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }

  if (unitIndex === 0) {
    return `${Math.round(value)} ${units[unitIndex]}`;
  }

  return `${value.toFixed(2)} ${units[unitIndex]}`;
}

function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) {
    return "< 1s";
  }

  const roundedSeconds = Math.max(1, Math.round(seconds));
  const hours = Math.floor(roundedSeconds / 3600);
  const minutes = Math.floor((roundedSeconds % 3600) / 60);
  const remainingSeconds = roundedSeconds % 60;

  if (hours > 0) {
    return `${hours}h ${minutes}m`;
  }

  if (minutes > 0) {
    return `${minutes}m ${remainingSeconds}s`;
  }

  return `${remainingSeconds}s`;
}
function readThemePreference(): ThemePreference {
  if (typeof window === "undefined") {
    return "system";
  }

  try {
    const stored = window.localStorage.getItem(THEME_STORAGE_KEY);
    if (stored === "light" || stored === "dark" || stored === "system") {
      return stored;
    }
  } catch {
    // ignore storage read failures
  }

  return "system";
}

function readSystemPrefersDark(): boolean {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
    return false;
  }

  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}
function stateChipClasses(state: string): string {
  if (state === "completed" || state === "listening") {
    return "bg-accentSoft text-accent";
  }

  if (state === "completed_with_issues") {
    return "bg-warningSoft text-warningInk";
  }

  if (state === "error") {
    return "bg-dangerSoft text-dangerInk";
  }

  if (state === "paused" || state === "stopped") {
    return "bg-warningSoft text-warningInk";
  }

  if (state === "receiving" || state === "sending") {
    return "bg-infoSoft text-infoInk";
  }

  return "bg-neutralSoft text-neutralInk";
}
function App() {
  const tauriReady = useMemo(() => isTauriRuntime(), []);

  const [sourcePaths, setSourcePaths] = useState<string[]>([]);
  const [sourceSummary, setSourceSummary] = useState<PackageSummary | null>(null);
  const [inspectBusy, setInspectBusy] = useState(false);

  const [destinationMode, setDestinationMode] = useState<DestinationMode>("nearby");
  const [activeWorkspace, setActiveWorkspace] = useState<WorkspaceTab>("send");
  const [themePreference, setThemePreference] = useState<ThemePreference>(() => readThemePreference());
  const [systemPrefersDark, setSystemPrefersDark] = useState(() => readSystemPrefersDark());
  const resolvedTheme = themePreference === "system" ? (systemPrefersDark ? "dark" : "light") : themePreference;
  const [showAdvancedSend, setShowAdvancedSend] = useState(false);
  const [sourcePickerMode, setSourcePickerMode] = useState<SourcePickerMode>("files");
  const [sourcePickerMenuOpen, setSourcePickerMenuOpen] = useState(false);
  const [dragDropActive, setDragDropActive] = useState(false);
  const [dropZoneActive, setDropZoneActive] = useState(false);

  const [receivers, setReceivers] = useState<ReceiverListItem[]>([]);
  const [selectedPeerId, setSelectedPeerId] = useState("");
  const [receiversBusy, setReceiversBusy] = useState(false);
  const [receiverQuery, setReceiverQuery] = useState("");

  const [targetAddr, setTargetAddr] = useState("");
  const [certificatePath, setCertificatePath] = useState("");
  const [serverName, setServerName] = useState(DEFAULT_SERVER_NAME);

  const [drives, setDrives] = useState<RemovableDrive[]>([]);
  const [drivesBusy, setDrivesBusy] = useState(false);
  const [selectedDriveId, setSelectedDriveId] = useState("");
  const [localDestinationPath, setLocalDestinationPath] = useState("");

  const [chunkSize, setChunkSize] = useState("1048576");
  const [parallelism, setParallelism] = useState("4");

  const [sendBusy, setSendBusy] = useState(false);
  const [sendStatus, setSendStatus] = useState<SendStatusPayload>({
    state: "idle",
    message: "Select a source and destination mode to begin.",
  });

  const [receiverDeviceName, setReceiverDeviceName] = useState("");
  const [receiverOutputDir, setReceiverOutputDir] = useState("");
  const [receiverBusy, setReceiverBusy] = useState(false);
  const [receiverStatus, setReceiverStatus] = useState<ReceiverStatusPayload>({
    state: "idle",
    message: "Receiver is idle.",
  });

  const [uiError, setUiError] = useState("");
  const [uiNote, setUiNote] = useState(
    tauriReady
      ? "Connected"
      : "Run this screen through `npm run tauri dev` to enable native commands."
  );

  const [historyEntries, setHistoryEntries] = useState<TransferHistoryEntry[]>([]);
  const activeSendAttemptRef = useRef<SendAttemptContext | null>(null);
  const sendPausedRef = useRef(false);
  const [sendPaused, setSendPaused] = useState(false);
  const batchSendPlanRef = useRef<BatchSendPlan | null>(null);
  const [batchProgress, setBatchProgress] = useState<{ current: number; total: number } | null>(null);
  const lastReceiverTerminalKeyRef = useRef("");
  const workspaceTopRef = useRef<HTMLDivElement | null>(null);
  const didMountWorkspaceRef = useRef(false);
  const sourcePickerMenuRef = useRef<HTMLDivElement | null>(null);
  const sourceDropZoneRef = useRef<HTMLDivElement | null>(null);

  function updateSendPaused(value: boolean) {
    sendPausedRef.current = value;
    setSendPaused(value);
  }

  function appendHistoryEntry(entry: Omit<TransferHistoryEntry, "id" | "createdAt">) {
    const resolvedEntry: TransferHistoryEntry = {
      id: `${Date.now()}-${Math.random().toString(16).slice(2, 10)}`,
      createdAt: new Date().toISOString(),
      ...entry,
    };

    setHistoryEntries((previous) => [resolvedEntry, ...previous].slice(0, MAX_HISTORY_ENTRIES));
  }

  function clearHistory() {
    setHistoryEntries([]);
    setUiNote("Transfer history cleared.");
  }

  function applyHistorySettings(entry: TransferHistoryEntry) {
    if (entry.direction !== "send") {
      return;
    }

    if (entry.sourcePath) {
      setSourcePaths([entry.sourcePath]);
      setSourcePickerMode("files");
      if (tauriReady) {
        void inspectSelectedSource(entry.sourcePath);
      }
    }

    if (entry.destinationMode) {
      setDestinationMode(entry.destinationMode);
    }

    if (entry.destinationPath) {
      if (entry.destinationMode === "local") {
        setLocalDestinationPath(entry.destinationPath);
      }

      if (entry.destinationMode === "usb") {
        const match = drives.find((drive) => drive.mountPath === entry.destinationPath);
        if (match) {
          setSelectedDriveId(match.id);
        }
      }
    }

    if (entry.selectedPeerId) {
      setSelectedPeerId(entry.selectedPeerId);
    }

    if (entry.targetAddr) {
      setTargetAddr(entry.targetAddr);
    }

    if (entry.certificatePath) {
      setCertificatePath(entry.certificatePath);
    }

    if (entry.chunkSize) {
      setChunkSize(String(entry.chunkSize));
    }

    if (entry.parallelism) {
      setParallelism(String(entry.parallelism));
    }

    setUiNote("Loaded transfer settings from history.");
  }

  useEffect(() => {
    if (typeof window === "undefined") {
      return;
    }

    try {
      const raw = window.localStorage.getItem(HISTORY_STORAGE_KEY);
      if (!raw) {
        return;
      }

      const parsed = JSON.parse(raw);
      if (!Array.isArray(parsed)) {
        return;
      }

      setHistoryEntries(parsed.slice(0, MAX_HISTORY_ENTRIES));
    } catch {
      // ignore malformed history cache
    }
  }, []);

  useEffect(() => {
    if (typeof window === "undefined") {
      return;
    }

    try {
      window.localStorage.setItem(HISTORY_STORAGE_KEY, JSON.stringify(historyEntries));
    } catch {
      // ignore storage write failures
    }
  }, [historyEntries]);

  useEffect(() => {
    if (typeof document === "undefined") {
      return;
    }

    const handlePointerDown = (event: MouseEvent) => {
      if (!sourcePickerMenuRef.current) {
        return;
      }

      if (!sourcePickerMenuRef.current.contains(event.target as Node)) {
        setSourcePickerMenuOpen(false);
      }
    };

    document.addEventListener("mousedown", handlePointerDown);
    return () => {
      document.removeEventListener("mousedown", handlePointerDown);
    };
  }, []);

  useEffect(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
      return;
    }

    const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
    const handleChange = (event: MediaQueryListEvent) => {
      setSystemPrefersDark(event.matches);
    };

    setSystemPrefersDark(mediaQuery.matches);

    if (typeof mediaQuery.addEventListener === "function") {
      mediaQuery.addEventListener("change", handleChange);
      return () => mediaQuery.removeEventListener("change", handleChange);
    }

    mediaQuery.addListener(handleChange);
    return () => mediaQuery.removeListener(handleChange);
  }, []);

  useEffect(() => {
    if (typeof window === "undefined") {
      return;
    }

    try {
      window.localStorage.setItem(THEME_STORAGE_KEY, themePreference);
    } catch {
      // ignore storage write failures
    }
  }, [themePreference]);

  useEffect(() => {
    if (typeof document === "undefined") {
      return;
    }

    document.documentElement.dataset.theme = resolvedTheme;
    document.documentElement.style.colorScheme = resolvedTheme;
  }, [resolvedTheme]);

  useEffect(() => {
    if (!didMountWorkspaceRef.current) {
      didMountWorkspaceRef.current = true;
      return;
    }

    workspaceTopRef.current?.scrollIntoView({
      behavior: "smooth",
      block: "start",
    });
  }, [activeWorkspace]);

  useEffect(() => {
    if (!tauriReady) {
      return;
    }

    let unlistenDragDrop: TauriUnlisten | null = null;

    const setup = async () => {
      unlistenDragDrop = await getCurrentWindow().onDragDropEvent((event) => {
        const payload = event.payload;

        if (payload.type === "leave") {
          setDragDropActive(false);
          setDropZoneActive(false);
          return;
        }

        if (payload.type === "enter") {
          setDragDropActive(true);
          setDropZoneActive(isInsideSourceDropZone(payload.position));
          return;
        }

        if (payload.type === "over") {
          setDropZoneActive(isInsideSourceDropZone(payload.position));
          return;
        }

        const insideDropZone = isInsideSourceDropZone(payload.position);
        setDragDropActive(false);
        setDropZoneActive(false);

        if (!insideDropZone || inspectBusy || sendBusy) {
          return;
        }

        void appendQueuedSources(payload.paths, "drop");
      });
    };

    void setup();

    return () => {
      setDragDropActive(false);
      setDropZoneActive(false);
      if (unlistenDragDrop) {
        void unlistenDragDrop();
      }
    };
  }, [appendQueuedSources, inspectBusy, sendBusy, tauriReady]);

  useEffect(() => {
    if (!tauriReady) {
      return;
    }

    let unlistenSend: TauriUnlisten | null = null;
    let unlistenReceiver: TauriUnlisten | null = null;

    const setup = async () => {
      unlistenSend = await listenSendStatus((payload) => {
        const activeProgressStates = new Set(["starting", "scanning", "sending"]);
        const terminalStates = new Set(["completed", "completed_with_issues", "error", "stopped", "idle"]);

        if (payload.state === "paused") {
          updateSendPaused(true);
          setSendStatus(payload);
          return;
        }

        if (sendPausedRef.current && activeProgressStates.has(payload.state)) {
          return;
        }

        if (terminalStates.has(payload.state)) {
          updateSendPaused(false);
        }

        setSendStatus(payload);

        if (
          payload.state === "completed" ||
          payload.state === "completed_with_issues" ||
          payload.state === "error" ||
          payload.state === "stopped"
        ) {
          const attempt = activeSendAttemptRef.current;
          if (attempt) {
            const result: TransferResult =
              payload.state === "completed"
                ? "completed"
                : payload.state === "completed_with_issues"
                ? "completed_with_issues"
                : payload.state === "stopped"
                ? "stopped"
                : "error";

            appendHistoryEntry({
              direction: "send",
              result,
              message: payload.message,
              sourcePath: attempt.sourcePath,
              destinationMode: attempt.destinationMode,
              destinationPath: attempt.destinationPath,
              selectedPeerId: attempt.selectedPeerId,
              targetAddr: attempt.targetAddr,
              certificatePath: attempt.certificatePath,
              chunkSize: attempt.chunkSize,
              parallelism: attempt.parallelism,
              bytesTransferred:
                payload.summary?.bytesTransferred ?? payload.progress?.transferredBytes,
              elapsedSecs: payload.summary?.elapsedSecs,
              averageMibPerSec:
                payload.summary?.averageMibPerSec ?? payload.progress?.averageMibPerSec,
              totalFiles: payload.summary?.totalFiles,
              failedFiles: payload.summary?.failedFiles,
              integrityVerified: payload.summary?.integrityVerified,
              issues: payload.summary?.issues?.map((issue) => ({
                path: issue.path,
                message: issue.message,
              })),
            });
          }

          const activePlan = batchSendPlanRef.current;
          if (
            (payload.state === "completed" || payload.state === "completed_with_issues") &&
            activePlan &&
            activePlan.currentIndex + 1 < activePlan.sources.length
          ) {
            activePlan.currentIndex += 1;
            setBatchProgress({ current: activePlan.currentIndex + 1, total: activePlan.sources.length });
            const nextSource = activePlan.sources[activePlan.currentIndex];

            activeSendAttemptRef.current = null;
            void startTransferForSource(nextSource, activePlan).catch((error) => {
              const message = `Could not continue batch transfer: ${asErrorMessage(error)}`;
              setUiError(message);
              setSendBusy(false);
              setBatchProgress(null);
              batchSendPlanRef.current = null;
              activeSendAttemptRef.current = null;
              updateSendPaused(false);
              setSendStatus({
                state: "error",
                message,
              });
            });
            return;
          }

          setSendBusy(false);
          setBatchProgress(null);
          batchSendPlanRef.current = null;
          activeSendAttemptRef.current = null;
        }
      });

      unlistenReceiver = await listenReceiverStatus((payload) => {
        setReceiverStatus(payload);

        const activeStates = new Set(["starting", "listening", "receiving"]);
        const terminalStates = new Set(["completed", "completed_with_issues", "error", "stopped", "idle"]);
        if (activeStates.has(payload.state)) {
          setReceiverBusy(true);
        }
        if (terminalStates.has(payload.state)) {
          setReceiverBusy(false);
        }

        if (payload.state === "completed" || payload.state === "completed_with_issues" || payload.state === "error") {
          const receiverKey = [
            payload.state,
            payload.message,
            payload.savedPath ?? "",
            payload.summary?.sha256Hex ?? "",
            payload.summary?.bytesTransferred ?? payload.progress?.transferredBytes ?? 0,
          ].join("|");

          if (lastReceiverTerminalKeyRef.current !== receiverKey) {
            appendHistoryEntry({
              direction: "receive",
              result: payload.state as TransferResult,
              message: payload.message,
              sourcePath: payload.remoteAddress ?? undefined,
              destinationPath: payload.savedPath ?? payload.outputDir ?? undefined,
              bytesTransferred:
                payload.summary?.bytesTransferred ?? payload.progress?.transferredBytes,
              elapsedSecs: payload.summary?.elapsedSecs,
              averageMibPerSec:
                payload.summary?.averageMibPerSec ?? payload.progress?.averageMibPerSec,
              totalFiles: payload.summary?.totalFiles,
              failedFiles: payload.summary?.failedFiles,
              integrityVerified: payload.summary?.integrityVerified,
              issues: payload.summary?.issues?.map((issue) => ({
                path: issue.path,
                message: issue.message,
              })),
            });
            lastReceiverTerminalKeyRef.current = receiverKey;
          }
        }
      });
    };

    void setup();

    return () => {
      if (unlistenSend) {
        void unlistenSend();
      }
      if (unlistenReceiver) {
        void unlistenReceiver();
      }
    };
  }, [tauriReady]);

  const selectedReceiver = receivers.find((item) => item.peerId === selectedPeerId) ?? null;
  const selectedDrive = drives.find((item) => item.id === selectedDriveId) ?? null;
  const activeWorkspaceLabel =
    WORKSPACE_TABS.find((workspace) => workspace.key === activeWorkspace)?.label ?? "Send";
  const activeWorkspaceContext = WORKSPACE_CONTEXT[activeWorkspace];
  const activeDestinationLabel =
    DESTINATION_MODES.find((mode) => mode.key === destinationMode)?.label ?? "Nearby Device";
  const filteredReceivers = useMemo(() => {
    const query = receiverQuery.trim().toLowerCase();
    if (!query) {
      return receivers;
    }

    return receivers.filter((receiver) => {
      const haystack = [
        receiver.deviceName,
        receiver.peerId,
        receiver.shortFingerprint,
        receiver.trustMessage,
        receiver.addresses.join(" "),
      ]
        .join(" ")
        .toLowerCase();

      return haystack.includes(query);
    });
  }, [receivers, receiverQuery]);

  const sendPercent = Math.max(
    0,
    Math.min(100, sendStatus.progress?.percent ?? (["completed", "completed_with_issues"].includes(sendStatus.state) ? 100 : 0))
  );
  const sendRemainingBytes = sendStatus.progress
    ? Math.max(0, sendStatus.progress.totalBytes - sendStatus.progress.transferredBytes)
    : 0;
  const sendEtaSeconds =
    sendStatus.progress && sendStatus.progress.averageMibPerSec > 0
      ? sendRemainingBytes / (sendStatus.progress.averageMibPerSec * 1024 * 1024)
      : undefined;
  const sendStatusMeta = sendPaused
    ? `Paused at ${sendPercent.toFixed(2)}%`
    : sendStatus.state === "sending" && typeof sendEtaSeconds === "number"
    ? `ETA ${formatDuration(sendEtaSeconds)}`
    : null;

  const receiverPercent = Math.max(
    0,
    Math.min(100, receiverStatus.progress?.percent ?? (["completed", "completed_with_issues"].includes(receiverStatus.state) ? 100 : 0))
  );
  const sendIsActive =
    sendBusy && !["completed", "completed_with_issues", "error", "stopped", "idle"].includes(sendStatus.state);
  const receiverIsActive =
    receiverBusy && !["completed", "completed_with_issues", "error", "stopped", "idle"].includes(receiverStatus.state);
  const transferBarState = sendIsActive
    ? sendPaused
      ? "paused"
      : sendStatus.state
    : receiverIsActive
    ? receiverStatus.state
    : "idle";
  const transferBarTitle = sendIsActive
    ? "Send in progress"
    : receiverIsActive
    ? "Receiver active"
    : "No active transfer";
  const transferBarMessage = sendIsActive
    ? sendStatus.message
    : receiverIsActive
    ? receiverStatus.message
    : "Start sending or start receiver to see live transfer details.";
  const transferBarPercent = sendIsActive ? sendPercent : receiverIsActive ? receiverPercent : 0;
  const transferBarMeta = sendIsActive
    ? sendStatusMeta ??
      (sendStatus.progress
        ? `${sendStatus.progress.averageMibPerSec.toFixed(2)} MiB/s`
        : null)
    : receiverIsActive && receiverStatus.progress
    ? `${receiverStatus.progress.averageMibPerSec.toFixed(2)} MiB/s`
    : null;
  const transferBarCurrentPath = sendIsActive
    ? sendStatus.progress?.currentPath
    : receiverIsActive
    ? receiverStatus.progress?.currentPath
    : null;
  const recentHistoryEntries = historyEntries.slice(0, 3);
  async function inspectSelectedSource(path: string) {
    if (!tauriReady) {
      return;
    }

    setInspectBusy(true);
    setUiError("");

    try {
      const summary = await inspectSource(path);
      setSourceSummary(summary);
      setChunkSize(String(summary.recommendedChunkSize));
      setParallelism(String(summary.recommendedParallelism));
      setUiNote(`Loaded package summary for ${summary.rootName}.`);
    } catch (error) {
      setSourceSummary(null);
      setUiError(`Failed to inspect source: ${asErrorMessage(error)}`);
    } finally {
      setInspectBusy(false);
    }
  }

  function normalizeUniquePaths(paths: string[]): string[] {
    const seen = new Set<string>();
    const ordered: string[] = [];

    for (const path of paths) {
      const trimmed = path.trim();
      if (!trimmed) {
        continue;
      }

      const key = trimmed.toLowerCase();
      if (seen.has(key)) {
        continue;
      }

      seen.add(key);
      ordered.push(trimmed);
    }

    return ordered;
  }

  async function appendQueuedSources(paths: string[], origin: "picker" | "drop" = "picker") {
    const cleaned = normalizeUniquePaths(paths);
    if (cleaned.length === 0) {
      return;
    }

    const merged = normalizeUniquePaths([...sourcePaths, ...cleaned]);
    setSourcePaths(merged);

    if (merged.length === 1) {
      await inspectSelectedSource(merged[0]);
      return;
    }

    setSourceSummary(null);
    setUiNote(
      origin === "drop"
        ? `Added ${cleaned.length} item${cleaned.length === 1 ? "" : "s"} to the transfer queue.`
        : `Queued ${merged.length} source paths for batch send.`
    );
  }

  function isInsideSourceDropZone(position: { x: number; y: number }) {
    if (!sourceDropZoneRef.current || typeof window === "undefined") {
      return false;
    }

    const rect = sourceDropZoneRef.current.getBoundingClientRect();
    const scale = window.devicePixelRatio || 1;
    const x = position.x / scale;
    const y = position.y / scale;

    return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
  }

  async function triggerSourcePicker(mode: SourcePickerMode = sourcePickerMode) {
    if (!tauriReady) {
      setUiError("Tauri runtime not available.");
      return;
    }

    setSourcePickerMode(mode);
    setSourcePickerMenuOpen(false);

    if (mode === "files") {
      await addSourceFiles();
      return;
    }

    setUiError("");

    try {
      const selectedPath = await withTimeout(
        pickSourceFolder(),
        FILE_PICKER_TIMEOUT_MS,
        "Source picker timed out. Try again or paste a path manually."
      );
      if (!selectedPath) {
        return;
      }

      await appendQueuedSources([selectedPath]);
    } catch (error) {
      setUiError(`Could not choose source: ${asErrorMessage(error)}`);
    }
  }

  async function addSourceFiles() {
    if (!tauriReady) {
      setUiError("Tauri runtime not available.");
      return;
    }

    setUiError("");

    try {
      const selectedPaths = await withTimeout(
        pickSourceFiles(),
        FILE_PICKER_TIMEOUT_MS,
        "Source picker timed out. Try again or paste a path manually."
      );

      if (selectedPaths.length === 0) {
        return;
      }

      await appendQueuedSources(selectedPaths);
    } catch (error) {
      setUiError(`Could not add files: ${asErrorMessage(error)}`);
    }
  }

  function removeSourceAt(index: number) {
    setSourcePaths((previous) => {
      const next = previous.filter((_, itemIndex) => itemIndex !== index);
      if (next.length === 1 && tauriReady) {
        void inspectSelectedSource(next[0]);
      } else if (next.length !== 1) {
        setSourceSummary(null);
      }
      return next;
    });
  }

  function clearSources() {
    setSourcePaths([]);
    setSourceSummary(null);
  }
  async function refreshReceivers() {
    if (!tauriReady) {
      setUiError("Tauri runtime not available.");
      return;
    }

    setReceiversBusy(true);
    setUiError("");

    try {
      const items = await discoverNearbyReceivers(4);
      setReceivers(items);

      if (items.length > 0) {
        if (!items.some((item) => item.peerId === selectedPeerId)) {
          setSelectedPeerId(items[0].peerId);
        }
        setUiNote(`Found ${items.length} nearby receiver(s).`);
      } else {
        setSelectedPeerId("");
        setUiNote("No nearby receivers found on the current LAN.");
      }
    } catch (error) {
      setUiError(`Receiver discovery failed: ${asErrorMessage(error)}`);
    } finally {
      setReceiversBusy(false);
    }
  }

  async function pickCertificate() {
    if (!tauriReady) {
      setUiError("Tauri runtime not available.");
      return;
    }

    setUiError("");

    try {
      const path = await withTimeout(
        pickCertificateFile(),
        FILE_PICKER_TIMEOUT_MS,
        "Certificate picker timed out. Try again or paste the certificate path manually."
      );
      if (path) {
        setCertificatePath(path);
      }
    } catch (error) {
      setUiError(`Could not choose certificate: ${asErrorMessage(error)}`);
    }
  }

  async function refreshDrives() {
    if (!tauriReady) {
      setUiError("Tauri runtime not available.");
      return;
    }

    setDrivesBusy(true);
    setUiError("");

    try {
      const available = await withTimeout(
        listRemovableDrives(),
        DRIVE_SCAN_TIMEOUT_MS,
        "Drive scan timed out. Click Refresh and try again."
      );
      setDrives(available);

      if (available.length > 0 && !available.some((drive) => drive.id === selectedDriveId)) {
        setSelectedDriveId(available[0].id);
      }

      if (available.length === 0) {
        setSelectedDriveId("");
      }
    } catch (error) {
      setUiError(`Failed to list drives: ${asErrorMessage(error)}`);
    } finally {
      setDrivesBusy(false);
    }
  }

  async function pickLocalDestination() {
    if (!tauriReady) {
      setUiError("Tauri runtime not available.");
      return;
    }

    setUiError("");

    try {
      const path = await withTimeout(
        pickLocalDestinationFolder(),
        FILE_PICKER_TIMEOUT_MS,
        "Destination folder picker timed out. Try again or paste the destination path manually."
      );
      if (path) {
        setLocalDestinationPath(path);
      }
    } catch (error) {
      setUiError(`Could not choose local destination: ${asErrorMessage(error)}`);
    }
  }

  async function pickReceiverOutputDir() {
    if (!tauriReady) {
      setUiError("Tauri runtime not available.");
      return;
    }

    setUiError("");

    try {
      const path = await withTimeout(
        pickReceiveFolder(),
        FILE_PICKER_TIMEOUT_MS,
        "Receive folder picker timed out. Try again or paste the destination path manually."
      );
      if (path) {
        setReceiverOutputDir(path);
      }
    } catch (error) {
      setUiError(`Could not choose receiver folder: ${asErrorMessage(error)}`);
    }
  }

  async function startReceiver() {
    if (!tauriReady) {
      setUiError("Tauri runtime not available.");
      return;
    }

    setReceiverBusy(true);
    setUiError("");

    try {
      const payload = await startReceiverCommand({
        deviceName: toOptionalText(receiverDeviceName),
        outputDir: toOptionalText(receiverOutputDir),
      });

      setReceiverStatus({
        state: "starting",
        message: `Receiver starting on ${payload.bindAddr}.`,
        bindAddr: payload.bindAddr,
        outputDir: payload.outputDir,
        outputDirLabel: payload.outputDirLabel,
        certificatePath: payload.certificatePath,
      });
    } catch (error) {
      setReceiverBusy(false);
      setUiError(`Could not start receiver: ${asErrorMessage(error)}`);
    }
  }

  async function stopReceiver() {
    if (!tauriReady) {
      setUiError("Tauri runtime not available.");
      return;
    }

    setUiError("");

    try {
      await stopReceiverCommand();
      setReceiverBusy(false);
      setUiNote("Receiver stop requested.");
    } catch (error) {
      setUiError(`Could not stop receiver: ${asErrorMessage(error)}`);
    }
  }
  async function controlActiveSend(action: "pause" | "resume" | "stop") {
    if (!tauriReady) {
      setUiError("Tauri runtime not available.");
      return;
    }

    if (!sendBusy) {
      return;
    }

    setUiError("");

    const pausedBeforeAction = sendPausedRef.current;

    try {
      if (action === "pause") {
        await pauseSendCommand();
        updateSendPaused(true);
        setSendStatus((previous) => ({
          ...previous,
          state: "paused",
          message: "Transfer paused. Press Resume to continue.",
        }));
        setUiNote("Transfer paused.");
      } else if (action === "resume") {
        await resumeSendCommand();
        updateSendPaused(false);
        setSendStatus((previous) => ({
          ...previous,
          state: "sending",
          message: "Transfer resumed.",
        }));
        setUiNote("Transfer resumed.");
      } else {
        await stopSendCommand();
        updateSendPaused(false);
        setUiNote("Stop requested.");
      }
    } catch (error) {
      if (action === "resume") {
        updateSendPaused(pausedBeforeAction);
      }

      const verb = action === "pause" ? "pause" : action === "resume" ? "resume" : "stop";
      setUiError(`Could not ${verb} transfer: ${asErrorMessage(error)}`);
    }
  }


  async function startTransferForSource(source: string, plan: BatchSendPlan) {
    const baseAttempt = {
      sourcePath: source,
      destinationMode: plan.destinationMode,
      chunkSize: plan.chunkSize,
      parallelism: plan.parallelism,
    };

    const batchPrefix =
      plan.sources.length > 1
        ? `Batch ${plan.currentIndex + 1}/${plan.sources.length}: `
        : "";

    if (plan.destinationMode === "local" || plan.destinationMode === "usb") {
      const destinationPath = plan.destinationPath ?? "";
      activeSendAttemptRef.current = {
        ...baseAttempt,
        attemptId: `${Date.now()}-${Math.random().toString(16).slice(2, 10)}`,
        startedAt: Date.now(),
        destinationPath,
      };

      await invokeWithSendStartRetry(() =>
        startLocalCopyTransfer({
          sourcePath: source,
          destinationPath,
          destinationKind: plan.destinationMode === "usb" ? "usb_drive" : "local_folder",
          chunkSize: plan.chunkSize,
          parallelism: plan.parallelism,
        })
      );

      setSendStatus({
        state: "starting",
        message: `${batchPrefix}Local copy started.`,
      });
      return;
    }

    if (plan.destinationMode === "nearby") {
      const peerId = plan.selectedPeerId ?? "";
      activeSendAttemptRef.current = {
        ...baseAttempt,
        attemptId: `${Date.now()}-${Math.random().toString(16).slice(2, 10)}`,
        startedAt: Date.now(),
        selectedPeerId: peerId,
      };

      await invokeWithSendStartRetry(() =>
        startSendCommand({
          sourcePath: source,
          selectedPeerId: peerId,
          serverName: plan.serverName,
          chunkSize: plan.chunkSize,
          parallelism: plan.parallelism,
        })
      );

      setSendStatus({
        state: "starting",
        message: `${batchPrefix}Transfer started using nearby receiver mode.`,
      });
      return;
    }

    const manualAddress = plan.targetAddr ?? "";
    const manualCertificate = plan.certificatePath ?? "";
    activeSendAttemptRef.current = {
      ...baseAttempt,
      attemptId: `${Date.now()}-${Math.random().toString(16).slice(2, 10)}`,
      startedAt: Date.now(),
      targetAddr: manualAddress,
      certificatePath: manualCertificate,
    };

    await invokeWithSendStartRetry(() =>
      startSendCommand({
        sourcePath: source,
        targetAddr: manualAddress,
        certificatePath: manualCertificate,
        serverName: plan.serverName,
        chunkSize: plan.chunkSize,
        parallelism: plan.parallelism,
      })
    );

    setSendStatus({
      state: "starting",
      message: `${batchPrefix}Transfer started using manual mode.`,
    });
  }

  async function startTransfer() {
    if (!tauriReady) {
      setUiError("Tauri runtime not available.");
      return;
    }

    const sources = normalizeUniquePaths(sourcePaths);
    if (sources.length === 0) {
      setUiError("Choose at least one source file or folder before sending.");
      return;
    }

    const parsedChunkSize = toPositiveInteger(chunkSize);
    const parsedParallelism = toPositiveInteger(parallelism);

    if (!parsedChunkSize || !parsedParallelism) {
      setUiError("Chunk size and parallelism must be positive integers.");
      return;
    }

    const resolvedServerName = toOptionalText(serverName) ?? DEFAULT_SERVER_NAME;

    let destinationPath: string | undefined;
    let resolvedPeerId: string | undefined;
    let resolvedTargetAddr: string | undefined;
    let resolvedCertificatePath: string | undefined;

    if (destinationMode === "local" || destinationMode === "usb") {
      destinationPath = destinationMode === "usb" ? selectedDrive?.mountPath ?? "" : localDestinationPath.trim();
      if (!destinationPath) {
        setUiError(
          destinationMode === "usb"
            ? "Select a removable drive before sending."
            : "Choose a local destination folder before sending."
        );
        return;
      }
    }

    if (destinationMode === "nearby") {
      if (!selectedPeerId) {
        setUiError("Select a nearby receiver before sending.");
        return;
      }
      resolvedPeerId = selectedPeerId;
    }

    if (destinationMode === "manual") {
      resolvedTargetAddr = targetAddr.trim();
      resolvedCertificatePath = certificatePath.trim();
      if (!resolvedTargetAddr || !resolvedCertificatePath) {
        setUiError("Manual mode requires receiver address and certificate path.");
        return;
      }
    }

    const plan: BatchSendPlan = {
      sources,
      currentIndex: 0,
      destinationMode,
      destinationPath,
      selectedPeerId: resolvedPeerId,
      targetAddr: resolvedTargetAddr,
      certificatePath: resolvedCertificatePath,
      serverName: resolvedServerName,
      chunkSize: parsedChunkSize,
      parallelism: parsedParallelism,
    };

    updateSendPaused(false);
    setSendBusy(true);
    setUiError("");

    if (sources.length > 1) {
      batchSendPlanRef.current = plan;
      setBatchProgress({ current: 1, total: sources.length });
      setSourceSummary(null);
      setUiNote(`Starting batch send (${sources.length} items).`);
    } else {
      batchSendPlanRef.current = null;
      setBatchProgress(null);
    }

    try {
      await startTransferForSource(sources[0], plan);
    } catch (error) {
      const message = `Could not start transfer: ${asErrorMessage(error)}`;
      setSendBusy(false);
      setUiError(message);
      setBatchProgress(null);
      batchSendPlanRef.current = null;

      const attempt = activeSendAttemptRef.current;
      if (attempt) {
        appendHistoryEntry({
          direction: "send",
          result: "error",
          message,
          sourcePath: attempt.sourcePath,
          destinationMode: attempt.destinationMode,
          destinationPath: attempt.destinationPath,
          selectedPeerId: attempt.selectedPeerId,
          targetAddr: attempt.targetAddr,
          certificatePath: attempt.certificatePath,
          chunkSize: attempt.chunkSize,
          parallelism: attempt.parallelism,
        });
      }

      activeSendAttemptRef.current = null;
    }
  }
  return (
    <div className="min-h-screen bg-canvas text-ink">
      <div className="mx-auto grid min-h-screen w-full max-w-[1640px] gap-6 p-5 lg:grid-cols-[300px_minmax(0,1fr)] 2xl:max-w-[1720px] 2xl:grid-cols-[320px_minmax(0,1fr)]">
        <aside className="self-start rounded-2xl border border-border bg-panel p-5 shadow-card lg:sticky lg:top-5 lg:max-h-[calc(100vh-2.5rem)] lg:overflow-y-auto">
          <h1 className="text-2xl font-semibold tracking-tight">FastTransfer</h1>
          <p className="mt-2 text-sm text-muted">Desktop sender/receiver for LAN and local copy.</p>

          <div className="mt-6 rounded-xl border border-border bg-surfaceMuted p-3 text-xs text-muted">
            {uiNote}
          </div>

          {uiError ? (
            <div className="mt-3 rounded-xl border border-dangerSoft bg-dangerSoft p-3 text-xs text-dangerInk">
              {uiError}
            </div>
          ) : null}

          <div className="mt-6" ref={sourcePickerMenuRef}>
            <div className="flex overflow-hidden rounded-xl border border-border bg-surface shadow-sm">
              <button
                className="min-w-0 flex-1 px-4 py-3 text-left"
                onClick={() => {
                  void triggerSourcePicker();
                }}
                disabled={!tauriReady || inspectBusy || sendBusy}
              >
                <div className="text-sm font-medium text-ink">Add Items</div>
                <div className="mt-1 text-[11px] text-muted">
                  Last used: {sourcePickerMode === "files" ? "Files" : "Folder"}
                </div>
              </button>
              <button
                className="w-12 border-l border-border text-sm font-semibold text-muted transition hover:bg-surfaceMuted"
                onClick={() => setSourcePickerMenuOpen((previous) => !previous)}
                disabled={!tauriReady || inspectBusy || sendBusy}
                aria-label="Open source picker options"
              >
                v
              </button>
            </div>

            {sourcePickerMenuOpen ? (
              <div className="mt-2 overflow-hidden rounded-xl border border-border bg-panel shadow-card">
                <button
                  className="block w-full px-4 py-3 text-left text-sm transition hover:bg-surfaceMuted"
                  onClick={() => {
                    void triggerSourcePicker("files");
                  }}
                  disabled={!tauriReady || inspectBusy || sendBusy}
                >
                  Add Files
                </button>
                <button
                  className="block w-full border-t border-border px-4 py-3 text-left text-sm transition hover:bg-surfaceMuted"
                  onClick={() => {
                    void triggerSourcePicker("folder");
                  }}
                  disabled={!tauriReady || inspectBusy || sendBusy}
                >
                  Add Folder
                </button>
              </div>
            ) : null}
          </div>

          <div
            ref={sourceDropZoneRef}
            className={`mt-4 rounded-2xl border border-dashed p-4 text-center transition ${
              dropZoneActive
                ? "border-accent bg-accentSoft text-accent"
                : dragDropActive
                ? "border-accent bg-surfaceMuted text-ink"
                : "border-border bg-surfaceMuted text-muted"
            }`}
          >
            <div className="text-sm font-semibold text-ink">Drag files and folders here</div>
            <div className="mt-1 text-[11px]">
              Drop a mix of files and folders in one step, or use Add Items above.
            </div>
          </div>

          <div className="mt-6 rounded-xl border border-border bg-surface p-3 text-xs text-muted">
            <div className="flex items-center justify-between gap-2">
              <div className="font-semibold text-ink">Selected Items</div>
              <div className="text-[11px]">
                {sourcePaths.length === 0
                  ? "No source selected"
                  : `${sourcePaths.length} selected`}
              </div>
            </div>

            {sourcePaths.length > 0 ? (
              <div className="mt-2 max-h-40 space-y-1 overflow-y-auto pr-1">
                {sourcePaths.map((path, index) => (
                  <div key={`${path}-${index}`} className="flex items-start gap-2 rounded-lg border border-border bg-surfaceMuted p-2">
                    <div className="min-w-0 flex-1 break-all">{path}</div>
                    <button
                      className="rounded-md border border-border bg-surface px-2 py-1 text-[11px]"
                      onClick={() => removeSourceAt(index)}
                      disabled={sendBusy}
                    >
                      Remove
                    </button>
                  </div>
                ))}
              </div>
            ) : null}

            {sourcePaths.length > 1 ? (
              <button
                className="mt-2 rounded-lg border border-border bg-surface px-3 py-1 text-[11px]"
                onClick={clearSources}
                disabled={sendBusy}
              >
                Clear All
              </button>
            ) : null}

            {batchProgress ? (
              <div className="mt-2 text-[11px] text-muted">
                Batch in progress: {batchProgress.current}/{batchProgress.total}
              </div>
            ) : null}
          </div>
          {/* <section className="mt-6 rounded-xl border border-border bg-surface p-4">
            <div className="flex items-center justify-between">
              <div className="text-xs font-semibold uppercase tracking-wide text-muted">Settings</div>
              <button
                className="rounded-lg border border-border px-2 py-1 text-[11px] text-muted"
                onClick={() => {
                  setServerName(DEFAULT_SERVER_NAME);
                  setChunkSize("1048576");
                  setParallelism("4");
                }}
                disabled={sendBusy}
              >
                Reset
              </button>
            </div>

            <label className="mt-3 block text-xs">
              <div className="mb-1 font-semibold uppercase tracking-wide text-muted">Device Name</div>
              <input
                className="w-full rounded-lg border border-border px-3 py-2 text-sm"
                placeholder="Laptop, Desktop, etc."
                value={receiverDeviceName}
                onChange={(event) => setReceiverDeviceName(event.target.value)}
              />
            </label>

            <label className="mt-3 block text-xs">
              <div className="mb-1 font-semibold uppercase tracking-wide text-muted">Server Name</div>
              <input
                className="w-full rounded-lg border border-border px-3 py-2 text-sm"
                placeholder={DEFAULT_SERVER_NAME}
                value={serverName}
                onChange={(event) => setServerName(event.target.value)}
                disabled={sendBusy}
              />
            </label>

            <div className="mt-3 grid grid-cols-2 gap-2">
              <label className="text-xs">
                <div className="mb-1 font-semibold uppercase tracking-wide text-muted">Chunk</div>
                <input
                  className="w-full rounded-lg border border-border px-3 py-2 text-sm"
                  value={chunkSize}
                  onChange={(event) => setChunkSize(event.target.value)}
                  disabled={sendBusy}
                />
              </label>

              <label className="text-xs">
                <div className="mb-1 font-semibold uppercase tracking-wide text-muted">Parallel</div>
                <input
                  className="w-full rounded-lg border border-border px-3 py-2 text-sm"
                  value={parallelism}
                  onChange={(event) => setParallelism(event.target.value)}
                  disabled={sendBusy}
                />
              </label>
            </div>

            {sourceSummary ? (
              <button
                className="mt-3 w-full rounded-lg border border-border bg-surfaceMuted px-3 py-2 text-xs"
                onClick={() => {
                  setChunkSize(String(sourceSummary.recommendedChunkSize));
                  setParallelism(String(sourceSummary.recommendedParallelism));
                }}
                disabled={sendBusy}
              >
                Use Recommended Tuning
              </button>
            ) : null}
          </section> */}
          {/* <section className="mt-6 rounded-xl border border-border bg-surface p-4 text-xs text-muted">
            <div className="flex items-center justify-between">
              <div className="font-semibold uppercase tracking-wide">Recent History</div>
              <button
                className="rounded-lg border border-border px-2 py-1 text-[11px]"
                onClick={() => setActiveWorkspace("history")}
                disabled={historyEntries.length === 0}
              >
                Open
              </button>
            </div>

            {recentHistoryEntries.length === 0 ? (
              <p className="mt-2">No transfers yet.</p>
            ) : (
              <div className="mt-2 space-y-2">
                {recentHistoryEntries.map((entry) => (
                  <div key={entry.id} className="rounded-lg border border-border bg-surfaceMuted p-2">
                    <div className="flex items-center justify-between gap-2">
                      <div className="font-semibold text-ink">{entry.direction === "send" ? "Send" : "Receive"}</div>
                      <span
                        className={[
                          "rounded-full px-2 py-1 text-[10px] font-mono",
                          entry.result === "completed"
                            ? "bg-accentSoft text-accent"
                            : entry.result === "completed_with_issues"
                            ? "bg-warningSoft text-warningInk"
                            : entry.result === "stopped"
                            ? "bg-warningSoft text-warningInk"
                            : "bg-dangerSoft text-dangerInk",
                        ].join(" ")}
                      >
                        {entry.result}
                      </span>
                      </span>
                    </div>
                    <div className="mt-1 truncate">{entry.message}</div>
                  </div>
                ))}
              </div>
            )}
          </section> */}
        </aside>

        <main className="space-y-5">
          <section className="sticky top-5 z-20 rounded-2xl border border-border bg-panel/95 p-4 shadow-card backdrop-blur">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div>
                <div className="text-sm font-semibold">{transferBarTitle}</div>
                <div className="text-xs text-muted">{transferBarMessage}</div>
              </div>
              <div className="flex items-center gap-2">
                <span className={`rounded-full px-3 py-1 font-mono text-xs ${stateChipClasses(sendStatus.state)}`}>
                  send: {sendStatus.state}
                </span>
                <span className={`rounded-full px-3 py-1 font-mono text-xs ${stateChipClasses(receiverStatus.state)}`}>
                  recv: {receiverStatus.state}
                </span>
              </div>
            </div>

            <div className="mt-3 h-2 w-full rounded-full bg-surfaceMuted">
              <div
                className={[
                  "h-2 rounded-full transition-all duration-200",
                  sendPaused && sendIsActive ? "bg-warningInk" : "bg-accent",
                ].join(" ")}
                style={{ width: `${transferBarPercent.toFixed(2)}%` }}
              />
            </div>

            <div className="mt-2 flex items-center justify-between text-xs text-muted">
              <span>{transferBarPercent.toFixed(2)}%</span>
              {transferBarMeta ? <span>{transferBarMeta}</span> : <span>Idle</span>}
            </div>

            {transferBarCurrentPath ? (
              <div className="mt-1 truncate text-xs text-muted">Current: {transferBarCurrentPath}</div>
            ) : null}

            <div className="mt-4 grid gap-3 lg:grid-cols-[minmax(0,1fr)_auto] lg:items-center">
              <div className="rounded-xl border border-border bg-surface p-3 text-xs text-muted">
                <div className="font-semibold text-ink">Live Metrics</div>
                <div className="mt-1 flex flex-wrap gap-x-4 gap-y-1">
                  <span>
                    Send: {sendPercent.toFixed(2)}%
                    {sendStatusMeta ? ` (${sendStatusMeta})` : ""}
                  </span>
                  <span>Receiver: {receiverPercent.toFixed(2)}%</span>
                  <span>Throughput: {transferBarMeta ?? "-"}</span>
                </div>
              </div>

              <div className="flex flex-wrap items-center gap-2">
                {/* {sendIsActive ? (
                  <>
                    <button
                      className="rounded-xl border border-border bg-surface px-3 py-1 text-xs"
                      onClick={() => {
                        void controlActiveSend(sendPaused ? "resume" : "pause");
                      }}
                      disabled={!tauriReady}
                    >
                      {sendPaused ? "Resume" : "Pause"}
                    </button>
                    <button
                      className="rounded-xl border border-dangerSoft bg-dangerSoft px-3 py-1 text-xs text-dangerInk"
                      onClick={() => {
                        void controlActiveSend("stop");
                      }}
                      disabled={!tauriReady}
                    >
                      Stop Send
                    </button>
                  </>
                ) : (
                  <button
                    className="rounded-xl border border-border bg-surface px-3 py-1 text-xs"
                    onClick={() => setActiveWorkspace("send")}
                  >
                    Open Send
                  </button>
                )} */}

                {/* <button
                  className="rounded-xl border border-border bg-surface px-3 py-1 text-xs"
                  onClick={() => {
                    void startReceiver();
                    setActiveWorkspace("receive");
                  }}
                  disabled={!tauriReady || receiverBusy}
                >
                  Start Receiver
                </button>
                <button
                  className="rounded-xl border border-border bg-surface px-3 py-1 text-xs"
                  onClick={() => {
                    void stopReceiver();
                    setActiveWorkspace("receive");
                  }}
                  disabled={!tauriReady}
                >
                  Stop Receiver
                </button> */}
              </div>
            </div>
          </section>

          {/* <section ref={workspaceTopRef} className="rounded-2xl border border-border bg-panel p-4 shadow-card">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div>
                <div className="text-[11px] font-semibold uppercase tracking-wide text-muted">You Are Here</div>
                <div className="mt-1 text-lg font-semibold text-ink">{activeWorkspaceLabel}</div>
                <p className="mt-1 text-sm text-muted">{activeWorkspaceContext.subtitle}</p>
                {activeWorkspace === "send" ? (
                  <p className="mt-2 text-xs text-muted">
                    Current mode: <span className="font-semibold text-ink">{activeDestinationLabel}</span> - {DESTINATION_MODE_HINTS[destinationMode]}
                  </p>
                ) : (
                  <p className="mt-2 text-xs text-muted">{activeWorkspaceContext.hint}</p>
                )}
              </div>

              <div className="flex flex-wrap items-center gap-2">
                {activeWorkspace !== "send" ? (
                  <button
                    className="rounded-xl border border-border bg-surface px-3 py-1 text-xs"
                    onClick={() => setActiveWorkspace("send")}
                  >
                    Go To Send
                  </button>
                ) : null}
                {activeWorkspace !== "receive" ? (
                  <button
                    className="rounded-xl border border-border bg-surface px-3 py-1 text-xs"
                    onClick={() => setActiveWorkspace("receive")}
                  >
                    Go To Receive
                  </button>
                ) : null}
                {activeWorkspace !== "history" ? (
                  <button
                    className="rounded-xl border border-border bg-surface px-3 py-1 text-xs"
                    onClick={() => setActiveWorkspace("history")}
                  >
                    Open History
                  </button>
                ) : null}
                {activeWorkspace !== "settings" ? (
                  <button
                    className="rounded-xl border border-border bg-surface px-3 py-1 text-xs"
                    onClick={() => setActiveWorkspace("settings")}
                  >
                    Open Settings
                  </button>
                ) : null}
              </div>
            </div>
          </section> */}

          <section className="rounded-2xl border border-border bg-panel p-2 shadow-card">
            <div className="flex flex-wrap gap-2">
              {WORKSPACE_TABS.map((workspace) => (
                <button
                  key={workspace.key}
                  className={[
                    "min-w-[122px] rounded-xl px-4 py-2 text-left text-sm transition",
                    activeWorkspace === workspace.key
                      ? "bg-accent text-white"
                      : "border border-border bg-surface text-muted",
                  ].join(" ")}
                  onClick={() => setActiveWorkspace(workspace.key)}
                >
                  <span className="block font-semibold leading-tight">
                    {workspace.label}
                    {workspace.key === "history" ? ` (${historyEntries.length})` : ""}
                  </span>
                  <span className="mt-1 block text-[11px] opacity-80">
                    {workspace.key === "send"
                      ? `${activeDestinationLabel} mode`
                      : workspace.key === "receive"
                      ? "Incoming transfers"
                      : workspace.key === "history"
                      ? `${historyEntries.length} records`
                      : "App preferences"}
                  </span>
                </button>
              ))}
            </div>
          </section>
          <div className="space-y-5">
          {activeWorkspace === "send" ? (
            <section className="rounded-2xl border border-border bg-panel p-6 shadow-card">
            <div className="flex items-start justify-between gap-4">
              <div>
                <h2 className="text-xl font-semibold">Send</h2>
                <p className="mt-1 text-sm text-muted">
                  Choose a destination mode and start transfer.
                </p>
              </div>
              <button
                className="rounded-xl border border-border bg-surface px-4 py-2 text-sm"
                onClick={() => {
                  if (destinationMode === "nearby") {
                    void refreshReceivers();
                  }
                  if (destinationMode === "usb") {
                    void refreshDrives();
                  }
                }}
                disabled={!tauriReady || receiversBusy || drivesBusy || sendBusy}
              >
                Refresh
              </button>
            </div>

            <div className="mt-5 flex flex-wrap gap-2">
              {DESTINATION_MODES.map((mode) => (
                <button
                  key={mode.key}
                  className={[
                    "rounded-full px-4 py-2 text-sm",
                    destinationMode === mode.key
                      ? "bg-accent text-white"
                      : "border border-border bg-surface text-muted",
                  ].join(" ")}
                  onClick={() => setDestinationMode(mode.key)}
                  disabled={!tauriReady || sendBusy}
                >
                  {mode.label}
                </button>
              ))}
            </div>

            <div className="mt-3 rounded-xl border border-border bg-surface p-3 text-xs text-muted">
              Current send mode: <span className="font-semibold text-ink">{activeDestinationLabel}</span> - {DESTINATION_MODE_HINTS[destinationMode]}
            </div>
            {destinationMode === "nearby" ? (
              <div className="mt-5 rounded-xl border border-border bg-surface p-4">
                <div className="mb-3 flex items-center justify-between">
                  <div className="text-sm font-semibold">Nearby Receivers</div>
                  <button
                    className="rounded-lg border border-border px-3 py-1 text-xs"
                    onClick={() => {
                      void refreshReceivers();
                    }}
                    disabled={!tauriReady || receiversBusy || sendBusy}
                  >
                    {receiversBusy ? "Scanning..." : "Scan LAN"}
                  </button>
                </div>

                <div className="mb-3 flex flex-wrap items-center gap-2">
                  <input
                    className="min-w-[220px] flex-1 rounded-xl border border-border px-3 py-2 text-sm"
                    placeholder="Filter by name, IP, or fingerprint"
                    value={receiverQuery}
                    onChange={(event) => setReceiverQuery(event.target.value)}
                    disabled={sendBusy}
                  />
                  <span className="text-xs text-muted">
                    {filteredReceivers.length}/{receivers.length}
                  </span>
                </div>

                {receivers.length === 0 ? (
                  <p className="text-sm text-muted">No devices found yet.</p>
                ) : filteredReceivers.length === 0 ? (
                  <p className="text-sm text-muted">No receivers match your filter.</p>
                ) : (
                  <div className="max-h-72 space-y-2 overflow-y-auto pr-1">
                    {filteredReceivers.map((receiver) => (
                      <button
                        key={receiver.peerId}
                        className={[
                          "w-full rounded-xl border px-3 py-3 text-left",
                          selectedPeerId === receiver.peerId
                            ? "border-accent bg-accentSoft"
                            : "border-border bg-surface",
                        ].join(" ")}
                        onClick={() => setSelectedPeerId(receiver.peerId)}
                        disabled={sendBusy}
                      >
                        <div className="flex items-center justify-between gap-3">
                          <span className="text-sm font-semibold">{receiver.deviceName}</span>
                          <span className="rounded-full bg-surface px-2 py-1 font-mono text-[11px] text-muted">
                            {receiver.shortFingerprint}
                          </span>
                        </div>
                        <div className="mt-1 text-xs text-muted">{receiver.trustMessage}</div>
                        <div className="mt-1 text-xs text-muted">{receiver.addresses.join(", ")}</div>
                      </button>
                    ))}
                  </div>
                )}
              </div>
            ) : null}

            {destinationMode === "manual" ? (
              <div className="mt-5 grid gap-3 rounded-xl border border-border bg-surface p-4 md:grid-cols-2">
                <label className="text-sm">
                  <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted">Receiver Address</div>
                  <input
                    className="w-full rounded-xl border border-border px-3 py-2"
                    placeholder="192.168.1.20:5000"
                    value={targetAddr}
                    onChange={(event) => setTargetAddr(event.target.value)}
                    disabled={sendBusy}
                  />
                </label>

                <label className="text-sm">
                  <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted">Server Name</div>
                  <input
                    className="w-full rounded-xl border border-border px-3 py-2"
                    value={serverName}
                    onChange={(event) => setServerName(event.target.value)}
                    disabled={sendBusy}
                  />
                </label>

                <label className="text-sm md:col-span-2">
                  <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted">Certificate Path</div>
                  <div className="flex gap-2">
                    <input
                      className="w-full rounded-xl border border-border px-3 py-2"
                      placeholder="C:\\path\\receiver-cert.der"
                      value={certificatePath}
                      onChange={(event) => setCertificatePath(event.target.value)}
                      disabled={sendBusy}
                    />
                    <button
                      className="rounded-xl border border-border bg-surface px-3 py-2 text-sm"
                      onClick={() => {
                        void pickCertificate();
                      }}
                      disabled={!tauriReady || sendBusy}
                    >
                      Browse
                    </button>
                  </div>
                </label>
              </div>
            ) : null}

            {destinationMode === "usb" ? (
              <div className="mt-5 rounded-xl border border-border bg-surface p-4">
                <div className="mb-3 flex items-center justify-between">
                  <div className="text-sm font-semibold">Removable Drives</div>
                  <button
                    className="rounded-lg border border-border px-3 py-1 text-xs"
                    onClick={() => {
                      void refreshDrives();
                    }}
                    disabled={!tauriReady || drivesBusy || sendBusy}
                  >
                    {drivesBusy ? "Loading..." : "Refresh Drives"}
                  </button>
                </div>

                {drives.length === 0 ? (
                  <p className="text-sm text-muted">No removable drives detected.</p>
                ) : (
                  <div className="space-y-2">
                    {drives.map((drive) => (
                      <button
                        key={drive.id}
                        className={[
                          "w-full rounded-xl border px-3 py-3 text-left",
                          selectedDriveId === drive.id
                            ? "border-accent bg-accentSoft"
                            : "border-border bg-surface",
                        ].join(" ")}
                        onClick={() => setSelectedDriveId(drive.id)}
                        disabled={sendBusy}
                      >
                        <div className="text-sm font-semibold">{drive.driveLetter} - {drive.label}</div>
                        <div className="mt-1 text-xs text-muted">
                          {drive.mountPath} | Free {formatBytes(drive.freeBytes)} / {formatBytes(drive.totalBytes)}
                        </div>
                      </button>
                    ))}
                  </div>
                )}
              </div>
            ) : null}

            {destinationMode === "local" ? (
              <div className="mt-5 rounded-xl border border-border bg-surface p-4">
                <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted">Destination Folder</div>
                <div className="flex gap-2">
                  <input
                    className="w-full rounded-xl border border-border px-3 py-2"
                    placeholder="Choose local destination"
                    value={localDestinationPath}
                    onChange={(event) => setLocalDestinationPath(event.target.value)}
                    disabled={sendBusy}
                  />
                  <button
                    className="rounded-xl border border-border bg-surface px-3 py-2 text-sm"
                    onClick={() => {
                      void pickLocalDestination();
                    }}
                    disabled={!tauriReady || sendBusy}
                  >
                    Browse
                  </button>
                </div>
              </div>
            ) : null}

            <div className="mt-5 flex flex-wrap items-center justify-between gap-2">
              <button
                className="rounded-xl border border-border bg-surface px-3 py-2 text-xs"
                onClick={() => setShowAdvancedSend((previous) => !previous)}
                disabled={sendBusy}
              >
                {showAdvancedSend ? "Hide Advanced" : "Show Advanced"}
              </button>
              {!showAdvancedSend ? (
                <div className="text-xs text-muted">
                  Using optimized defaults (chunk {chunkSize}, parallel {parallelism}).
                </div>
              ) : null}
            </div>

            {showAdvancedSend ? (
              <div className="mt-3 grid gap-3 md:grid-cols-2">
                <label className="text-sm">
                  <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted">Chunk Size</div>
                  <input
                    className="w-full rounded-xl border border-border px-3 py-2 font-mono"
                    value={chunkSize}
                    onChange={(event) => setChunkSize(event.target.value)}
                    disabled={sendBusy}
                  />
                </label>

                <label className="text-sm">
                  <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted">Parallelism</div>
                  <input
                    className="w-full rounded-xl border border-border px-3 py-2 font-mono"
                    value={parallelism}
                    onChange={(event) => setParallelism(event.target.value)}
                    disabled={sendBusy}
                  />
                </label>
              </div>
            ) : null}

            <div className="mt-3 flex items-end">
              <button
                className="w-full rounded-xl bg-accent px-5 py-3 text-sm font-semibold text-white disabled:opacity-60"
                onClick={() => {
                  void startTransfer();
                }}
                disabled={!tauriReady || sendBusy || inspectBusy}
              >
                {sendBusy ? "Working..." : "Send Files"}
              </button>
            </div>

            {sendBusy ? (
              <>
                <div className="mt-3 flex gap-2">
                  <button
                    className="rounded-xl border border-border bg-surface px-4 py-2 text-sm"
                    onClick={() => {
                      void controlActiveSend(sendPaused ? "resume" : "pause");
                    }}
                    disabled={!tauriReady}
                  >
                    {sendPaused ? "Resume" : "Pause"}
                  </button>
                  <button
                    className="rounded-xl border border-dangerSoft bg-dangerSoft px-4 py-2 text-sm text-dangerInk"
                    onClick={() => {
                      void controlActiveSend("stop");
                    }}
                    disabled={!tauriReady}
                  >
                    Stop
                  </button>
                </div>
                <p className="mt-2 text-xs text-muted">
                  {sendPaused
                    ? "Transfer is paused. Click Resume to continue from current progress."
                    : "Transfer is active. You can pause or stop at any time."}
                </p>
              </>
            ) : null}

            {sourceSummary ? (
              <div className="mt-4 rounded-xl border border-border bg-accentSoft p-4 text-sm text-muted">
                <div className="font-semibold text-ink">Package Summary</div>
                <div className="mt-2 grid gap-1 md:grid-cols-2">
                  <div>Root: {sourceSummary.rootName} ({sourceSummary.rootKind})</div>
                  <div>Size: {formatBytes(sourceSummary.totalBytes)}</div>
                  <div>Files: {sourceSummary.totalFiles}</div>
                  <div>Folders: {sourceSummary.totalDirectories}</div>
                  <div>Recommended chunk: {sourceSummary.recommendedChunkSize}</div>
                  <div>Recommended parallelism: {sourceSummary.recommendedParallelism}</div>
                </div>
                <div className="mt-2 text-xs">{sourceSummary.tuningProfile}: {sourceSummary.tuningReason}</div>
              </div>
            ) : null}

            {destinationMode === "nearby" && selectedReceiver ? (
              <div className="mt-4 rounded-xl border border-border bg-surface p-3 text-sm text-muted">
                Selected device: <span className="font-semibold text-ink">{selectedReceiver.deviceName}</span> ({selectedReceiver.shortFingerprint})
              </div>
            ) : null}

            {destinationMode === "usb" && selectedDrive ? (
              <div className="mt-4 rounded-xl border border-border bg-surface p-3 text-sm text-muted">
                Selected drive: <span className="font-semibold text-ink">{selectedDrive.mountPath}</span>
              </div>
            ) : null}
          </section>
          ) : null}

          {activeWorkspace === "receive" ? (
            <section className="rounded-2xl border border-border bg-panel p-6 shadow-card">
            <div className="flex items-start justify-between gap-4">
              <div>
                <h2 className="text-xl font-semibold">Receive</h2>
                <p className="mt-1 text-sm text-muted">Start receiver to accept incoming transfers and watch status.</p>
              </div>
              <div className="flex gap-2">
                <button
                  className="rounded-xl border border-border bg-surface px-4 py-2 text-sm"
                  onClick={() => {
                    void startReceiver();
                  }}
                  disabled={!tauriReady || receiverBusy}
                >
                  Start Receiver
                </button>
                <button
                  className="rounded-xl border border-border bg-surface px-4 py-2 text-sm"
                  onClick={() => {
                    void stopReceiver();
                  }}
                  disabled={!tauriReady}
                >
                  Stop
                </button>
              </div>
            </div>

            <div className="mt-4 grid gap-3 md:grid-cols-2">
              <label className="text-sm">
                <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted">Advertised Device Name</div>
                <input
                  className="w-full rounded-xl border border-border px-3 py-2"
                  placeholder="Office Laptop"
                  value={receiverDeviceName}
                  onChange={(event) => setReceiverDeviceName(event.target.value)}
                />
              </label>

              <label className="text-sm">
                <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted">Receive Folder</div>
                <div className="flex gap-2">
                  <input
                    className="w-full rounded-xl border border-border px-3 py-2"
                    placeholder="Default Downloads/FastTransfer"
                    value={receiverOutputDir}
                    onChange={(event) => setReceiverOutputDir(event.target.value)}
                  />
                  <button
                    className="rounded-xl border border-border bg-surface px-3 py-2 text-sm"
                    onClick={() => {
                      void pickReceiverOutputDir();
                    }}
                    disabled={!tauriReady}
                  >
                    Browse
                  </button>
                </div>
              </label>
            </div>
          </section>
          ) : null}

          {activeWorkspace === "history" ? (
            <section id="history-section" className="rounded-2xl border border-border bg-panel p-6 shadow-card">
              <div className="flex items-center justify-between">
                <h3 className="text-lg font-semibold">History</h3>
                <button
                  className="rounded-xl border border-border bg-surface px-3 py-2 text-xs"
                  onClick={clearHistory}
                  disabled={historyEntries.length === 0}
                >
                  Clear History
                </button>
              </div>

              {historyEntries.length === 0 ? (
                <p className="mt-3 text-sm text-muted">No transfers recorded yet.</p>
              ) : (
                <div className="mt-4 space-y-3">
                  {historyEntries.map((entry) => (
                    <div key={entry.id} className="rounded-xl border border-border bg-surface p-3">
                      <div className="flex items-center justify-between gap-3">
                        <div className="text-sm font-semibold text-ink">
                          {entry.direction === "send" ? "Send" : "Receive"}
                        </div>
                        <div className="flex items-center gap-2">
                          <span
                            className={[
                              "rounded-full px-2 py-1 text-[11px] font-mono",
                              entry.result === "completed"
                                ? "bg-accentSoft text-accent"
                                : entry.result === "completed_with_issues"
                                ? "bg-warningSoft text-warningInk"
                                : entry.result === "stopped"
                                ? "bg-warningSoft text-warningInk"
                                : "bg-dangerSoft text-dangerInk",
                            ].join(" ")}
                          >
                            {entry.result}
                          </span>
                          <span className="text-[11px] text-muted">
                            {new Date(entry.createdAt).toLocaleString()}
                          </span>
                        </div>
                      </div>

                      <p className="mt-2 text-xs text-muted">{entry.message}</p>

                      <div className="mt-2 grid gap-1 text-[11px] text-muted md:grid-cols-2">
                        {entry.sourcePath ? <div>Source: {entry.sourcePath}</div> : null}
                        {entry.destinationPath ? <div>Destination: {entry.destinationPath}</div> : null}
                        {entry.destinationMode ? <div>Mode: {entry.destinationMode}</div> : null}
                        {entry.bytesTransferred ? <div>Bytes: {formatBytes(entry.bytesTransferred)}</div> : null}
                        {entry.averageMibPerSec ? <div>Speed: {entry.averageMibPerSec.toFixed(2)} MiB/s</div> : null}
                        {typeof entry.failedFiles === "number" ? <div>Failed files: {entry.failedFiles}</div> : null}
                        {typeof entry.totalFiles === "number" ? <div>Total files: {entry.totalFiles}</div> : null}
                        {typeof entry.integrityVerified === "boolean" ? (
                          <div>
                            Integrity: {entry.integrityVerified ? (entry.failedFiles ? "Verified for transferred files" : "Verified") : "Not verified"}
                          </div>
                        ) : null}
                      </div>

                      {entry.issues && entry.issues.length > 0 ? (
                        <div className="mt-3 rounded-lg border border-border bg-surfaceMuted p-3 text-[11px] text-muted">
                          <div className="font-semibold text-ink">Issues</div>
                          <div className="mt-2 space-y-1">
                            {entry.issues.slice(0, 3).map((issue) => (
                              <div key={`${entry.id}-${issue.path}`}>
                                <span className="font-medium text-ink">{issue.path}</span>: {issue.message}
                              </div>
                            ))}
                            {entry.issues.length > 3 ? <div>+{entry.issues.length - 3} more issue(s)</div> : null}
                          </div>
                        </div>
                      ) : null}

                      {entry.direction === "send" ? (
                        <div className="mt-3">
                          <button
                            className="rounded-lg border border-border bg-surface px-3 py-1 text-xs"
                            onClick={() => {
                              applyHistorySettings(entry);
                              setActiveWorkspace("send");
                            }}
                          >
                            Reuse Settings
                          </button>
                        </div>
                      ) : null}
                    </div>
                  ))}
                </div>
              )}
            </section>
          ) : null}

          {activeWorkspace === "settings" ? (
            <section className="rounded-2xl border border-border bg-panel p-6 shadow-card">
              <h2 className="text-xl font-semibold">Settings</h2>
              <p className="mt-1 text-sm text-muted">Tune transfer defaults and receiver preferences.</p>

              <div className="mt-5 rounded-2xl border border-border bg-surface p-4">
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <div>
                    <div className="text-sm font-semibold text-ink">Theme</div>
                    <p className="mt-1 text-xs text-muted">
                      Choose how FastTransfer looks across the desktop app.
                    </p>
                  </div>
                  <span className="rounded-full bg-surfaceMuted px-3 py-1 text-[11px] font-mono text-muted">
                    Active: {resolvedTheme}{themePreference === "system" ? " (system)" : ""}
                  </span>
                </div>

                <div className="mt-4 flex flex-wrap gap-2">
                  {([
                    { value: "system", label: "System" },
                    { value: "light", label: "Light" },
                    { value: "dark", label: "Dark" },
                  ] as Array<{ value: ThemePreference; label: string }>).map((option) => (
                    <button
                      key={option.value}
                      className={[
                        "rounded-xl px-4 py-2 text-sm transition",
                        themePreference === option.value
                          ? "bg-accent text-white"
                          : "border border-border bg-surface text-muted",
                      ].join(" ")}
                      onClick={() => setThemePreference(option.value)}
                    >
                      {option.label}
                    </button>
                  ))}
                </div>
              </div>

              <div className="mt-5 grid gap-4 lg:grid-cols-2">
                <label className="text-sm">
                  <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted">Default Server Name</div>
                  <input
                    className="w-full rounded-xl border border-border px-3 py-2"
                    value={serverName}
                    onChange={(event) => setServerName(event.target.value)}
                    disabled={sendBusy}
                  />
                </label>

                <label className="text-sm">
                  <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted">Advertised Device Name</div>
                  <input
                    className="w-full rounded-xl border border-border px-3 py-2"
                    value={receiverDeviceName}
                    onChange={(event) => setReceiverDeviceName(event.target.value)}
                  />
                </label>

                <label className="text-sm">
                  <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted">Default Chunk Size</div>
                  <input
                    className="w-full rounded-xl border border-border px-3 py-2 font-mono"
                    value={chunkSize}
                    onChange={(event) => setChunkSize(event.target.value)}
                    disabled={sendBusy}
                  />
                </label>

                <label className="text-sm">
                  <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted">Default Parallelism</div>
                  <input
                    className="w-full rounded-xl border border-border px-3 py-2 font-mono"
                    value={parallelism}
                    onChange={(event) => setParallelism(event.target.value)}
                    disabled={sendBusy}
                  />
                </label>

                <label className="text-sm lg:col-span-2">
                  <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted">Default Receive Folder</div>
                  <div className="flex gap-2">
                    <input
                      className="w-full rounded-xl border border-border px-3 py-2"
                      value={receiverOutputDir}
                      onChange={(event) => setReceiverOutputDir(event.target.value)}
                    />
                    <button
                      className="rounded-xl border border-border bg-surface px-3 py-2 text-sm"
                      onClick={() => {
                        void pickReceiverOutputDir();
                      }}
                      disabled={!tauriReady}
                    >
                      Browse
                    </button>
                  </div>
                </label>
              </div>

              <div className="mt-5 flex flex-wrap gap-2">
                <button
                  className="rounded-xl border border-border bg-surface px-4 py-2 text-sm"
                  onClick={() => {
                    setServerName(DEFAULT_SERVER_NAME);
                    setChunkSize("1048576");
                    setParallelism("4");
                  }}
                  disabled={sendBusy}
                >
                  Reset Defaults
                </button>
                {sourceSummary ? (
                  <button
                    className="rounded-xl border border-border bg-surface px-4 py-2 text-sm"
                    onClick={() => {
                      setChunkSize(String(sourceSummary.recommendedChunkSize));
                      setParallelism(String(sourceSummary.recommendedParallelism));
                    }}
                    disabled={sendBusy}
                  >
                    Use Recommended Tuning
                  </button>
                ) : null}
              </div>
            </section>
          ) : null}
          </div>
        </main>
      </div>
    </div>
  );
}

export default App;














