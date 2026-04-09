import { useEffect, useMemo, useRef, useState } from "react";
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
  pickSourceFile,
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

type TransferDirection = "send" | "receive";
type TransferResult = "completed" | "error" | "stopped";

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
  integrityVerified?: boolean;
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

const HISTORY_STORAGE_KEY = "fasttransfer.desktop.history.v1";
const MAX_HISTORY_ENTRIES = 40;

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
function stateChipClasses(state: string): string {
  if (state === "completed" || state === "listening") {
    return "bg-accentSoft text-accent";
  }

  if (state === "error") {
    return "bg-[#fde8e8] text-[#b42318]";
  }

  if (state === "paused" || state === "stopped") {
    return "bg-[#fff4e5] text-[#9a5b00]";
  }

  if (state === "receiving" || state === "sending") {
    return "bg-[#e6f4ff] text-[#005da6]";
  }

  return "bg-[#edf1f1] text-[#4b5f63]";
}
function App() {
  const tauriReady = useMemo(() => isTauriRuntime(), []);

  const [sourcePath, setSourcePath] = useState("");
  const [sourceSummary, setSourceSummary] = useState<PackageSummary | null>(null);
  const [inspectBusy, setInspectBusy] = useState(false);

  const [destinationMode, setDestinationMode] = useState<DestinationMode>("nearby");
  const [activeWorkspace, setActiveWorkspace] = useState<WorkspaceTab>("send");

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
      ? "Connected to Tauri runtime."
      : "Run this screen through `npm run tauri dev` to enable native commands."
  );

  const [historyEntries, setHistoryEntries] = useState<TransferHistoryEntry[]>([]);
  const activeSendAttemptRef = useRef<SendAttemptContext | null>(null);
  const sendPausedRef = useRef(false);
  const [sendPaused, setSendPaused] = useState(false);
  const lastReceiverTerminalKeyRef = useRef("");

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
      setSourcePath(entry.sourcePath);
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
    if (!tauriReady) {
      return;
    }

    let unlistenSend: TauriUnlisten | null = null;
    let unlistenReceiver: TauriUnlisten | null = null;

    const setup = async () => {
      unlistenSend = await listenSendStatus((payload) => {
        const activeProgressStates = new Set(["starting", "scanning", "sending"]);
        const terminalStates = new Set(["completed", "error", "stopped", "idle"]);

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
          payload.state === "error" ||
          payload.state === "stopped"
        ) {
          setSendBusy(false);

          const attempt = activeSendAttemptRef.current;
          if (attempt) {
            const result: TransferResult =
              payload.state === "completed"
                ? "completed"
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
              integrityVerified: payload.summary?.integrityVerified,
            });
          }

          activeSendAttemptRef.current = null;
        }
      });

      unlistenReceiver = await listenReceiverStatus((payload) => {
        setReceiverStatus(payload);

        const terminalStates = new Set(["completed", "error", "stopped", "idle"]);
        if (terminalStates.has(payload.state)) {
          setReceiverBusy(false);
        }

        if (payload.state === "completed" || payload.state === "error") {
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
              result: payload.state,
              message: payload.message,
              sourcePath: payload.remoteAddress ?? undefined,
              destinationPath: payload.savedPath ?? payload.outputDir ?? undefined,
              bytesTransferred:
                payload.summary?.bytesTransferred ?? payload.progress?.transferredBytes,
              elapsedSecs: payload.summary?.elapsedSecs,
              averageMibPerSec:
                payload.summary?.averageMibPerSec ?? payload.progress?.averageMibPerSec,
              integrityVerified: payload.summary?.integrityVerified,
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

  useEffect(() => {
    if (destinationMode === "usb" && tauriReady) {
      void refreshDrives();
    }
  }, [destinationMode, tauriReady]);

  const selectedReceiver = receivers.find((item) => item.peerId === selectedPeerId) ?? null;
  const selectedDrive = drives.find((item) => item.id === selectedDriveId) ?? null;
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
    Math.min(100, sendStatus.progress?.percent ?? (sendStatus.state === "completed" ? 100 : 0))
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
    Math.min(100, receiverStatus.progress?.percent ?? (receiverStatus.state === "completed" ? 100 : 0))
  );
  const sendIsActive =
    sendBusy && !["completed", "error", "stopped", "idle"].includes(sendStatus.state);
  const receiverIsActive =
    receiverBusy && !["completed", "error", "stopped", "idle"].includes(receiverStatus.state);
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
  const recentHistoryEntries = historyEntries.slice(0, 3);`r`
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

  async function pickSource(command: "pick_source_file" | "pick_source_folder") {
    if (!tauriReady) {
      setUiError("Tauri runtime not available.");
      return;
    }

    setUiError("");

    try {
      const selectedPath = command === "pick_source_file" ? await pickSourceFile() : await pickSourceFolder();
      if (!selectedPath) {
        return;
      }

      setSourcePath(selectedPath);
      await inspectSelectedSource(selectedPath);
    } catch (error) {
      setUiError(`Could not choose source: ${asErrorMessage(error)}`);
    }
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
      const path = await pickCertificateFile();
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
      const available = await listRemovableDrives();
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
      const path = await pickLocalDestinationFolder();
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
      const path = await pickReceiveFolder();
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


  async function startTransfer() {
    if (!tauriReady) {
      setUiError("Tauri runtime not available.");
      return;
    }

    const source = sourcePath.trim();
    if (source.length === 0) {
      setUiError("Choose a source file or folder before sending.");
      return;
    }

    const parsedChunkSize = toPositiveInteger(chunkSize);
    const parsedParallelism = toPositiveInteger(parallelism);

    if (!parsedChunkSize || !parsedParallelism) {
      setUiError("Chunk size and parallelism must be positive integers.");
      return;
    }

    updateSendPaused(false);
    setSendBusy(true);
    setUiError("");

    const baseAttempt = {
      sourcePath: source,
      destinationMode,
      chunkSize: parsedChunkSize,
      parallelism: parsedParallelism,
    };

    try {
      if (destinationMode === "local" || destinationMode === "usb") {
        const destinationPath =
          destinationMode === "usb"
            ? selectedDrive?.mountPath ?? ""
            : localDestinationPath.trim();

        if (!destinationPath) {
          throw new Error(
            destinationMode === "usb"
              ? "Select a removable drive before sending."
              : "Choose a local destination folder before sending."
          );
        }

        activeSendAttemptRef.current = {
          ...baseAttempt,
          attemptId: `${Date.now()}-${Math.random().toString(16).slice(2, 10)}`,
          startedAt: Date.now(),
          destinationPath,
        };

        await startLocalCopyTransfer({
          sourcePath: source,
          destinationPath,
          destinationKind: destinationMode === "usb" ? "usb_drive" : "local_folder",
          chunkSize: parsedChunkSize,
          parallelism: parsedParallelism,
        });

        setSendStatus({
          state: "starting",
          message: "Local copy started.",
        });
        return;
      }

      if (destinationMode === "nearby") {
        if (!selectedPeerId) {
          throw new Error("Select a nearby receiver before sending.");
        }

        activeSendAttemptRef.current = {
          ...baseAttempt,
          attemptId: `${Date.now()}-${Math.random().toString(16).slice(2, 10)}`,
          startedAt: Date.now(),
          selectedPeerId,
        };

        await startSendCommand({
          sourcePath: source,
          selectedPeerId,
          serverName: toOptionalText(serverName) ?? DEFAULT_SERVER_NAME,
          chunkSize: parsedChunkSize,
          parallelism: parsedParallelism,
        });

        setSendStatus({
          state: "starting",
          message: "Transfer started using nearby receiver mode.",
        });
        return;
      }

      const manualAddress = targetAddr.trim();
      const manualCertificate = certificatePath.trim();
      if (!manualAddress || !manualCertificate) {
        throw new Error("Manual mode requires receiver address and certificate path.");
      }

      activeSendAttemptRef.current = {
        ...baseAttempt,
        attemptId: `${Date.now()}-${Math.random().toString(16).slice(2, 10)}`,
        startedAt: Date.now(),
        targetAddr: manualAddress,
        certificatePath: manualCertificate,
      };

      await startSendCommand({
        sourcePath: source,
        targetAddr: manualAddress,
        certificatePath: manualCertificate,
        serverName: toOptionalText(serverName) ?? DEFAULT_SERVER_NAME,
        chunkSize: parsedChunkSize,
        parallelism: parsedParallelism,
      });

      setSendStatus({
        state: "starting",
        message: "Transfer started using manual mode.",
      });
    } catch (error) {
      const message = `Could not start transfer: ${asErrorMessage(error)}`;
      setSendBusy(false);
      setUiError(message);

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

          <div className="mt-6 rounded-xl border border-border bg-[#f8fbfb] p-3 text-xs text-muted">
            {uiNote}
          </div>

          {uiError ? (
            <div className="mt-3 rounded-xl border border-[#f2c6c6] bg-[#fff3f3] p-3 text-xs text-[#b42318]">
              {uiError}
            </div>
          ) : null}

          <div className="mt-6 space-y-2">
            <button
              className="w-full rounded-xl border border-border bg-white px-4 py-3 text-left text-sm font-medium"
              onClick={() => {
                void pickSource("pick_source_file");
              }}
              disabled={!tauriReady || inspectBusy || sendBusy}
            >
              Select File
            </button>
            <button
              className="w-full rounded-xl border border-border bg-white px-4 py-3 text-left text-sm font-medium"
              onClick={() => {
                void pickSource("pick_source_folder");
              }}
              disabled={!tauriReady || inspectBusy || sendBusy}
            >
              Select Folder
            </button>
          </div>

          <div className="mt-6 rounded-xl border border-border bg-white p-3 text-xs text-muted">
            <div className="font-semibold text-ink">Source</div>
            <div className="mt-1 break-all">{sourcePath || "No source selected"}</div>
          </div>

          {/* <section className="mt-6 rounded-xl border border-border bg-white p-4">
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
                className="mt-3 w-full rounded-lg border border-border bg-[#f8fbfb] px-3 py-2 text-xs"
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
          {/* <section className="mt-6 rounded-xl border border-border bg-white p-4 text-xs text-muted">
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
                  <div key={entry.id} className="rounded-lg border border-border bg-[#f8fbfb] p-2">
                    <div className="flex items-center justify-between gap-2">
                      <div className="font-semibold text-ink">{entry.direction === "send" ? "Send" : "Receive"}</div>
                      <span
                        className={[
                          "rounded-full px-2 py-1 text-[10px] font-mono",
                          entry.result === "completed"
                            ? "bg-accentSoft text-accent"
                            : entry.result === "stopped"
                            ? "bg-[#fff4e5] text-[#9a5b00]"
                            : "bg-[#fde8e8] text-[#b42318]",
                        ].join(" ")}
                      >
                        {entry.result}
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

            <div className="mt-3 h-2 w-full rounded-full bg-[#e7efef]">
              <div
                className={[
                  "h-2 rounded-full transition-all duration-200",
                  sendPaused && sendIsActive ? "bg-[#d99a12]" : "bg-accent",
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
              <div className="rounded-xl border border-border bg-white p-3 text-xs text-muted">
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
                {sendIsActive ? (
                  <>
                    <button
                      className="rounded-xl border border-border bg-white px-3 py-1 text-xs"
                      onClick={() => {
                        void controlActiveSend(sendPaused ? "resume" : "pause");
                      }}
                      disabled={!tauriReady}
                    >
                      {sendPaused ? "Resume" : "Pause"}
                    </button>
                    <button
                      className="rounded-xl border border-[#f2c6c6] bg-[#fff3f3] px-3 py-1 text-xs text-[#b42318]"
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
                    className="rounded-xl border border-border bg-white px-3 py-1 text-xs"
                    onClick={() => setActiveWorkspace("send")}
                  >
                    Open Send
                  </button>
                )}

                <button
                  className="rounded-xl border border-border bg-white px-3 py-1 text-xs"
                  onClick={() => {
                    void startReceiver();
                    setActiveWorkspace("receive");
                  }}
                  disabled={!tauriReady || receiverBusy}
                >
                  Start Receiver
                </button>
                <button
                  className="rounded-xl border border-border bg-white px-3 py-1 text-xs"
                  onClick={() => {
                    void stopReceiver();
                    setActiveWorkspace("receive");
                  }}
                  disabled={!tauriReady}
                >
                  Stop Receiver
                </button>
              </div>
            </div>
          </section>

          <section className="rounded-2xl border border-border bg-panel p-2 shadow-card">
            <div className="flex flex-wrap gap-2">
              {WORKSPACE_TABS.map((workspace) => (
                <button
                  key={workspace.key}
                  className={[
                    "rounded-full px-4 py-2 text-sm",
                    activeWorkspace === workspace.key
                      ? "bg-accent text-white"
                      : "border border-border bg-white text-muted",
                  ].join(" ")}
                  onClick={() => setActiveWorkspace(workspace.key)}
                >
                  {workspace.label}
                  {workspace.key === "history" ? ` (${historyEntries.length})` : ""}
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
                className="rounded-xl border border-border bg-white px-4 py-2 text-sm"
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
                      : "border border-border bg-white text-muted",
                  ].join(" ")}
                  onClick={() => setDestinationMode(mode.key)}
                  disabled={!tauriReady || sendBusy}
                >
                  {mode.label}
                </button>
              ))}
            </div>

            {destinationMode === "nearby" ? (
              <div className="mt-5 rounded-xl border border-border bg-white p-4">
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
                            : "border-border bg-white",
                        ].join(" ")}
                        onClick={() => setSelectedPeerId(receiver.peerId)}
                        disabled={sendBusy}
                      >
                        <div className="flex items-center justify-between gap-3">
                          <span className="text-sm font-semibold">{receiver.deviceName}</span>
                          <span className="rounded-full bg-white px-2 py-1 font-mono text-[11px] text-muted">
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
              <div className="mt-5 grid gap-3 rounded-xl border border-border bg-white p-4 md:grid-cols-2">
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
                      className="rounded-xl border border-border bg-white px-3 py-2 text-sm"
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
              <div className="mt-5 rounded-xl border border-border bg-white p-4">
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
                            : "border-border bg-white",
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
              <div className="mt-5 rounded-xl border border-border bg-white p-4">
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
                    className="rounded-xl border border-border bg-white px-3 py-2 text-sm"
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

            <div className="mt-5 grid gap-3 md:grid-cols-3">
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

              <div className="flex items-end">
                <button
                  className="w-full rounded-xl bg-accent px-5 py-3 text-sm font-semibold text-white disabled:opacity-60"
                  onClick={() => {
                    void startTransfer();
                  }}
                  disabled={!tauriReady || sendBusy || inspectBusy}
                >
                  {sendBusy ? "Working..." : "Send Package"}
                </button>
              </div>
            </div>

            {sendBusy ? (
              <>
                <div className="mt-3 flex gap-2">
                  <button
                    className="rounded-xl border border-border bg-white px-4 py-2 text-sm"
                    onClick={() => {
                      void controlActiveSend(sendPaused ? "resume" : "pause");
                    }}
                    disabled={!tauriReady}
                  >
                    {sendPaused ? "Resume" : "Pause"}
                  </button>
                  <button
                    className="rounded-xl border border-[#f2c6c6] bg-[#fff3f3] px-4 py-2 text-sm text-[#b42318]"
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
              <div className="mt-4 rounded-xl border border-border bg-white p-3 text-sm text-muted">
                Selected device: <span className="font-semibold text-ink">{selectedReceiver.deviceName}</span> ({selectedReceiver.shortFingerprint})
              </div>
            ) : null}

            {destinationMode === "usb" && selectedDrive ? (
              <div className="mt-4 rounded-xl border border-border bg-white p-3 text-sm text-muted">
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
                  className="rounded-xl border border-border bg-white px-4 py-2 text-sm"
                  onClick={() => {
                    void startReceiver();
                  }}
                  disabled={!tauriReady || receiverBusy}
                >
                  Start Receiver
                </button>
                <button
                  className="rounded-xl border border-border bg-white px-4 py-2 text-sm"
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
                    className="rounded-xl border border-border bg-white px-3 py-2 text-sm"
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
                  className="rounded-xl border border-border bg-white px-3 py-2 text-xs"
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
                    <div key={entry.id} className="rounded-xl border border-border bg-white p-3">
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
                                : entry.result === "stopped"
                                ? "bg-[#fff4e5] text-[#9a5b00]"
                                : "bg-[#fde8e8] text-[#b42318]",
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
                        {typeof entry.integrityVerified === "boolean" ? (
                          <div>Integrity: {entry.integrityVerified ? "Verified" : "Not verified"}</div>
                        ) : null}
                      </div>

                      {entry.direction === "send" ? (
                        <div className="mt-3">
                          <button
                            className="rounded-lg border border-border bg-white px-3 py-1 text-xs"
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
                      className="rounded-xl border border-border bg-white px-3 py-2 text-sm"
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
                  className="rounded-xl border border-border bg-white px-4 py-2 text-sm"
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
                    className="rounded-xl border border-border bg-white px-4 py-2 text-sm"
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



