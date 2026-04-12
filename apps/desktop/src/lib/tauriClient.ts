import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type TauriUnlisten = UnlistenFn;

export type PackageSummary = {
  rootName: string;
  rootKind: string;
  totalFiles: number;
  totalDirectories: number;
  totalBytes: number;
  recommendedChunkSize: number;
  recommendedParallelism: number;
  tuningProfile: string;
  tuningReason: string;
};

export type ReceiverListItem = {
  deviceName: string;
  peerId: string;
  addresses: string[];
  transport: string;
  serviceName: string;
  shortFingerprint: string;
  trustState: string;
  trustMessage: string;
};

export type RemovableDrive = {
  id: string;
  driveLetter: string;
  label: string;
  mountPath: string;
  fileSystem?: string | null;
  freeBytes: number;
  totalBytes: number;
};

export type ProgressPayload = {
  label: string;
  phase: string;
  totalBytes: number;
  transferredBytes: number;
  percent: number;
  averageMibPerSec: number;
  completedFiles: number;
  totalFiles: number;
  currentPath?: string | null;
  completed: boolean;
};

export type TransferIssuePayload = {
  path: string;
  message: string;
};

export type TransferSummaryPayload = {
  fileName: string;
  bytesTransferred: number;
  elapsedSecs: number;
  averageMibPerSec: number;
  completedChunks: number;
  completedFiles: number;
  totalFiles: number;
  failedFiles: number;
  totalDirectories: number;
  sha256Hex: string;
  integrityVerified: boolean;
  issues: TransferIssuePayload[];
};

export type SendStatusPayload = {
  state: string;
  message: string;
  progress?: ProgressPayload | null;
  summary?: TransferSummaryPayload | null;
};

export type ReceiverStatusPayload = {
  state: string;
  message: string;
  bindAddr?: string | null;
  outputDir?: string | null;
  outputDirLabel?: string | null;
  progress?: ProgressPayload | null;
  summary?: TransferSummaryPayload | null;
  savedPath?: string | null;
  savedPathLabel?: string | null;
  remoteAddress?: string | null;
  certificatePath?: string | null;
};

export type ReceiverStartPayload = {
  bindAddr: string;
  outputDir: string;
  outputDirLabel: string;
  certificatePath: string;
};

export type LocalDestinationKind = "usb_drive" | "local_folder";

export type StartLocalCopyTransferRequest = {
  sourcePath: string;
  destinationPath: string;
  destinationKind: LocalDestinationKind;
  chunkSize: number;
  parallelism: number;
};

export type StartSendRequest = {
  sourcePath: string;
  selectedPeerId?: string;
  targetAddr?: string;
  certificatePath?: string;
  serverName?: string;
  chunkSize?: number;
  parallelism?: number;
};

export type StartReceiverOptions = {
  deviceName?: string;
  outputDir?: string;
};

export const DEFAULT_SERVER_NAME = "fasttransfer.local";

function toOptionalText(value?: string): string | undefined {
  if (!value) {
    return undefined;
  }

  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : undefined;
}

async function invokeWithArgFallback<T>(
  command: string,
  camelArgs?: Record<string, unknown>,
  snakeArgs?: Record<string, unknown>
): Promise<T> {
  try {
    return await invoke<T>(command, camelArgs);
  } catch (error) {
    if (!snakeArgs) {
      throw error;
    }
    return await invoke<T>(command, snakeArgs);
  }
}

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function listenSendStatus(
  onPayload: (payload: SendStatusPayload) => void
): Promise<TauriUnlisten> {
  return listen<SendStatusPayload>("send-status", (event) => {
    onPayload(event.payload);
  });
}

export async function listenReceiverStatus(
  onPayload: (payload: ReceiverStatusPayload) => void
): Promise<TauriUnlisten> {
  return listen<ReceiverStatusPayload>("receiver-status", (event) => {
    onPayload(event.payload);
  });
}

export function pickSourceFile(): Promise<string | null> {
  return invoke<string | null>("pick_source_file");
}

export function pickSourceFiles(): Promise<string[]> {
  return invoke<string[]>("pick_source_files");
}

export function pickSourceFolder(): Promise<string | null> {
  return invoke<string | null>("pick_source_folder");
}

export function pickCertificateFile(): Promise<string | null> {
  return invoke<string | null>("pick_certificate_file");
}
export function pickLocalDestinationFolder(): Promise<string | null> {
  return invoke<string | null>("pick_local_destination_folder");
}

export function pickReceiveFolder(): Promise<string | null> {
  return invoke<string | null>("pick_receive_folder");
}

export function openPathInFileManager(path: string): Promise<void> {
  return invoke<void>("open_path_in_file_manager", { path });
}

export function listRemovableDrives(): Promise<RemovableDrive[]> {
  return invoke<RemovableDrive[]>("list_removable_drives");
}

export function inspectSource(sourcePath: string): Promise<PackageSummary> {
  return invokeWithArgFallback<PackageSummary>(
    "inspect_source",
    { sourcePath },
    { source_path: sourcePath }
  );
}

export function discoverNearbyReceivers(timeoutSecs = 4): Promise<ReceiverListItem[]> {
  return invokeWithArgFallback<ReceiverListItem[]>(
    "discover_nearby_receivers",
    { timeoutSecs },
    { timeout_secs: timeoutSecs }
  );
}

export function startReceiver(options: StartReceiverOptions): Promise<ReceiverStartPayload> {
  return invokeWithArgFallback<ReceiverStartPayload>(
    "start_receiver",
    {
      deviceName: toOptionalText(options.deviceName),
      outputDir: toOptionalText(options.outputDir),
    },
    {
      device_name: toOptionalText(options.deviceName),
      output_dir: toOptionalText(options.outputDir),
    }
  );
}

export function stopReceiver(): Promise<void> {
  return invoke<void>("stop_receiver");
}

export function stopSend(): Promise<void> {
  return invoke<void>("stop_send");
}

export function pauseSend(): Promise<void> {
  return invoke<void>("pause_send");
}

export function resumeSend(): Promise<void> {
  return invoke<void>("resume_send");
}

export function startLocalCopyTransfer(request: StartLocalCopyTransferRequest): Promise<void> {
  return invoke<void>("start_local_copy_transfer", {
    request: {
      sourcePath: request.sourcePath,
      destinationPath: request.destinationPath,
      destinationKind: request.destinationKind,
      chunkSize: request.chunkSize,
      parallelism: request.parallelism,
    },
  });
}

export function startSend(request: StartSendRequest): Promise<void> {
  return invoke<void>("start_send", {
    request: {
      sourcePath: request.sourcePath,
      selectedPeerId: toOptionalText(request.selectedPeerId),
      targetAddr: toOptionalText(request.targetAddr),
      certificatePath: toOptionalText(request.certificatePath),
      serverName: toOptionalText(request.serverName),
      chunkSize: request.chunkSize,
      parallelism: request.parallelism,
    },
  });
}

