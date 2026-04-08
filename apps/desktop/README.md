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
5. Choose a file and send.

Discovered receivers are auto-trusted with trust-on-first-use for the session. The desktop app caches each discovered device fingerprint under `apps/desktop/.fasttransfer-desktop/trust/` and warns if the fingerprint changes later.

## Advanced fallback

If a receiver was not discovered automatically, open the advanced manual target fallback in the sender panel and provide:

- the receiver address
- the receiver certificate path

The app stores local receiver runtime data under `apps/desktop/.fasttransfer-desktop/` during development.
