# Nudge

Nudge is a free, open-source app for fast, seamless file transfer between phones and computers — an AirDrop-style experience for people outside the Apple ecosystem. The first target is **Android (Samsung) ↔ Windows**; macOS and iOS come later.

Full design rationale: see [docs/DESIGN.md](docs/DESIGN.md). Read it before making architectural changes.

## Goals (in priority order)
1. **Speed** — transfers should be as fast as the hardware allows, for any file size.
2. **Seamless feel** — one tap to send, one quick Accept per send (or none, for devices set to always accept), no choices the app can make itself.
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
5. If a **USB-C cable** is connected, prefer it (fastest of all).
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
- Pair once: tap a nearby device and confirm a matching six-digit code on both screens, or scan a QR code (exchanges certificates). The receiver is asked once per send (Accept / Decline, in the app, a notification or a toast); each paired device can be set to 'always accept', which skips the question. The sender is told Accepted / Declined. Pairing again with a reinstalled device replaces its old entry.

## UX integration
- Android: share-sheet target, quick settings tile, "received" notification with an Open action.
- Windows: tray icon (drag files onto it), Explorer right-click "Send with Nudge".
- macOS (later): menu-bar icon, Finder share extension.

## Platform notes
- Android 10+ blocks background clipboard reads — clipboard sync phone→PC needs a tap (notification/share action) or a custom keyboard.
- iOS (later): app can't create a hotspot (it joins one instead); background time is limited; share extensions have tight memory limits (hand off to main app); Local Network permission prompt; no USB file access; iPhone↔Mac can use AWDL via Network.framework peer-to-peer.

## Build order
1. Rust core: stream one file over TLS between two desktop processes, with resume + BLAKE3 verify + a throughput benchmark.
2. Android + Windows shells with discovery and QR pairing.
3. Share sheet / tray / context-menu integration.
4. Multi-file packing and folder transfer.
5. Direct-link mode and start-then-upgrade switching.
6. USB-C cable path.
7. macOS, then iOS.

## Working conventions
- Measure before optimizing: every performance change should come with benchmark numbers.
- Keep the core UI-agnostic; platform code only calls the core's public API.
