# FastTransfer Monorepo

FastTransfer is a P2P-first cross-platform file transfer system focused on high-throughput, resumable, integrity-checked transfers across Android, iOS, and desktop platforms.

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

The prototype is implemented with `quinn` and lives in the Rust workspace. Files are split into configurable chunks, sent over multiple QUIC streams in parallel, and reassembled on the receiver by chunk offset.

Start the receiver in one terminal:

```powershell
cargo run -p transfer-core --bin ft-receive -- --bind 0.0.0.0:5000 --output-dir ./received
```

On first run, the receiver generates a self-signed development certificate at `./.fasttransfer/certs/receiver-cert.der` and the matching key at `./.fasttransfer/certs/receiver-key.der`.

Discover nearby receivers from another terminal:

```powershell
cargo run -p transfer-core --bin ft-send -- discover --timeout-secs 3
```

Send a file to a specific receiver:

```powershell
cargo run -p transfer-core --bin ft-send -- send --to 127.0.0.1:5000 --file ./path/to/big-file.iso --cert ./.fasttransfer/certs/receiver-cert.der --chunk-size 1048576 --parallelism 4
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

6. For the current Rust CLI prototype, nearby discovery helps you find the receiver address, but manual sends still use `--cert`:

```powershell
cargo run -p transfer-core --bin ft-send -- send --to 192.168.1.25:5000 --file ./path/to/large-file.iso --cert ./receiver-cert.der --chunk-size 1048576 --parallelism 4
```

7. If no receivers appear, make sure both devices are on the same LAN segment, local firewall rules allow mDNS and the QUIC port, and `ft-receive` is still running.

### Verification behavior

- The sender computes the full-file SHA-256 before transfer and includes it in the transfer manifest.
- Each chunk stream also carries its own SHA-256 and the receiver verifies that hash before accepting the chunk.
- After reconstruction, the receiver computes the SHA-256 of the final file and compares it against the manifest hash.
- Any chunk-hash mismatch or final-file mismatch fails the transfer clearly; corrupted data is not accepted silently.
- On success, the sender prints the expected SHA-256 and the receiver prints the verified SHA-256.

### Resume behavior

- Both sender and receiver persist chunk-completion checkpoints under a local `.fasttransfer/resume/` directory.
- When a transfer restarts, the receiver loads its checkpoint, validates the manifest metadata, and tells the sender which chunks are still missing.
- The sender then sends only the missing chunks and skips the chunks that were already completed.
- If a saved checkpoint does not match the new manifest metadata, the transfer fails safely instead of reusing incompatible partial state.
- Checkpoints are removed automatically after a successful verified transfer.

### Resume test flow

1. Start the receiver:

```powershell
cargo run -p transfer-core --bin ft-receive -- --bind 0.0.0.0:5000 --output-dir ./received
```

2. Start sending a large file:

```powershell
cargo run -p transfer-core --bin ft-send -- send --to 127.0.0.1:5000 --file ./path/to/large-file.iso --cert ./.fasttransfer/certs/receiver-cert.der --chunk-size 1048576 --parallelism 4
```

3. Interrupt either side before the transfer finishes.

4. Restart the receiver with the same `--output-dir`.

5. Restart the sender with the same source file, certificate, chunk size, and parallelism.

6. The restarted transfer should continue from the missing chunks only instead of retransmitting the whole file.

### Mobile app

The Flutter app is intentionally scaffolded as a monorepo placeholder. When you are ready to initialize the app shell:

```powershell
cd apps/mobile
flutter create .
```

### Desktop app

The desktop shell now includes a minimal Tauri interface on top of the existing Rust transfer engine. It provides:

- a native file picker for the source file
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

During local development, the desktop app stores receiver certificates, trust-on-first-use records, and received files under `apps/desktop/.fasttransfer-desktop/`.

Default desktop LAN flow:

1. Start the receiver on PC1 from the desktop app.
2. On PC2, refresh nearby devices.
3. Select PC1 from the nearby device list and confirm its device name, short fingerprint, and trust state.
4. Pick a file and press Send.
5. The sender automatically binds the discovered receiver certificate to that session and connects over QUIC/TLS without a manual certificate picker.
6. If the receiver certificate changes later, the app marks the device as a fingerprint mismatch and blocks the discovered-device send path until you handle it deliberately.

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
