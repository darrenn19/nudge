# Nudge

Fast, seamless, private file transfer between phones and computers: an AirDrop-style experience outside the Apple ecosystem. First target: **Android ↔ Windows**.

- **Fast:** streams over the best available path and switches to a faster one mid-transfer.
- **Seamless:** one tap to send, no accept prompts between paired devices.
- **Private:** device to device only. No cloud, no accounts, no telemetry.

> Status: early development. Phase 1 (core engine) is done: encrypted, verified, resumable single-file transfer with a command-line tool. Phase 2 (Android and Windows apps) is next.

## Layout
| Path | What it is |
|---|---|
| `core/` | `nudge-core`: Rust transfer engine shared by every platform |
| `cli/` | `nudge`: command-line tool for testing and benchmarks |
| `app/` | Flutter app (coming in Phase 2) |
| `docs/` | [Design](docs/DESIGN.md), [protocol](docs/PROTOCOL.md), [benchmarks](docs/BENCHMARKS.md) |

## Building
Requires Rust (stable, installed via [rustup](https://rustup.rs)). On Windows you also need the MSVC C++ build tools.

```sh
cargo build --release
cargo test
```

## Trying it
The `nudge` tool (at `target/release/nudge`) sends one file at a time, encrypted, between devices that trust each other.

```sh
nudge id                        # this device's fingerprint
nudge trust <fingerprint>       # trust another device (do it on both)
nudge receive --dir ~/Downloads # wait for files (add --bind 0.0.0.0 for other devices)
nudge send photo.jpg <host>     # send a file
nudge bench --disk              # measure speed and CPU use
```

Two processes on the same machine share one identity, so local testing needs no `trust`. An interrupted transfer resumes when the same file is sent again.

## License
Licensed under [MIT](LICENSE-MIT).
