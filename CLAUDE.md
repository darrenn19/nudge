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
7. Screen mirroring (see below). ← current: laptop controls the phone (mirroring step 5)
8. macOS, then iOS. Deferred: no Apple device to build or test on yet.

## Screen mirroring (in progress: laptop views the phone works; control next)
Goal: better than Microsoft Phone Link on speed, smoothness and integration. Paired devices only. Two modes:
- **Regular:** see the other device's screen and control it (tap, swipe, type) with very low latency.
- **Connected:** the same, plus the mirror window works as a doorway between the devices: copy/paste, drag files in and out, notifications. It should feel like the other device is part of this one.

**What's realistic:**
- **Phone → laptop video:** MediaProjection + hardware MediaCodec (low-latency mode). Measured on the S24 (`docs/BENCHMARKS.md`): ~35 ms median over cable, ~30–35 ms over Wi-Fi when steady, before ~10–25 ms of display time. No software method gets near zero: every product (scrcpy, Phone Link, game streaming) uses this pipeline; we win by tuning each stage.
- **Laptop → phone control:** only through an Accessibility Service (`dispatchGesture`, global actions). Taps, swipes, scrolling and Back/Home are fine. Continuous drags lag the video a little. Typing needs a custom IME or `ACTION_SET_TEXT`; neither is seamless. adb-level injection (scrcpy) and Samsung's private APIs (Phone Link) aren't available to us.
- **Laptop → phone video and control:** easy (Windows Graphics Capture + `SendInput`). Can't reach elevated windows, UAC or the lock screen.
- **iOS (later):** can be viewed (ReplayKit), never controlled.
- **Android limits:** a consent prompt for every session (Android 14+ allows no reuse); the phone must be unlocked; `FLAG_SECURE` content (banking, DRM, passwords) shows black; Android 15 hides sensitive notifications while sharing.
- **Connected mode, doable:**
  - dropping files on the window (a normal Nudge transfer: lands in Downloads, can't target a specific app);
  - clipboard (already built; phone → laptop copy has the Android 10+ background limit);
  - a recent photos/files tray to drag from;
  - dragging out a full-resolution crop of the screen;
  - notifications with reply;
  - phone audio (AudioPlaybackCapture, Android 10+).
- **Connected mode, not doable:** dragging the file under the finger out of an arbitrary phone app, or per-app windows. Both need system or adb access.

**Architecture:**
- Reuse pairing certificates, discovery and paths: the cable and hotspot upgrades matter even more for latency.
- Video needs its own real-time channel: UDP-based (QUIC datagrams or similar) that drops stale frames, encrypted with the pairing certificates. Not the file stream: TCP head-of-line blocking stalls on lossy Wi-Fi.
- Render video natively on Windows (a native child window), not through a Flutter texture: that adds a frame of latency.
- Keep the core UI-agnostic as usual.

**Safety:**
- Always confirmed on the shared device, with a visible "being shared" indicator. Never started from a locked phone.
- Google Play restricts non-accessibility uses of Accessibility Services: expect review friction and needs a prominent disclosure. Sideloaded builds aren't affected.

**Build order (rough effort, one person):**
1. **Latency spike + tuning** (throwaway, `spikes/mirror-latency` on the PC, `app/android/app/src/debug/.../spike/` on the phone; how to run, including unattended runs over adb: the spike's README). Done 2026-10-28; numbers in `docs/BENCHMARKS.md`.
   - Result over Wi-Fi via the direct link, 90 s: median 18.7 ms, p95 32, p99 48, 0 lost frames, 0 freezes (before display time). Through the mesh router: no lost frames either, but p99 ~100 ms.
   - **Settings that carry over:** H.264 High, CBR 30 Mbit/s, 1056×2304, BT.709 limited signalled; intra-refresh (30 frames), no periodic keyframes; encoder speed-ups (`vendor.qti-ext-enc-low-latency.enable`, `KEY_OPERATING_RATE` max, `KEY_LATENCY` 1, `KEY_LOW_LATENCY`): encode 20.5 → 10.7 ms; paced sending (200 Mbit/s cap); phone Wi-Fi low-latency + high-perf locks; 1 s of resend history; PC: GPU decode (Media Foundation), D3D11 video processor, waitable swap chain with newest frame winning, 1 ms timers.
   - **Decision: mirroring over Wi-Fi uses the direct link** (the phone's Wi-Fi Direct group, as for big transfers). Through the router Wi-Fi pauses 50–300 ms; direct, the network max is ~50 ms. Catch: while joined, the PC is off its usual Wi-Fi.
   - Tried, no effect: the PC sending a tiny packet every 5 ms to stop its Wi-Fi dozing.
2. **Real-time channel in the core** (~1 week): encrypted UDP datagrams keyed by the pairing certificates, stale-frame dropping, a session handshake, and path selection (cable > hotspot > router). Testable between two desktop processes, no phone needed.
   - [x] Built (2026-10-28): `core/src/realtime/`. Handshake over an existing TLS connection; keys from TLS key export (bound to both certificates); AES-256-GCM per direction with replay window; fragments with NACK recovery (per-fragment tries, resends unpaced, give up after 50 ms), sender pacing that catches up for coarse timers, a complete keyframe supersedes earlier gaps; path selection by pinging every address (cable > wired > hotspot > Wi-Fi, switch only to a better kind or when the current one dies). Tests: `core/tests/realtime.rs`; two processes: `nudge mirror-bench --listen` / `--to <host>` (no flags: both in one process; `--drop P` simulates loss).
   - [x] Wired into `Node` as request kind 11 (`Mirror`), no version bump: older builds just refuse it ("needs an update"), file transfer unaffected. `Node::start_mirror` / `answer_mirror` / `take_mirror` / `stop_mirror` / `mirrors`, events `MirrorAsked` / `MirrorStarted` / `MirrorEnded` (`core/src/node/mirror.rs`).
   - [x] 1 ms Windows timers while a session runs (`app/rust/src/timer.rs`; `nudge mirror-bench` does the same). With 3% loss: p95 16.5 → 6.4 ms. Plus a tail-loss wait that adapts to the sender's pacing (4–30 ms instead of a fixed 30): p95 4.5, p99 10.6 ms.
   - [x] Recovery on *link time* (a silence counts as at most 20 ms), 5 NACK tries, 300 ms wall backstop, 1 s / 64 frames of history: a Wi-Fi pause no longer turns late frames into skipped ones (spike through the router: 27–149 skipped → 0).
   - [ ] Try it between the PC and a second machine or the phone (once the phone side exists).
3. **Session and safety plumbing** (~3–4 days): start/stop, the consent flow on the phone, the "being shared" indicator, paired-only checks. Do it before the features so they never exist without it. Done.
   - [x] In the core, enforced for every feature: paired only; the shared device's user is asked (`ShowMe`) or started it (`ShowYou`); a locked device never shares, and sharing ends when it locks (`Node::set_screen_locked`, from `LockState.kt` on Android); unclaimed sessions end after 15 s. Tests in `core/src/node.rs` (`mirroring_*`).
   - [x] Android: "wants to see your screen" notification (Allow / Don't allow, shown even with the window open), "Your screen is being shared" ongoing notification with Stop. Windows declines requests to share its own screen (not built yet).
   - [x] Allow opens Android's screen-capture prompt (whole screen preselected, Android 14+) and answers only after it (`MirrorConsentActivity`). "Being shared" is `MirrorService`'s notification (own group, so Android doesn't fold it away), plus Android's own red chip.
4. **Laptop views the phone** (~2–3 weeks): the spike rebuilt on the real channel; native Windows renderer; rotation, resolution changes, reconnect when switching paths. Measure again. Done 2026-10-28 (uncommitted); tested on the S24.
   - [x] Phone: `MirrorService.kt` (MediaProjection → H.264 with the step 1 settings → `Core.mirrorSend` → `app/rust/src/android_mirror.rs` → the session). Started from Allow on "wants to see your screen", or "Show my screen on <PC>" in the phone's device menu.
   - [x] PC: `app/rust/src/mirror/` (the spike's GPU decode and draw, its own window and thread), "Show its screen" in a paired phone's menu; opens by itself when the phone offers. The window and decoder open before the first frame. Logs capture-to-screen every 10 s (`%APPDATA%\Nudge\nudge.log`, "mirror:").
   - [x] Rotation: the phone re-creates the encoder at the new shape, the PC window re-fits (~0.25 s).
   - [x] Path changes: the session outlives its TLS connection breaking, each side sends its addresses every second (`ADDRS`), so a cable pulled out moves it to Wi-Fi and a new network becomes a path; it ends on a clean close, a bye, or 10 s with no path. Test: `core/tests/realtime.rs` (`a_broken_setup_connection_doesnt_end_the_session`).
   - [x] Direct link for Wi-Fi: the PC moves onto the phone's Wi-Fi Direct network *before* asking (no mid-stream freeze), Wi-Fi Direct only (`Link::AskDirect`; the local-only hotspot fallback made p95 300-500 ms), retried up to 3 times; kept while mirroring, ended 30 s after; the PC rejoins its usual Wi-Fi (retrying while the adapter rescans). Respects the "Join the phone's hotspot" setting.
   - [x] Measured (app, S24 → this PC): cable, 60 s steady: median 16-19 ms, p95 18-21, p99 20-31, max 34. Wi-Fi via the direct link: median 22-30, p95 29-42, p99 34-47. Before display time.
   - [ ] Left: the wait for the PC display's next refresh (arrival-to-screen p50 drifts 1-8 ms as the phone's 60 fps beats against the monitor; tearing would avoid it), one-off spikes at session start, the PC being off its home Wi-Fi while mirroring over the direct link (Windows' Wi-Fi Direct API could keep both), Windows doesn't share its own screen yet (step 7).
5. **Laptop controls the phone** (~2 weeks + tuning): Accessibility Service taps, swipes, scroll, Back/Home/Recents first; then typing via `ACTION_SET_TEXT` (a custom IME only if that's not good enough).
6. **Connected mode** (~3–4 weeks), cheapest first: file drop and clipboard (both reuse what's built), notifications with reply, recent-media tray, crop drag-out, then phone audio (shares the MediaProjection session).
7. **Phone views and controls the laptop** (~2–3 weeks): same channel, reversed (Windows Graphics Capture + Media Foundation encoder, `SendInput`).
iOS viewing (ReplayKit) moves to the iOS phase.

## Distribution & first run
Goal: anyone can install from a store and be sending files within a minute. No dev tools, no adb, no Developer options for core features.

**Where it ships:** Google Play, F-Droid, GitHub Releases (APK + Windows installer), Microsoft Store (MSIX). Store builds must need nothing else installed (H.264 by default; H.265 only if the PC has it).

**First run (phone):** one guided welcome flow, asking only when needed, each with a one-line reason:
1. Notifications → 2. Camera (QR pairing, only when pairing) → 3. Nearby Wi-Fi devices → 4. Battery exemption → 5. Samsung only: steer to "Never sleeping apps".
Everything else is asked the first time a feature needs it, never up front.

**Core vs. power-user features:**
- Core (no special setup): send/receive, pairing, share sheet, Quick Settings tile, clipboard with a tap, mirroring over Wi-Fi.
- Optional extras (clearly labelled, never in onboarding): cable auto-tethering (Developer options), tap-free clipboard (adb `READ_LOGS`), H.265 (HEVC Video Extensions).
- Permissions a store may reject (`READ_LOGS`, possibly `SYSTEM_ALERT_WINDOW`) go only in a separate build (e.g. GitHub/F-Droid), not the Play build.

**Unavoidable prompts (Android rules):** screen-capture consent every mirroring session; Accessibility switch for controlling the phone (asked when control is first used).

**Windows:** installer/MSIX handles auto-start and firewall rules; no prompts after install. Outside the Store, sign the installer (e.g. Azure Trusted Signing) or users see "unknown publisher" warnings.

**Rule for new features:** if a feature needs a manual setting, a scary warning screen or a dev tool, it's opt-in and asked at first use, not part of setup.

## Working conventions
- Measure before optimizing: every performance change should come with benchmark numbers.
- Keep the core UI-agnostic; platform code only calls the core's public API.


TODO LIST
- Local device controls in screen mirroring
- Speaker choice selection
- Open file from notification (no need to open app)
- Full screen mode in portrait/landscape mode during screen mirroring

POSSIBLE IDEAS

- Extended display (like PCs)
- Play sound to find device
- Nudge MCP for developing on mobile
- Shared notifications


FUTURE NEEDS
- Logo
- Better Readme + license
- Youtube Vids