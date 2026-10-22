# Nudge

Fast, seamless, private file transfer between phones and computers: an AirDrop-style experience outside the Apple ecosystem. First target: **Android ↔ Windows**.

- **Fast:** streams over the best available path and switches to a faster one mid-transfer.
- **Seamless:** one tap to send, no accept prompts between paired devices.
- **Private:** device to device only. No cloud, no accounts, no telemetry.

> Status: early development. Phases 1–3 are done: the core engine; Android and Windows apps that find each other, pair once by QR code and send with one quick Accept (or none, for devices set to always accept); and system integration (Android share sheet, quick settings tile and notifications; Windows tray, drag and drop and Explorer's "Send with Nudge"). Next: multi-file packing and folder transfer (Phase 4).

## Layout
| Path | What it is |
|---|---|
| `core/` | `nudge-core`: Rust engine shared by every platform (transfer, TLS, discovery, pairing) |
| `cli/` | `nudge`: command-line tool for testing and benchmarks |
| `app/` | Flutter app for Android and Windows; `app/rust/` is its bridge to the core |
| `docs/` | [Design](docs/DESIGN.md), [protocol](docs/PROTOCOL.md), [benchmarks](docs/BENCHMARKS.md) |

## Building the core and CLI
Requires Rust (stable, installed via [rustup](https://rustup.rs)). On Windows you also need the MSVC C++ build tools.

```sh
cargo build --release
cargo test
```

## Building the app
Keep the project **outside OneDrive** (or any synced folder): Dart can't resolve local package paths there, and the build output is several GB.

You need:
- [Flutter](https://docs.flutter.dev/get-started/install) (stable) and Rust.
- **Windows app:** Visual Studio 2022 (or Build Tools) with *Desktop development with C++*, and Windows **Developer Mode** turned on (Settings → System → For developers), which Flutter needs for plugin symlinks.
- **Android app:** the Android SDK with JDK 17, plus the Rust Android targets:
  `rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android`.
  Gradle downloads the NDK on the first build.

```sh
cd app
flutter run -d windows        # or: flutter build apk
```

The Rust bridge builds automatically as part of the Flutter build. After changing `app/rust/src/api/`, regenerate the Dart bindings:

```sh
cargo install flutter_rust_bridge_codegen --version 2.13.0
cd app && flutter_rust_bridge_codegen generate
```

## Using the app
1. Open Nudge on both devices, on the same Wi-Fi.
2. On the computer, tap the QR icon (**Pair a device**). On the phone, do the same and scan the computer's code.
3. Tap a paired device to choose files and send. Files arrive in **Downloads** (Windows) or **Download/Nudge** (Android).

**Sharing with someone without Nudge:** tap the globe icon (**Share without the app**) and have them scan the code with their phone camera. They need to be on the same Wi-Fi. A page opens in their browser and they show up as a device: files you send to it wait for them to tap Accept, and they can send files back from the page. It stops after 10 minutes with nobody connected. The page isn't encrypted, so use it on networks you trust. On Android you can also start from another app: share to **Share by QR code** (or pick **Someone without Nudge** in Nudge's device list), and the files go to the first person who opens the page.

Neither device needs Nudge open to receive. On Windows it starts with Windows, hidden in the tray (untick **Start with Windows** in the tray menu to stop that), and shows a toast for each file. On Android it runs in the background with a minimised "Ready to receive" notification, starts at boot, and shows a notification for each file. A phone with its screen off can still receive: the computer sends to where it last saw it.

## Trying it from the command line
The `nudge` tool (at `target/release/nudge`) does everything the apps do, without a UI.

```sh
nudge serve --dir ~/Downloads   # be discoverable and accept files from paired devices
nudge pair                      # show a pairing QR code in the terminal and wait
nudge pair "nudge:pair?..."     # pair with a device showing a code
nudge devices                   # list nearby and paired devices
nudge send photo.jpg <host>     # send a file to a host directly
nudge bench --disk              # measure speed and CPU use
```

On Windows the CLI and the app share one identity (`%APPDATA%\Nudge`), so pairing in one counts for both. An interrupted transfer resumes when the same file is sent again.

## License
Licensed under [MIT](LICENSE-MIT).
