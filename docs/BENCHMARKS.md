# Nudge Benchmarks

Measured numbers only. Every performance change should add a row here. These replace the modeled estimates in `DESIGN.md`.

## How to run
Phase 1 adds `nudge bench`. Record the command line used with every result.

## Results

| Date | Commit | Path | Devices | File set | MB/s | CPU % | Time to first byte | Notes |
|---|---|---|---|---|---|---|---|---|
| 2026-10-20 | uncommitted (Phase 1 step 1) | Loopback, plain TCP (no TLS) | Same Windows PC, release build | 1 × 2 GB random file | 446.7 | not measured | not measured | End-to-end incl. disk read, write and fsync. Baseline before TLS. |
| 2026-10-20 | uncommitted (Phase 1 step 2) | Loopback, plain TCP (`--no-tls`) | Same Windows PC, release build | 1 × 2 GB random file, 3 runs | 485 / 361 / 383 | not measured | not measured | Alternated with the TLS runs below. |
| 2026-10-20 | uncommitted (Phase 1 step 2) | Loopback, TLS 1.3 (aws-lc-rs, AES-256-GCM) | Same Windows PC, release build | 1 × 2 GB random file, 3 runs | 647 / 653 / 627 | not measured | not measured | TLS faster than plain: likely Windows loopback behaviour with 1 MB plain writes plus disk noise. End-to-end incl. disk, so not a pure network figure; `nudge bench` (step 5) will isolate it. |
| 2026-10-20 | uncommitted (Phase 1 step 3) | Loopback, TLS 1.3 + BLAKE3 per 4 MB chunk | Same Windows PC, release build | 1 × 2 GB random file, 5 runs | 413 / 447 / 483 / 500 / 615 | not measured | not measured | Wider spread than step 2 (627–653). Hashing alone measured at 6,140 MB/s single-thread on this CPU (0.33 s per 2 GB), so its cost is at most ~10% and likely below the disk noise here. Revisit with `nudge bench` (no disk) in step 5. |
| 2026-10-20 | uncommitted (Phase 1 step 4) | Loopback, TLS + BLAKE3 + resume checkpoints (sync every 256 MiB) | Same Windows PC, release build | 1 × 2 GB random file, 6 runs | 344 / 549 / 439 / 498 / 514 / 433 (mean 463) | not measured | not measured | A/B, alternating runs. |
| 2026-10-20 | uncommitted (Phase 1 step 4) | Same, checkpoints disabled (A/B control) | Same Windows PC, release build | 1 × 2 GB random file, 6 runs | 535 / 473 / 563 / 467 / 509 / 414 (mean 494) | not measured | not measured | ~6% gap, inside the ±100 MB/s run-to-run noise. Checkpoints kept. |
