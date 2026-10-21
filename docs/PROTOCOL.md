# Nudge Wire Protocol

Status: **v3 draft (Phase 1).** This file is the single source of truth for the wire format; change it in the same commit as any code that changes the format. Code: `core/src/protocol.rs`.

## Goals
- One connection per transfer, data streamed continuously (no per-chunk acks).
- Resume from an exact byte offset after interruption.
- BLAKE3 integrity per chunk, so corruption is caught on arrival and a partial file can be verified before resuming.

## Transport
TCP, default port 47800, wrapped in **TLS 1.3 only** (rustls with the aws-lc-rs provider). Code: `core/src/tls.rs`, `core/src/identity.rs`.

- **Identity:** each device has one self-signed certificate (ECDSA P-256), created on first run and stored as `cert.der` / `key.der` in its identity folder.
- **Fingerprint:** BLAKE3 of the certificate's DER bytes, shown as 64 hex characters.
- **Mutual authentication:** both sides present their certificate. Each side accepts the other only if the peer's fingerprint is in its `trusted_peers.txt` (or equals its own). No certificate authority; the TLS server name (`nudge.local`) is not checked.
- **Pairing:** adding each other's fingerprint to the trusted list. Done by hand with `nudge trust` for now; QR pairing comes in Phase 2.
- **Failures:** the side that rejects reports `UntrustedPeer(fingerprint)`; the rejected side receives a `certificate_unknown` alert and reports `NotTrustedByPeer`.

The messages below run inside the TLS stream.

## Messages (v3, single file)
All integers are big-endian.

**1. Offer** (sender → receiver)

| Field | Type | Notes |
|---|---|---|
| magic | 4 bytes | `NUDG` |
| version | u16 | `PROTOCOL_VERSION`; receiver rejects a mismatch |
| name_len | u16 | ≤ 1024 |
| name | UTF-8 | file name only; receiver sanitizes it |
| size | u64 | total file size in bytes |
| modified | u64 | sender's modification time, ns since the Unix epoch; 0 = unknown (disables resume) |

**2. Response** (receiver → sender), one tag byte:

| Tag | Meaning | Payload |
|---|---|---|
| 0 | Reject | none |
| 1 | Accept | u64 offset to start streaming from (0 = fresh); must be a multiple of `CHUNK_SIZE` |
| 2 | Done | none |

**3. Data** (sender → receiver): the file from `offset` to `size`, in chunks of `CHUNK_SIZE` = 4 MiB (4,194,304 bytes; the last may be shorter). Each chunk is its raw bytes followed by the 32-byte BLAKE3 hash of those bytes. Chunk boundaries are fixed multiples of `CHUNK_SIZE` from the start of the file. An empty file has no chunks.

The receiver hashes each chunk as it arrives. On a mismatch it closes the connection and reports `ChunkMismatch { chunk }` (`chunk` counts from the start of the file); the file is never renamed into place.

**4. Done** (receiver → sender): sent only after every byte is written, synced to disk, and the file renamed into place.

## Resume
Code: `core/src/resume.rs`. Resume is decided entirely by the receiver; the sender just starts from the offset in `Accept`.

**State on disk.** Next to each `<base>.nudgepart` the receiver keeps `<base>.nudgemeta`:

| Field | Type |
|---|---|
| magic | `NDGM` |
| version | u16 (1) |
| sender fingerprint | 32 bytes (zeros if unknown) |
| size | u64 |
| modified | u64 |
| name_len, name | u16 + UTF-8, the name exactly as offered |
| chunk hashes | 32 bytes each, for chunks 0, 1, 2, ... in order |

A chunk's hash is appended only after it passed its check **and** the data file was synced (`sync_data`) — every 64 chunks (256 MiB) and whenever the transfer stops for any reason. So the meta file never claims data a crash could lose: a dropped connection loses no progress; a crash or power cut loses at most 256 MiB. A torn hash at the end of the file (crash mid-append) is ignored and trimmed.

**Matching.** On an `Offer`, the receiver looks for a meta file with the same sender, name, size and modified time (non-zero), whose `.nudgepart` exists at full size. If found, it replies `Accept { offset = chunks_done × CHUNK_SIZE }`. Partials from the same sender and name with a different size or modified time are an older version and are deleted. Partials from other senders are left alone.

**Finishing.** After the last chunk, the data file is synced, renamed to its final name (re-checked for clashes at that moment), the meta file is deleted, and `Done` is sent.

**Limitations.** A file edited in place without its modified time changing would resume with stale early chunks. Abandoned partials are not cleaned up yet. Two simultaneous transfers of the same file from the same sender are not prevented.

## Version history
- **v1:** data was raw bytes with no hashes.
- **v2:** BLAKE3 hash after every 4 MiB chunk.
- **v3:** `Offer` carries the file's modified time; receivers resume interrupted transfers.

## Receiver file handling
- Writes to `<name>.nudgepart` (preallocated to `size`) plus `<name>.nudgemeta`; on completion the data file is renamed to `<name>` and the meta file deleted. A failed transfer leaves both for resume.
- Never overwrites: a clash becomes `<stem> (1).<ext>`, `(2)`, ...
- Sanitizes names: path separators, control characters and Windows-forbidden characters become `_`; trailing dots/spaces are trimmed; Windows device names (`CON`, `NUL`, ...) get a `_` prefix; `.`, `..` and empty names are rejected.

## Still to specify
- Cleanup of abandoned partial files
- Multi-file streams (Phase 4)
- Error codes
