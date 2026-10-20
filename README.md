# Nudge

Fast, seamless, private file transfer between phones and computers: an AirDrop-style experience outside the Apple ecosystem. First target: **Android ↔ Windows**.

- **Fast:** streams over the best available path and switches to a faster one mid-transfer.
- **Seamless:** one tap to send, no accept prompts between paired devices.
- **Private:** device to device only. No cloud, no accounts, no telemetry.

> Status: early development (Phase 0: project setup).

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
cargo build
cargo test
cargo run -p nudge-cli
```

## License
Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
