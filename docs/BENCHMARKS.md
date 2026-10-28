# Nudge Benchmarks

Measured numbers only. Every performance change should add a row here. Phone ↔ PC numbers over Wi-Fi (Phase 2) will replace the modelled estimates in `DESIGN.md`; everything so far is one Windows PC talking to itself.

## How to run
Always use a release build. Record the command line with every result.

**Local (one machine, each layer separately):**
```sh
cargo run --release -p nudge-cli -- bench --size 2G --disk
```
Runs sender and receiver in one process over loopback and reports raw TCP, raw TLS, BLAKE3 alone, the full protocol memory-to-memory, and (with `--disk`) a real file transfer through the temp folder. CPU figures cover both ends together. `CPU s/GB` is the number to watch for phone battery.

**Remote (a real network path, no disks):**
```sh
# receiving device
nudge receive --discard --bind 0.0.0.0
# sending device (the two must trust each other: nudge id / nudge trust)
nudge bench --to <receiver-ip> --size 2G
```

## Layer breakdown (`nudge bench`)

2026-10-20, uncommitted (Phase 1 step 5), this Windows PC, `nudge bench --size 2G --disk`, two runs:

| Layer | MB/s (run 1 / 2) | CPU cores | CPU s/GB (both ends) |
|---|---|---|---|
| TCP, raw bytes | 1994 / 1914 | 0.84 / 0.75 | 0.42 / 0.39 |
| TLS 1.3, raw bytes | 1073 / 956 | 1.44 / 1.33 | 1.34 / 1.39 |
| BLAKE3 hashing alone (one side) | 5477 / 5455 | 0.98 / 1.02 | 0.18 / 0.19 |
| Nudge protocol over TLS, memory → memory | 553 / 565 | 1.40 / 1.53 | 2.53 / 2.70 |
| Nudge protocol over TLS, disk → disk | 437 / 445 | 1.81 / 1.69 | 4.14 / 3.80 |

Setup cost (1-byte file over a fresh TLS connection, median of 10): **3.9 / 4.0 ms**.

What this says:
- **Encryption** costs about 1 CPU-second per GB across both ends. On loopback that halves raw speed (both ends share one CPU), but 1 GB/s is still far above any Wi-Fi link.
- **Hashing** is cheap: about 0.19 CPU-seconds per GB per side.
- **The protocol path loses about half of raw-TLS speed** (560 vs ~1000 MB/s) and uses about 1 CPU-second per GB more than TLS plus hashing should. That's overhead in our own pipeline (buffer allocation and copies, thread hand-offs), not in TLS or BLAKE3. It doesn't matter on this PC, where the disk caps real transfers at ~440 MB/s, but it will matter on phone CPUs. **First optimisation target**, once phone numbers exist.
- **Setup is ~4 ms**, so small files are effectively instant on a fast link.
- Earlier loopback rows below include the disk, which explains their wide spread.

## Results

| Date | Commit | Path | Devices | File set | MB/s | CPU % | Time to first byte | Notes |
|---|---|---|---|---|---|---|---|---|
| 2026-10-20 | uncommitted (Phase 1 step 1) | Loopback, plain TCP (no TLS) | Same Windows PC, release build | 1 × 2 GB random file | 446.7 | not measured | not measured | End-to-end incl. disk read, write and fsync. Baseline before TLS. |
| 2026-10-20 | uncommitted (Phase 1 step 2) | Loopback, plain TCP (`--no-tls`) | Same Windows PC, release build | 1 × 2 GB random file, 3 runs | 485 / 361 / 383 | not measured | not measured | Alternated with the TLS runs below. |
| 2026-10-20 | uncommitted (Phase 1 step 2) | Loopback, TLS 1.3 (aws-lc-rs, AES-256-GCM) | Same Windows PC, release build | 1 × 2 GB random file, 3 runs | 647 / 653 / 627 | not measured | not measured | TLS faster than plain: likely Windows loopback behaviour with 1 MB plain writes plus disk noise. End-to-end incl. disk, so not a pure network figure; `nudge bench` (step 5) will isolate it. |
| 2026-10-20 | uncommitted (Phase 1 step 3) | Loopback, TLS 1.3 + BLAKE3 per 4 MB chunk | Same Windows PC, release build | 1 × 2 GB random file, 5 runs | 413 / 447 / 483 / 500 / 615 | not measured | not measured | Wider spread than step 2 (627–653). Hashing alone measured at 6,140 MB/s single-thread on this CPU (0.33 s per 2 GB), so its cost is at most ~10% and likely below the disk noise here. Revisit with `nudge bench` (no disk) in step 5. |
| 2026-10-20 | uncommitted (Phase 1 step 4) | Loopback, TLS + BLAKE3 + resume checkpoints (sync every 256 MiB) | Same Windows PC, release build | 1 × 2 GB random file, 6 runs | 344 / 549 / 439 / 498 / 514 / 433 (mean 463) | not measured | not measured | A/B, alternating runs. |
| 2026-10-20 | uncommitted (Phase 1 step 4) | Same, checkpoints disabled (A/B control) | Same Windows PC, release build | 1 × 2 GB random file, 6 runs | 535 / 473 / 563 / 467 / 509 / 414 (mean 494) | not measured | not measured | ~6% gap, inside the ±100 MB/s run-to-run noise. Checkpoints kept. |
| 2026-10-22 | uncommitted (buffer pool) | Loopback, protocol over TLS, memory → memory | Same Windows PC, release build | 2 GB generated, `nudge bench --size 2G` | before 417; after 742 / 569 | before 4.74 CPU s/GB; after 2.67 / 3.60 | not measured | Reused 1 MB buffers instead of allocating and zeroing one per buffer on both sides. Raw TLS on the same runs: 660–787 MB/s, so the protocol now runs close to TLS speed. |

## Start then upgrade: phone hotspot (step 5)

2026-10-24, uncommitted. Galaxy S24 (SM-S921W, Android 16) → Windows laptop (MediaTek MT7925 Wi-Fi 7, on the router's 6 GHz at 2.4 Gbps). 1 × 2 GB random file shared to Nudge on the phone; the phone decides. Router speed is the phone's link to the router (its signal was −74 dBm), which swung a lot between runs.

| Run | Router (start) | Decision | Hotspot band | Hotspot speed | Notes |
|---|---|---|---|---|---|
| 1 | 10.6 MB/s | Hotspot up in 0.6 s, PC joined in 4 s | 2.4 GHz | stalled | **Bug:** the send stayed on the dead router connection (no progress, no error); the PC gave up after ~70 s. Fixed: a send with nothing moving for 3 s moves to another path. |
| 2 | 31.5 MB/s | Moved after 3 s stalled at 186 MB | 2.4 GHz | never reconnected | **Bug:** closing the dead connection waited minutes (TLS goodbye into a full buffer). Fixed: 1 s cap. |
| 3 | 9.9 MB/s | Moved at 51 MB, reconnected in 1.3 s, resumed at the exact byte | 2.4 GHz (2437 MHz, 20 MHz) | 9.5 MB/s | Works end to end, but no faster: the phone hosts on 2.4 GHz. |
| 4 | 19.0 MB/s | Asked for 5 GHz (Android 16 `startLocalOnlyHotspotWithConfiguration`) | 5745 MHz, then 2412 MHz within 1 ms | 9.4 MB/s | Phone's own Wi-Fi on 5 GHz (5220 MHz): the hotspot is moved off 5 GHz. |
| 5 | 38.3 MB/s | Asked for 5 GHz with the phone's Wi-Fi on 6 GHz | 5745 → 2412 MHz again | 8.3 MB/s | Same: Samsung keeps the local-only hotspot on 2.4 GHz while connected to Wi-Fi. A 38 MB/s router path was given up on a guess ("2× the router"); the guess is now capped at 25 MB/s until a hotspot speed has been measured. Also a 4.5 min wait after the last byte, not yet explained (needs PC-side logs). |

**Takeaways.** Switching mid-transfer works: hotspot up in < 1 s, PC on it in ~4 s, resume at the exact byte. But on this phone the local-only hotspot only runs on 2.4 GHz while the phone stays on Wi-Fi, which caps it near 9–10 MB/s, no better than the router here. Once measured, Nudge stops switching with this phone.

**Changes after these runs (not yet measured on the phone):**
- The direct link is now a **Wi-Fi Direct group** owned by the phone, asked for on 5 GHz (on the phone's own channel when its Wi-Fi is on 5 GHz), with the local-only hotspot as fallback. The PC joins it with `netsh` like any Wi-Fi network. The group's real frequency goes into the log.
- Direct-link speeds are stored under a new name (`direct`), so the 2.4 GHz hotspot measurements above don't stop the phone from trying the new link.
- When both speeds are known from earlier transfers and the direct link wins, it starts at the beginning of the send instead of after a few seconds of measuring.
- **Likely cause of the 4.5 min wait in run 5:** the PC's old router connection still looked alive (TCP doesn't notice a vanished network while it's only reading), so it kept its hold on the file and the resumed one ran as a second transfer alongside it. Now a new connection for the same file stops the old one, waits for it to save, and carries on as the same transfer (test: `new_connection_takes_over_a_silent_one`). To confirm on the next run.
- Both apps now write `nudge.log` in their data folder (Windows: `%APPDATA%\Nudge\nudge.log`), so runs can be read from both sides.

To measure next: the run-5 setup again (2 GB, phone on 6 GHz, then on 5 GHz), reading the direct link's frequency from the log.

## Many small files (`many_small_files`)

2026-10-23, uncommitted (Phase 4: packs), this Windows PC, two nodes over loopback TLS, 1,000 files × 10 KB, from send until every file is saved:
`cargo test --release -p nudge-core --lib many_small_files -- --ignored --nocapture`

| Version | Time | Per file |
|---|---|---|
| One transfer per file (before) | 8.44 s | 8.4 ms |
| Packs, each unpacked file synced to disk | 1.63 / 1.65 s | 1.6 ms |
| Packs, unpacked files not synced one by one (kept) | 0.71 / 0.74 s | 0.7 ms |

Over Wi-Fi the gap widens: each file sent on its own also waits several network round trips.

## Screen mirroring latency spike (mirroring step 1)

2026-10-27, uncommitted, Galaxy S24 (SM-S921W, Snapdragon) → this Windows PC, `spikes/mirror-latency`.
Phone: MediaProjection → hardware H.264 (Baseline, low-latency mode, 10 Mbit/s target, 880×1904, 60 fps max) → plain UDP.
PC: software decode (openh264), on the same thread that draws the window. Content: the spike's clock screen, then Instagram video and scrolling.
Clocks synced by ping, so these are per-frame measurements. Not included: about one frame for each display (+10–25 ms).

Medians, with the worst 5% (p95) after the slash, in ms:

| Path | Phone encode | Network | PC queue + decode | PC draw | **Total** | fps shown | Frames lost/skipped |
|---|---|---|---|---|---|---|---|
| USB cable (tethering) | 22.9 / 35.6 | 2.7 / 8.0 | 12.4 / 40.0 | 3.7 / 7.2 | **44.4 / 77.1** | 37 | 2.8% |
| Wi-Fi, run 1 | 25.6 / 38.5 | 6.8 / 11.6 | 14.9 / 46.4 | 3.8 / 6.9 | **53.2 / 88.8** | 36 | 2.4% |
| Wi-Fi, run 2 (20 Mbit/s requested; averaged 9.6) | 24.8 / 36.6 | 7.4 / 14.8 | 15.6 / 59.8 | 3.8 / 7.2 | **53.7 / 103.2** | 32 | 4.3% |

What this says:
- **Go.** Wi-Fi lands at ~53 ms median, ~65–75 ms counting the displays: inside the 40–80 ms target. Cable is ~10 ms faster, mostly network.
- **The phone encoder is the biggest cost (23–26 ms).** Android's standard low-latency flag may not be enough on Snapdragon; try Qualcomm's own low-latency setting next.
- **The bad tails come from the PC side** (decode p95 40–60 ms, p99 up to 127 ms): software decoding on the UI thread. The real app decodes in hardware, off the UI thread.
- **2–4% of frames are lost or skipped**, even over the cable. Each loss freezes the picture until the next keyframe. The real channel needs packet pacing plus loss recovery (intra-refresh or FEC) instead of waiting for a keyframe.
- The picture looked blotchy at 10 Mbit/s. Doubling the requested bitrate didn't raise the actual bitrate, so the encoder (or its low-latency mode) is capping it. Look into this during step 4.
- Not yet measured: phone hotspot.

### After the quality pass (same day)

Phone: H.264 High profile, constant bitrate 30 Mbit/s requested (17.6 averaged), 1056×2304. PC: hardware decode (Media Foundation) and D3D11 video processor into a flip-model swap chain, window DPI-aware.

| Path | Phone encode | Network | PC queue + decode | PC draw | **Total** | fps shown | Frames lost/skipped |
|---|---|---|---|---|---|---|---|
| USB cable | 22.9 / 53.3 | 3.7 / 9.6 | **1.0** / 10.7 | **0.9** / 16.6 | **34.7 / 68.3** | 43 | 9.2% |

- PC decode + draw went from ~16 ms to ~2 ms (median). Total median down 10 ms, to 35 ms.
- New problem: **47 keyframes in 60 s**. Each lost packet triggers a keyframe request; at 30 Mbit/s a keyframe is a big burst (30–58 ms to encode, 10–40 ms to send) that causes more loss. That's the p95 and the stutter. Fix: resend just the missing packets (a 2 ms round trip on this link) and pace sending, instead of asking for keyframes.
- PC draw p95 of 16.6 ms: presenting sometimes waits a whole display refresh. Fix: never block on present.

### Loss recovery, self-test (2026-10-28)

No phone: `--selftest` (fake phone on the same PC, software H.264, 720×1280, under 1 Mbit/s) with `--drop` throwing away packets on arrival. Shows the mechanism works, not phone numbers.

| Drop rate (packets) | Fragments recovered | Recovered by resend | Given up | Keyframes requested | Recovery time p50 / p95 |
|---|---|---|---|---|---|
| 1% (14) | 23 | 23 frames | 0 | 0 | 8.3 / 10.2 ms |
| 5% (72) | 80 | 79 frames | 0 | 0 | 9.4 / 16.1 ms |

More fragments were recovered than dropped on purpose: probably the fake phone losing a few on its own (it ignores failed non-blocking sends). Before this change, each of those losses would have frozen the picture until a keyframe arrived. Recovery time here is mostly the fake phone answering only once per frame (every 16.7 ms); the real phone answers NACKs on its own thread, so it should come close to one round trip (2–7 ms).

### Wi-Fi tuning on the phone (2026-10-28, evening)

Galaxy S24 → this PC (MediaTek MT7925 Wi-Fi 7), `spikes/mirror-latency`, H.264 High 30 Mbit/s CBR, 1056×2304, 45 s per run unless noted, the spike's clock screen plus whatever was on the phone. Home Wi-Fi is a mesh: phone and PC on different access points (5 GHz ch 44 and ch 101). "Direct" = the phone hosts a Wi-Fi Direct group (Nudge's `Hotspot.kt`, 5 GHz ch 44) and the PC joins it. Medians / p95 / p99 / max, ms.

| Run | Encode | Network | **Total** | Frames lost/skipped | Freezes |
|---|---|---|---|---|---|
| Cable, defaults | 20.3 / 24.5 / 27.4 / 48.6 | 2.4 / 3.5 / 4.5 / 20.3 | **23.7 / 28.4 / 35.2** / 234 (first frame; else 61, keyframes) | 0 | 0 |
| Router Wi-Fi, defaults | 20.4 / 30.2 / 62.7 / 321 | 6.7 / 17.0 / 51.5 / 290 | **29.5 / 51.3 / 116** / 340 | 27 | 2, 393 ms |
| Router, + intra-refresh | 20.5 / 40.6 / 78.2 / 158 | 6.7 / 18.3 / 48.3 / 145 | **28.8 / 60.1 / 108** / 257 | 149 | 6, 364 ms |
| Router, + encoder speed-ups | **10.8 / 13.9 / 15.5 / 21.3** | 6.9 / 18.4 / 62.6 / 173 | **20.7 / 31.5 / 78.6** / 188 | 135 | 3, 1741 ms |
| Router, + link-time recovery | 10.6 / 16.5 / 27.1 / 50.7 | 6.9 / 62.9 / 83.3 / 131 | 21.5 / 76.0 / 95.8 / 274 | 88 | 3, 139 ms |
| Router, + 1 s resend history | 10.5 / 13.3 / 16.2 / 79.5 | 9.4 / 58.2 / 88.8 / 241 | 21.6 / 72.0 / 104 / 303 | **0** | **0** |
| **Direct link**, all of the above | 10.8 / 16.1 / 23.8 / 97.6 | 5.5 / 10.1 / 18.9 / 53.7 | **17.4 / 28.7 / 37.4** / 260 (first frame) | 0 | 0 |
| **Direct link**, 90 s | 10.7 / 19.7 / 33.9 / 139 | 5.1 / 8.8 / 14.4 / 45.7 | **18.7 / 32.0 / 48.1** / 224 (first frame) | 0 | 0 |

Router runs vary a lot from one to the next (network p95 17–66 ms with the same settings), so compare columns within a run, not small differences between runs.

What this says:
- **Encoder speed-ups halve encoding:** Qualcomm's `vendor.qti-ext-enc-low-latency.enable` with `KEY_OPERATING_RATE` max and `KEY_LATENCY` 1 take encode from ~20.5 to ~10.7 ms median, and its worst case from 158 to 21 ms on the same link. On by default now.
- **Through the router, Wi-Fi pauses for 50–300 ms**, then delivers everything at once. Plain ping shows it too (median 13 ms, p90 113, max 206). It's the router path: on the direct link the network max is 46–54 ms.
- **Those pauses were turning late frames into skipped ones:** the PC gave up after 50 ms and used up its resend requests during the pause, and the phone had already dropped the packets it would have resent (10 frames of history). Fixed by timing recovery in *link time* (pauses count as at most 20 ms), 5 requests instead of 3, a 300 ms wall-clock backstop, and 1 s of history on the sender: **0 lost frames and 0 freezes even through the router.** Ported to the core (`core/src/realtime/frames.rs`).
- **The direct link is the fix for Wi-Fi latency:** p99 37–48 ms vs ~100 through the router. Mirroring over Wi-Fi should bring it up the way big transfers do.
- Tried and no effect: the PC sending a tiny packet every 5 ms (to stop its Wi-Fi from dozing). Wi-Fi "media streaming mode" (`WlanSetInterface`) is on in the spike but not shown to help here.
- What's left: the first frame (window, decoder and encoder starting: 220–260 ms, once per session), occasional phone encoder hiccups (a few frames up to 70–140 ms), and the PC waiting for the display's next refresh (draw p95 ~12 ms).

## Real-time channel, one PC (mirroring step 2)

2026-10-28, `nudge mirror-bench` (both ends in one process, real TLS + UDP over this PC's Wi-Fi address), 30 Mbit/s, 60 fps, 6 s, keyframe (~500 KB) every 120 frames, `--drop` throwing away fragments on arrival. Latency is send-to-arrival (pacing + network + recovery), no encoder or display.

| Fragments dropped | Frames received | Recovered by resend | Given up | Keyframe requests | Latency p50 / p95 / p99 |
|---|---|---|---|---|---|
| 0% | 360 / 360 | 0 | 0 | 0 | 1.3 / 3.0 / 16.6 ms |
| 1% | 360 / 360 | 147 | 0 | 0 | 1.1 / 3.1 / 16.4 ms |
| 3% | 357 / 360 | 271 | 1 | 2 | 1.6 / 16.0 / 18.2 ms |
| 5% | 360 / 360 | 334 | 0 | 0 | 1.5 / 17.5 / 21.5 ms |

The ~16 ms tail is Windows' default timer tick (the sender's pacer and the receiver's checks wake on it); expect it to shrink with `timeBeginPeriod(1)` and on Android.
Fixes found along the way: the receiver NACKed fragments still on their way during pauses between paced bursts (stall now measured from the latest fragment, 30 ms); NACK tries were per frame, so late gaps in a big keyframe were never asked for (now per fragment); resends waited behind the pacer (now sent at once).

### 1 ms timers and faster tail-loss detection (2026-10-28, later)

Same bench. The bench now asks Windows for 1 ms timers, as the Windows app does while a mirroring session runs (`NUDGE_COARSE_TIMER=1` for the old behaviour). The receiver also notices a frame whose *last* fragments were lost sooner: it waits twice the longest recent pause between one frame's fragments (+2 ms, between 4 and 30 ms) instead of a fixed 30 ms.

| Fragments dropped | Before (coarse timers) p50 / p95 / p99 | 1 ms timers | + adaptive tail wait |
|---|---|---|---|
| 0% | 0.9 / 1.6 / 4.3 | 0.8 / 1.9 / 6.2 | 1.1 / 1.8 / 12.1 |
| 1% | – | 0.8 / 1.3 / 19.0 | 1.0 / 1.8 / 10.1 |
| 3% | 1.0 / 16.5 / 17.7 | 1.0 / 6.4 / 18.6 | 0.9 / **4.5 / 10.6** |
| 5% | – | 1.0 / 9.2 / 19.5 | 0.9 / **7.6 / 15.0** |

Every frame arrived in every run, none given up, no keyframe requests. The ~20 ms max that stays even at 0% is a 500 KB keyframe being paced out at 200 Mbit/s: intra-refresh (no big keyframes) is the fix for that, not the channel. Run-to-run noise at 0% is a few ms at p99.

### Spike runs with NACK recovery and pacing (2026-10-28, not written up at the time)

Re-read from the CSVs in `spikes/mirror-latency` (settings not recorded; "wifi-lock" runs presumably tested Wi-Fi low-latency locks). Medians / p95 / p99, ms:

| Run | Frames | Encode | Network | Draw | **Total** | Frames > 80 ms |
|---|---|---|---|---|---|---|
| `run-1793197936` | 7577 | 25.0 / 36.2 / 49.6 | 3.1 / 8.9 / 15.1 | 0.7 / 10.9 / 15.2 | **31.3 / 46.3 / 61.7** | 0.2% (all encode) |
| `wifi-lock-1793217502` | 7167 | 22.5 / 39.0 / 53.7 | 7.2 / 17.4 / 32.2 | 0.9 / 13.7 / 16.2 | **35.9 / 54.5 / 83.7** | 1.1% (49 encode, 29 network) |
| `wifi-lock-1793197762` | 1908 | 25.7 / 37.5 / 47.3 | 18.7 / 111.7 / 188.8 | 0.9 / 13.9 / 16.5 | **52.1 / 145.9 / 220.7** | 20.4% (network) |
| `run-1793202049` | 368 | 28.2 / 44.6 / 53.2 | 29.4 / 110.5 / 167.2 | 0.8 / 7.0 / 11.8 | **62.8 / 143.1 / 195.1** | 34.2% (network) |

What this says:
- Good runs are now ~31–36 ms median, ~46–55 ms p95: decode is ~1 ms, so **the phone encoder (~23–25 ms) is most of the latency**, and its stalls (max 200–300 ms) cause most remaining spikes on good links.
- Some Wi-Fi runs are much worse (network p95 >100 ms): Wi-Fi power saving or a busy channel. Which settings were on for each run wasn't recorded; record them next time.
- Draw p95 ~14 ms is waiting for the display's next refresh (one slot, newest frame wins). Only tearing (`DXGI_PRESENT_ALLOW_TEARING`) avoids it, and these numbers stop at `Present`, so they can't show whether that helps on screen.

