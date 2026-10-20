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
