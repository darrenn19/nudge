# Nudge

Nudge is a free, open-source app for fast, seamless file transfer between phones and computers — an AirDrop-style experience for people outside the Apple ecosystem. The first target is **Android (Samsung) ↔ Windows**; macOS and iOS come later.

Full design rationale: see `docs/DESIGN.md`. Read it before making architectural changes.

## Goals (in priority order)
1. **Speed** — transfers should be as fast as the hardware allows, for any file size.
2. **Seamless feel** — one tap to send, no accept prompts between paired devices, no choices the app can make itself.
3. **Clean UI** — minimal, calm, native-feeling on each platform.
4. **Private** — no cloud, no accounts, no telemetry. Data goes device to device only.

## Key decisions (don't change without discussion)
- **License:** MIT or Apache-2.0 (permissive — GPL conflicts with the iOS App Store).
- **Core engine in Rust**, shared by every platform (tokio for async I/O). Networking, crypto, discovery, file handling all live here.
- **UI:** Flutter + flutter_rust_bridge is the default plan (one UI codebase). Native UIs (Jetpack Compose / WinUI 3 / SwiftUI) are the alternative if polish demands it.
- **Transport:** TLS 1.3 over TCP on the local network. Not QUIC by default (higher CPU on phones at gigabit speeds); QUIC may be added later for cross-network transfers.
- **Never route files through a server.**

## Transfer strategy: "start then upgrade"
The app never asks the user which path to use.
1. On send, start streaming immediately over the **router** (shared Wi-Fi) path.
2. In parallel, bring up a **direct link** (Android local-only hotspot, or Wi-Fi Direct) in the background.
3. After a few seconds, compare measured speed vs. the expected direct speed for the *remaining* bytes.
4. If direct wins meaningfully (≥20% faster AND saves ≥10 s), switch and **resume from the exact byte offset**.
5. If a **data cable** is connected (any kind: USB-C, micro-USB, Lightning), prefer it (fastest of all). Files go over it once the phone shares a network on it (USB tethering); an app can't switch that on, and Android switches it off whenever the cable comes out, so the phone asks once, steering to Developer options → Default USB configuration → USB tethering (it then comes on by itself), and neither side asks again (`UsbCable.kt`). A phone's cable is recognised by its driver, not its adapter name (`core/src/link.rs`).
- Store real measured speeds per device pair + path, and use that history for estimates (not hard-coded numbers).
- Show the ETA as information, never as a question. Keep a manual override in settings ("always use router").
- Fallback: on phones that can't host a hotspot while staying on Wi-Fi, decide up front from history.

## Performance rules
- Stream continuously over one connection with large buffers (1–4 MB). No chunk-and-wait-for-ack.
- Pack many small files into one stream (tar-style) and unpack on arrival.
- Hash while streaming with BLAKE3; verify on arrival.
- Track byte offsets so any interrupted transfer resumes instead of restarting.
- Keep encryption ON (AES-GCM is hardware-accelerated; it costs almost nothing).
- Don't compress media. Optionally zstd for text-like files only.
- Android: write via raw file descriptors with big buffers; run transfers in a foreground service; request battery-optimization exemption.
- Windows: preallocate file size before writing.
- Open/warm the connection when the share sheet opens, before the user taps Send.

## Discovery & pairing
- mDNS discovery + UDP broadcast fallback.
- Pair once via QR code (exchanges certificates). Paired devices auto-accept with no prompt.

## UX integration
- Android: share-sheet target, quick settings tile, "received" notification with an Open action.
- Windows: tray icon (drag files onto it), Explorer right-click "Send with Nudge".
- macOS (later): menu-bar icon, Finder share extension.

## Seamless mode (current focus)
Goal: AirDrop-like behaviour — send/receive with the app closed, receive while locked, send from anywhere in the OS.

**Architecture rule:** the network listener and transfer engine run in a background process, never in the UI. The UI only controls it.

### Background process
- **Android:** Rust core runs inside a native Kotlin **foreground service** (do NOT run it through the Flutter engine; call Rust via JNI/UniFFI). Ongoing "Nudge is ready" notification.
  - `foregroundServiceType="connectedDevice"` for the always-on listener (don't use `dataSync` for it; Android 15 caps that at ~6h/day — only use it for active transfers if needed).
  - Start at boot (`BOOT_COMPLETED` receiver); restart if killed.
  - Request the battery-optimization exemption (`REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`) so Doze doesn't cut network access.
  - Samsung: onboarding must guide the user to add Nudge to **Never sleeping apps** (Settings → Battery → Background usage limits). See dontkillmyapp.com.
  - Reconnect fast: try each paired device's last known IP first, then mDNS. Hold the multicast lock only during discovery.
- **Windows:** tray app that auto-starts at login (MSIX `windows.startupTask`, or registry Run key). Closing the window hides to tray; Quit lives in the tray menu. Installer adds firewall rules for all network types (home Wi-Fi is often marked Public; only paired devices can send).

### Receiving while locked
- Android foreground service keeps receiving while locked. After a reboot, before first unlock, user storage is encrypted: queue or reject with "unlock phone once" message.
- Windows tray app receives while the session is locked. Signed-out/asleep is out of scope (would need a Windows service).
- Security: only **paired** devices may send while locked. Setting: "Accept files while locked: Paired devices / Ask me / Never". Received-file notifications use private lock-screen visibility.

### Sending from a locked device
- Lock-screen Quick Settings tile actions ("Send latest screenshot", "Send clipboard"); a setting decides whether the tile requires unlock.
- Opt-in "pull from laptop" (laptop requests e.g. the latest photo from a locked phone) — paired devices only, off by default.

### OS integrations
**Android**
- Share sheet: `ACTION_SEND` / `ACTION_SEND_MULTIPLE` on a lightweight no-UI activity that hands files to the service and finishes.
- **Direct Share targets:** publish a sharing shortcut per paired device (`ShortcutManagerCompat`) so "My Laptop" appears in the share sheet's top row.
- Text selection menu: `ACTION_PROCESS_TEXT` → "Send to laptop".
- Quick Settings tile: `TileService`.
- Opt-in post-screenshot notification action (MediaStore observer; needs photo permission).
- Later: `DocumentsProvider` (laptop as a "Save to" location); exported intent API for Tasker/automation.

**Windows**
- "Send to" menu: shortcut in `shell:sendto` (do first — trivial).
- Win11 top-level right-click "Send with Nudge": `IExplorerCommand` + package identity (MSIX or sparse package).
- Windows Share panel: Share Target contract (needs package identity).
- Tray icon drag-and-drop; global hotkey via `RegisterHotKey` (e.g. send clipboard).
- CLI (`nudge send <file>`) and `nudge://` URI protocol handler.
- Package as MSIX (or add a sparse package) early — it unlocks the best integrations.

### Seamless mode build order
1. Android foreground service + Windows tray auto-start (everything depends on this).
2. Android share sheet + Direct Share targets; Windows "Send to".
3. Receiving while locked + security settings.
4. Quick Settings tile, text selection; Windows right-click menu + Share Target.
5. Lock-screen tile actions and pull-from-laptop (opt-in).

**Testing:** use a real Samsung phone, locked overnight, then send to it in the morning — Doze and Samsung sleeping-apps behaviour don't reproduce in emulators.

## Building
- Windows MSIX: `powershell -File app\windows\packaging\package.ps1` (`-Install` registers it on this PC in Developer Mode). The package provides the Win11 top-level menu (`app/windows/shell`), Share panel, startup task and firewall rules.
- Android: the Rust core runs in the process via JNI (`app/rust/src/android.rs` ↔ `Core.kt`); `Background.kt` handles events with no window. The Flutter window attaches to the same core.

## Platform notes
- Android 10+ blocks background clipboard reads — clipboard sync phone→PC needs a tap (notification/share action) or a custom keyboard.
- iOS (later): app can't create a hotspot (it joins one instead); background time is limited; share extensions have tight memory limits (hand off to main app); Local Network permission prompt; no USB file access; iPhone↔Mac can use AWDL via Network.framework peer-to-peer.

## Build order
1. Rust core: stream one file over TLS between two desktop processes, with resume + BLAKE3 verify + a throughput benchmark.
2. Android + Windows shells with discovery and QR pairing.
3. Seamless mode: background process, locked receiving, OS integrations (see "Seamless mode" above).
4. Multi-file packing and folder transfer (packs: `core/src/pack.rs`; folders from Windows and Android, empty ones included).
5. Direct-link mode and start-then-upgrade switching. (done: `core/src/node/paths.rs`)
6. Cable path, any data cable, via USB tethering. (done; a cable without tethering, over Android Open Accessory, would need a Windows USB driver)
7. macOS, then iOS. ← next

## Working conventions
- Measure before optimizing: every performance change should come with benchmark numbers.
- Keep the core UI-agnostic; platform code only calls the core's public API.