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

## Many small files (`many_small_files`)

2026-10-23, uncommitted (Phase 4: packs), this Windows PC, two nodes over loopback TLS, 1,000 files × 10 KB, from send until every file is saved:
`cargo test --release -p nudge-core --lib many_small_files -- --ignored --nocapture`

| Version | Time | Per file |
|---|---|---|
| One transfer per file (before) | 8.44 s | 8.4 ms |
| Packs, each unpacked file synced to disk | 1.63 / 1.65 s | 1.6 ms |
| Packs, unpacked files not synced one by one (kept) | 0.71 / 0.74 s | 0.7 ms |

Over Wi-Fi the gap widens: each file sent on its own also waits several network round trips.
