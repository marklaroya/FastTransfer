# FastTransfer

FastTransfer is a high-performance, local-first file transfer system designed to move large files between devices at maximum speed — without using the cloud.

Built for desktop and mobile, FastTransfer uses a streaming, parallelized transfer pipeline over QUIC to deliver fast, resumable, and integrity-verified transfers across devices on the same network.

## Why FastTransfer?

- **Streaming transfers** — start sending immediately without waiting for full file scans
- **High throughput** — chunked + parallel transfers over QUIC
- **Resume support** — continue interrupted transfers without restarting
- **Integrity verified** — SHA-256 validation per chunk and per file
- **Local-first** — no cloud, no upload/download delays
- **Folder support** — send entire directories with structure preserved


## How it works

1. Discover nearby devices over LAN (mDNS)
2. Select a file or folder to send
3. Stream file data in chunks over QUIC
4. Receiver reconstructs files in real time
5. Integrity is verified before completion
6. Transfers can resume if interrupted


## Architecture

- `crates/`: Rust workspace for the transfer engine and protocol components.
- `apps/mobile/`: Flutter app surface for Android and iOS clients.
- `apps/desktop/`: Tauri desktop app shell.
- `services/control-plane/`: Go control-plane service for coordination, auth, policy, and device registration.
- `services/relay/`: Relay service placeholder for NAT traversal fallback paths.

## Repository Layout

```text
FastTransfer/
+-- apps/
|   +-- desktop/
|   +-- mobile/
+-- crates/
|   +-- chunker/
|   +-- discovery/
|   +-- integrity/
|   +-- protocol/
|   +-- quic-transport/
|   +-- resume/
|   +-- transfer-core/
+-- services/
|   +-- control-plane/
|   +-- relay/
+-- .cargo/
+-- Cargo.toml
+-- README.md
```

## Prerequisites

- Rust `1.76+`
- Flutter SDK for mobile app development
- Tauri prerequisites for desktop packaging
- Go `1.22+` for backend services

## Setup

### Rust workspace

```powershell
cargo check --workspace
```

### Desktop-to-desktop QUIC prototype

The prototype is implemented with `quinn` and lives in the Rust workspace. A transfer source can be either a single file or a whole folder. Folder transfers now use a streaming pipeline: the sender scans recursively, emits metadata as items are discovered, and starts sending file chunks immediately instead of waiting for a full precomputed manifest. Files are chunked with configurable size, sent over multiple QUIC streams in parallel, and reassembled on the receiver with original relative paths preserved.

Start the receiver in one terminal:

`````powershell
cargo run -p transfer-core --bin ft-receive -- --bind 0.0.0.0:5000 --output-dir ./received
`````

On first run, the receiver generates a self-signed development certificate at `./.fasttransfer/certs/receiver-cert.der` and the matching key at `./.fasttransfer/certs/receiver-key.der`.

Discover nearby receivers from another terminal:

```powershell
cargo run -p transfer-core --bin ft-send -- discover --timeout-secs 3
```

Send a file or folder package to a specific receiver:

```powershell
cargo run -p transfer-core --bin ft-send -- send --to 127.0.0.1:5000 --source ./path/to/big-file.iso --cert ./.fasttransfer/certs/receiver-cert.der --chunk-size 1048576 --parallelism 4
```

For the Rust CLI prototype, manual target mode still expects the receiver certificate path via `--cert`. The desktop app's discovered-device flow now handles certificate trust automatically over LAN discovery.

### Local network discovery behavior

- `ft-receive` advertises its QUIC listener over mDNS as a FastTransfer receiver.
- The discovery payload includes the receiver device name, reachable socket addresses, certificate fingerprint, and the DER certificate bytes needed for QUIC/TLS trust.
- The sender can reconstruct the discovered receiver certificate locally, verify that it matches the advertised fingerprint, and use it without asking the user to browse for a `.der` file in the default desktop flow.
- The discovery crate keeps LAN browsing and advertising logic isolated from file-transfer orchestration.
- Discovery is intended for same-LAN scenarios such as shared Wi-Fi or a phone hotspot; it does not replace relay or NAT traversal.

### Local network discovery test flow

1. Put two devices on the same Wi-Fi network or the same hotspot.

2. On the receiving device, start the receiver:

```powershell
cargo run -p transfer-core --bin ft-receive -- --bind 0.0.0.0:5000 --output-dir ./received --device-name "Office Laptop"
```

3. On the sending device, list nearby receivers:

```powershell
cargo run -p transfer-core --bin ft-send -- discover --timeout-secs 5
```

4. In the desktop app, refresh nearby devices, pick the discovered receiver, confirm its short fingerprint, choose a file, and send. The app caches the receiver fingerprint locally with trust-on-first-use and reuses the discovered certificate automatically for the QUIC/TLS session.

5. If the same receiver later advertises a different certificate fingerprint, the desktop app reports a fingerprint mismatch and blocks the discovered-device send path until the change is handled deliberately.

6. For the current Rust CLI prototype, nearby discovery helps you find the receiver address, but manual sends still use `--cert` and `--source`:

```powershell
cargo run -p transfer-core --bin ft-send -- send --to 192.168.1.25:5000 --source ./path/to/large-file.iso --cert ./receiver-cert.der --chunk-size 1048576 --parallelism 4
```

7. If no receivers appear, make sure both devices are on the same LAN segment, local firewall rules allow mDNS and the QUIC port, and `ft-receive` is still running.

### Verification behavior

- The sender computes a SHA-256 for each file lazily right before that file is sent and includes the expected hash in the streamed file metadata.
- Each chunk stream also carries its own SHA-256 and the receiver verifies that hash before accepting the chunk.
- After reconstruction, the receiver computes the SHA-256 of each received file and compares it against the manifest entry for that relative path.
- Any chunk-hash mismatch or final-file mismatch fails the transfer clearly; corrupted data is not accepted silently.
- On success, the sender prints the package fingerprint and the receiver verifies every file before marking the package complete.

### Resume behavior

- Both sender and receiver persist streaming checkpoints under `.fasttransfer/resume-streaming/` (inside their local `.fasttransfer` state directory).
- When a transfer restarts, the receiver loads its checkpoint, validates streaming metadata, and tells the sender whether each discovered file is still needed.
- The sender skips files already marked complete and sends only the missing files; this is best-effort resume at file granularity for the current MVP.
- If a saved checkpoint does not match the new streaming metadata, the transfer fails safely instead of reusing incompatible partial state.
- Checkpoints are removed automatically after a successful verified transfer.

### Resume test flow

1. Start the receiver:

`````powershell
cargo run -p transfer-core --bin ft-receive -- --bind 0.0.0.0:5000 --output-dir ./received
`````

2. Start sending a large file or folder package:

```powershell
cargo run -p transfer-core --bin ft-send -- send --to 127.0.0.1:5000 --source ./path/to/large-file.iso --cert ./.fasttransfer/certs/receiver-cert.der --chunk-size 1048576 --parallelism 4
```

3. Interrupt either side before the transfer finishes.

4. Restart the receiver with the same `--output-dir`.

5. Restart the sender with the same source file, certificate, chunk size, and parallelism.

6. The restarted transfer should continue from the missing chunks only instead of retransmitting the whole package.

### Mobile app

The Flutter app is intentionally scaffolded as a monorepo placeholder. When you are ready to initialize the app shell:

```powershell
cd apps/mobile
flutter create .
```

### Desktop app

The desktop shell now includes a minimal Tauri interface on top of the existing Rust transfer engine. It provides:

- a native file picker for a single source file
- a native folder picker for recursive package transfer
- a nearby receivers list powered by the `discovery` crate
- automatic LAN trust for discovered receivers with visible device identity checks
- manual target entry as an advanced fallback
- a send action that uses `transfer-core`
- live sender progress and a receiver status view

Local run steps:

```powershell
cd apps/desktop
npm install
npm run tauri dev
```

During local development, the desktop app stores receiver certificates and trust-on-first-use records under `apps/desktop/.fasttransfer-desktop/`. Received files go to `Downloads/FastTransfer` by default, with an automatic fallback to `Desktop/FastTransfer` when Downloads is unavailable.

Default desktop LAN flow:

1. Start the receiver on PC1 from the desktop app.
2. On PC2, refresh nearby devices.
3. Select PC1 from the nearby device list and confirm its device name, short fingerprint, and trust state.
4. Pick a file or folder and review the package summary.
5. Press Send.
6. The sender automatically binds the discovered receiver certificate to that session and connects over QUIC/TLS without a manual certificate picker.
7. If the receiver certificate changes later, the app marks the device as a fingerprint mismatch and blocks the discovered-device send path until you handle it deliberately.

### Control plane

```powershell
cd services/control-plane
go run .
```

## Rust Crates

- `protocol`: shared wire-level types and control/chunk framing.
- `chunker`: deterministic file chunk planning.
- `integrity`: SHA-256 and checksum helpers.
- `resume`: persistent resume checkpoints and completed-chunk tracking.
- `discovery`: local network discovery helpers for advertising and browsing receivers over mDNS.
- `quic-transport`: QUIC transport wiring built on `quinn`.
- `transfer-core`: chunk scheduling, resume orchestration, reassembly, integrity verification, progress reporting, and CLI binaries.

## Folder Transfer Behavior

- You can send either a single file or a whole folder from the CLI and the desktop app.
- Folder transfers use paths relative to the selected root, never the sender''s absolute filesystem paths.
- Empty directories are preserved and recreated on the receiver before any file chunks are written.
- Progress is package-wide and reports total transferred bytes, completed files, and the current relative path being transferred.
- Resume metadata is tracked for the full package, and interrupted folder transfers continue from missing files on restart (partial files may be resent in this MVP).

## Folder Transfer Test Flow

1. Create a test folder with nested subfolders, several files, and at least one empty directory.
2. Start the receiver:

```powershell
cargo run -p transfer-core --bin ft-receive -- --bind 0.0.0.0:5000 --output-dir ./received
```

3. Send the folder package:

```powershell
cargo run -p transfer-core --bin ft-send -- send --to 127.0.0.1:5000 --source ./path/to/test-folder --cert ./.fasttransfer/certs/receiver-cert.der --chunk-size 1048576 --parallelism 4
```

4. Confirm that the receiver recreates the same folder tree under the output directory, including empty folders.
5. Compare a few nested files and verify that the transfer completes with integrity verification.
6. Interrupt and restart the same transfer to confirm resume continues from missing chunks only.
## Prototype Notes

- Chunk size is configurable from the sender CLI with `--chunk-size`.
- Parallelism is configurable from the sender CLI with `--parallelism`.
- Progress is reported as chunks complete, not as individual stream writes occur.
- The receiver writes chunk payloads into the correct file offsets to preserve final file order.
- The receiver refuses to overwrite an existing output file unless it is resuming a matching checkpointed transfer.
- The current LAN auto-trust flow is a development-oriented trust-on-first-use model and should be replaced with stronger authenticated identity for production deployments.

## Next Steps

1. Replace the development trust-on-first-use LAN flow with authenticated device identity and certificate lifecycle management.
2. Add retry windows and selective retransmission for chunks that were in flight during disconnects.
3. Introduce relay negotiation for NAT traversal beyond the local network.
4. Scaffold the Flutter and Tauri applications against the Rust core via FFI.




## Desktop Destination Modes

The desktop Send view now supports four destination types:

- Nearby device (default LAN flow with discovery and automatic certificate trust)
- Manual address (advanced fallback with explicit certificate path)
- USB / external drive (direct local copy to removable media)
- Local folder (direct local copy to any selected folder)

For USB and local-folder modes, FastTransfer now uses a destination-sink pipeline in `transfer-core` (`LanSink`, `LocalFolderSink`, `UsbDriveSink`) and streams file data directly to disk without loading whole packages into memory.

Safety checks before local copy starts:

- free-space check on the destination volume
- FAT32 4 GiB single-file limit detection with a clear error message
- removable-drive type validation for USB mode
- runtime detection if a USB drive is removed during transfer
- permission and filesystem write errors are surfaced as transfer failures

The desktop app reuses the same package summary, transfer progress, and transfer queue UI for LAN and local copy destinations.

## Desktop Local Copy Test Flow

1. Open the desktop app and go to `Send`.
2. Choose a source file or folder.
3. Select `USB / External drive` or `Local folder` as the destination mode.
4. For USB mode, refresh and pick a removable drive.
5. For local-folder mode, browse and pick a destination folder.
6. Click `Send package`.
7. Verify progress updates (current file, overall bytes/files, speed) and confirm files arrive with preserved folder structure.

