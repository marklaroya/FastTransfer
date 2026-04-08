# FastTransfer Desktop

Minimal Tauri desktop shell for the existing FastTransfer transfer engine.

## Local run

```powershell
cd apps/desktop
npm install
npm run tauri dev
```

## Default LAN flow

1. Start the receiver from the right-hand panel on PC1.
2. On PC2, refresh nearby devices.
3. Pick the discovered receiver from the sender panel.
4. Confirm the device name, short fingerprint, and trust state.
5. Choose a file or folder and review the package summary.
6. Send.

Discovered receivers are auto-trusted with trust-on-first-use for the session. The desktop app caches each discovered device fingerprint under `apps/desktop/.fasttransfer-desktop/trust/` and warns if the fingerprint changes later.

Incoming packages are saved to `Downloads/FastTransfer` by default, with an automatic fallback to `Desktop/FastTransfer` when the Downloads folder is unavailable. Folder transfers preserve the selected root name, nested folders, empty directories, and relative file paths. The UI shows the simplified folder label while the backend keeps using the absolute path internally.

## Advanced fallback

If a receiver was not discovered automatically, open the advanced manual target fallback in the sender panel and provide:

- the receiver address
- the receiver certificate path

The app stores local receiver runtime data under `apps/desktop/.fasttransfer-desktop/` during development.

## File And Folder Transfers

- Use Select file for a single-file transfer.
- Use Select folder for a recursive package transfer.
- The sender shows a package summary before transfer: root name, root type, total files, total folders, and total bytes.
- Progress updates show package-wide bytes plus the current file path.
