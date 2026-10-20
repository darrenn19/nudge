# Nudge Wire Protocol

Status: **v1 draft (Phase 1).** This file is the single source of truth for the wire format; change it in the same commit as any code that changes the format. Code: `core/src/protocol.rs`.

## Goals
- One connection per transfer, data streamed continuously (no per-chunk acks).
- Resume from an exact byte offset after interruption.
- BLAKE3 integrity per chunk, so a partial file can be verified before resuming (planned).

## Transport
TCP, default port 47800, wrapped in **TLS 1.3 only** (rustls with the aws-lc-rs provider). Code: `core/src/tls.rs`, `core/src/identity.rs`.

- **Identity:** each device has one self-signed certificate (ECDSA P-256), created on first run and stored as `cert.der` / `key.der` in its identity folder.
- **Fingerprint:** BLAKE3 of the certificate's DER bytes, shown as 64 hex characters.
- **Mutual authentication:** both sides present their certificate. Each side accepts the other only if the peer's fingerprint is in its `trusted_peers.txt` (or equals its own). No certificate authority; the TLS server name (`nudge.local`) is not checked.
- **Pairing:** adding each other's fingerprint to the trusted list. Done by hand with `nudge trust` for now; QR pairing comes in Phase 2.
- **Failures:** the side that rejects reports `UntrustedPeer(fingerprint)`; the rejected side receives a `certificate_unknown` alert and reports `NotTrustedByPeer`.

The messages below run inside the TLS stream.

## Messages (v1, single file)
All integers are big-endian.

**1. Offer** (sender → receiver)

| Field | Type | Notes |
|---|---|---|
| magic | 4 bytes | `NUDG` |
| version | u16 | `PROTOCOL_VERSION`; receiver rejects a mismatch |
| name_len | u16 | ≤ 1024 |
| name | UTF-8 | file name only; receiver sanitizes it |
| size | u64 | total file size in bytes |

**2. Response** (receiver → sender), one tag byte:

| Tag | Meaning | Payload |
|---|---|---|
| 0 | Reject | none |
| 1 | Accept | u64 offset to start streaming from (0 = fresh) |
| 2 | Done | none |

**3. Data** (sender → receiver): raw bytes from `offset` to `size`, no framing.

**4. Done** (receiver → sender): sent only after every byte is written, synced to disk, and the file renamed into place.

## Receiver file handling
- Writes to `<name>.nudgepart`, preallocated to `size`; renamed to `<name>` on completion. A failed transfer leaves the `.nudgepart` file for resume.
- Never overwrites: a clash becomes `<stem> (1).<ext>`, `(2)`, ...
- Sanitizes names: path separators, control characters and Windows-forbidden characters become `_`; trailing dots/spaces are trimmed; Windows device names (`CON`, `NUL`, ...) get a `_` prefix; `.`, `..` and empty names are rejected.

## Still to specify
- Chunk hashes and the resume flow
- Multi-file streams (Phase 4)
- Error codes
