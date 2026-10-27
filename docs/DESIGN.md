# Nudge — Design Notes

Background and reasoning behind the decisions in `CLAUDE.md`.

## Why build this
The Apple ecosystem makes moving files and clipboard between devices effortless. Android + Windows users have options, but none feel as seamless or as fast:
- **KDE Connect** — broad feature set (clipboard, notifications, files, SMS), but the Windows port isn't officially released and file transfer isn't its focus.
- **LocalSend** — great open-source cross-platform file transfer (Flutter, MIT, HTTPS REST protocol). Real-world speeds reported anywhere from ~6 to ~40+ MB/s depending on device, encryption, and network; known Android speed issues; always goes via the router; accept prompt each time.
- **Phone Link / Quick Share** — free but closed source.

Nudge's niche: **the fastest possible transfer with zero decisions**, by choosing and switching paths automatically.

## Where transfer time actually goes (biggest first)
1. **The Wi-Fi path.** Via a router, every byte crosses the air twice (phone→router→laptop) and both hops share airtime on a half-duplex medium — roughly half the link's speed at best. A direct link is one hop at close range.
2. **Round trips.** Chunk/ack protocols waste the link. Stream continuously.
3. **Per-file overhead.** Hundreds of small files sent individually is slow. Pack into one stream.
4. **Disk/OS overhead.** Small writes and content-resolver calls on Android; non-preallocated files on Windows.
5. Encryption and compression are mostly *not* bottlenecks.

## Paths compared
| Path | Speed | Setup | Downsides |
|---|---|---|---|
| Router (shared Wi-Fi) | Slowest | Instant | Shares airtime; depends on router; blocked on networks with client isolation |
| Direct (phone hotspot / Wi-Fi Direct) | ~2× router | ~3 s | Laptop briefly loses its normal Wi-Fi; permissions; Windows' own Wi-Fi Direct stack is flaky, so the PC joins the phone's Wi-Fi Direct group as an ordinary Wi-Fi client (by name and passphrase, via `netsh`). Phones keep that group on 5 GHz, where a local-only hotspot often drops to 2.4 GHz; the hotspot is the fallback |
| USB-C (USB 3) | Fastest (hundreds of MB/s; usually storage-limited) | Plug in | Needs a USB 3 cable; not available on iPhone |

The **start-then-upgrade** strategy gets the best of all: instant start for small files, automatic move to the fastest path for large ones, no probe time wasted (the probe *is* the start of the transfer). Google Quick Share uses a similar upgrade-mid-transfer approach.

## Modeled estimates (targets, not benchmarks)
Model: setup + size ÷ sustained speed + per-file overhead.
Assumptions: AirDrop 40 MB/s, ~2 s setup · LocalSend 25 MB/s, ~3 s setup, ~0.1 s/file · Nudge router 45 MB/s, ~0.5 s setup · Nudge direct 80 MB/s, ~3 s setup.

| Scenario | LocalSend | AirDrop | Nudge (router) | Nudge (direct) |
|---|---|---|---|---|
| 1 photo (5 MB) | ~3 s | ~2 s | ~0.6 s | ~3 s |
| 100 photos (500 MB) | ~33 s | ~20 s | ~12 s | ~10 s |
| 4K video (1 GB) | ~43 s | ~27 s | ~23 s | ~16 s |
| 1,000 photos (5 GB) | ~5 min | ~3 min | ~2 min | ~1.2 min |
| Large video (20 GB) | ~13 min | ~8.4 min | ~7.4 min | ~4.2 min |

The 80 MB/s direct figure is optimistic (needs Wi-Fi 6, close range; phone storage write speed may cap it). Step 1's benchmark should replace these with real numbers.

## Apple platforms
- **macOS:** straightforward. Rust core compiles natively; macOS has no Wi-Fi Direct but can join the Android hotspot. Menu-bar app + Finder extension.
- **iOS:** possible (LocalSend proves it) with limits: no app-created hotspot (join via NEHotspotConfiguration instead), limited background execution (keep app open for big transfers), share-extension memory cap, Local Network permission, no USB file access, no background clipboard. iPhone↔Mac can use AWDL through Network.framework peer-to-peer for AirDrop-like direct speed.
- **Distribution:** Apple Developer account ($99/yr) for App Store and for Mac notarization. Use a permissive license.

## Future ideas
- Clipboard send (manual, one tap) and optional auto-sync where the OS allows.
- QUIC transport for transfers across different networks.
