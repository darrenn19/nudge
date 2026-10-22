# Nudge Wire Protocol

Status: **v4 draft (Phase 2).** This file is the single source of truth for the wire format; change it in the same commit as any code that changes the format. Code: `core/src/protocol.rs`. Discovery and pairing are described below too (`core/src/discovery.rs`, `core/src/pairing.rs`).

## Goals
- One connection per transfer, data streamed continuously (no per-chunk acks).
- Resume from an exact byte offset after interruption.
- BLAKE3 integrity per chunk, so corruption is caught on arrival and a partial file can be verified before resuming.

## Transport
TCP, default port 47800, wrapped in **TLS 1.3 only** (rustls with the aws-lc-rs provider). Code: `core/src/tls.rs`, `core/src/identity.rs`.

- **Identity:** each device has one self-signed certificate (ECDSA P-256), created on first run and stored as `cert.der` / `key.der` in its identity folder.
- **Fingerprint:** BLAKE3 of the certificate's DER bytes, shown as 64 hex characters.
- **Mutual authentication:** both sides present their certificate. Each side accepts the other only if the peer's fingerprint is in its `trusted_peers.txt` (or equals its own). No certificate authority; the TLS server name (`nudge.local`) is not checked.
- **Pairing window:** while a device shows a pairing code, its listening side also completes handshakes with unknown clients. Such a connection may only send a `PairRequest`; anything else is rejected. Outgoing connections are always strict.
- **Trusted list:** `trusted_peers.txt`, one device per line: fingerprint, a space, then the name it gave when pairing. Lines starting with `#` are comments. Filled by QR pairing (below) or by hand with `nudge trust`.
- **Failures:** the side that rejects reports `UntrustedPeer(fingerprint)`; the rejected side receives a `certificate_unknown` alert and reports `NotTrustedByPeer`.

- **Waiting for the request:** after the handshake, the listening side waits up to 15 s for the header below, or up to 60 s if the peer is trusted. The longer wait lets a paired device open a connection ahead of time (when the share sheet opens, or while the user picks a device) and send its offer when the user taps Send. The sender uses such a connection for at most 45 s. If it turns out to be closed, the sender retries once on a fresh connection, and resume makes that safe. While sending several files, the next file's connection opens while the current one streams. None of this changes the wire format.

The messages below run inside the TLS stream.

## Messages (v4)
All integers are big-endian. Every connection starts with a header:

| Field | Type | Notes |
|---|---|---|
| magic | 4 bytes | `NUDG` |
| version | u16 | `PROTOCOL_VERSION`; the receiving side rejects a mismatch |
| kind | u8 | 1 = file `Offer`, 2 = `PairRequest` |

### File transfer (kind 1, single file)

**1. Offer** (sender → receiver), after the header. Only accepted from a trusted device.

| Field | Type | Notes |
|---|---|---|
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

### Pairing (kind 2)
Code: `core/src/pairing.rs`, `core/src/node.rs`.

**The code.** The device showing a code (the *owner*) makes a fresh 16-byte random secret and shows a QR code containing this text:

```
nudge:pair?v=1&fp=<owner fingerprint, 64 hex>&s=<secret, 32 hex>&n=<owner name>&a=<IPv4 list, comma-separated>&p=<TCP port>
```

Values are percent-encoded. Readers ignore unknown fields. A code works **once** and expires after **5 minutes**; showing a new code invalidates the old one.

**The exchange.** The scanning device connects to one of the code's addresses (or, failing those, an address found by discovery for that fingerprint) and trusts exactly the code's fingerprint for this connection. Then:

1. Scanner → **PairRequest**, after the header: `proof` (32 bytes), then `name_len` (u16, ≤ 255) and the scanner's device name (UTF-8).
   `proof = BLAKE3-keyed(key = BLAKE3-derive-key("nudge pairing proof v1", secret), scanner fingerprint ‖ owner fingerprint)`.
2. Owner checks the proof in constant time against its current, unexpired code, using the scanner's fingerprint from the TLS handshake. Binding both fingerprints means a proof can't be replayed by or for another device.
3. Owner → **PairReply**: tag 0 = Reject; tag 1 = Accept, followed by `name_len` (u16) and the owner's device name. On accept the code is used up.
4. Both sides add the other to their trusted list with its name. From then on transfers between them need no prompt.

## Discovery
Code: `core/src/discovery.rs`. Discovery only says who is nearby and where; it proves nothing. Fingerprints are checked by TLS when connecting. Two methods run together and are merged per fingerprint:

**mDNS / DNS-SD.** Service type `_nudge._tcp.local.`, instance `nudge-<first 16 hex of fingerprint>`, port = the device's TCP port. TXT record: `fp` (64 hex), `n` (device name, ≤ 200 bytes), `pl` (platform: `windows`, `android`, ...), `v` (protocol version). Unregistered (goodbye) on shutdown.

**UDP beacons**, for networks that drop multicast. Port 47801. Sent to every interface's IPv4 broadcast address and 255.255.255.255:

| Field | Type | Notes |
|---|---|---|
| magic | 4 bytes | `NDGD` |
| version | u8 | 1 |
| kind | u8 | 1 = announce, 2 = query (please announce), 3 = goodbye |
| port | u16 | TCP port |
| fingerprint | 32 bytes | |
| name | u8 length + UTF-8 | ≤ 200 bytes |
| platform | u8 length + UTF-8 | ≤ 32 bytes |

A device sends a query on start, an announce every 5 s, and a goodbye on shutdown. It answers a query with a unicast announce to the sender. A device seen only by beacon is dropped after 16 s of silence. If port 47801 is taken (another app, or a second Nudge on the same machine), the device uses a random port: it can still query and hear unicast replies.

Addresses are tried best first: private IPv4, other IPv4, then IPv6 (link-local IPv6 is skipped).

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
- **v4:** connections start with a header carrying a request kind; adds QR pairing (`PairRequest` / `PairReply`).

## Receiver file handling
- Writes to `<name>.nudgepart` (preallocated to `size`) plus `<name>.nudgemeta`; on completion the data file is renamed to `<name>` and the meta file deleted. A failed transfer leaves both for resume.
- Never overwrites: a clash becomes `<stem> (1).<ext>`, `(2)`, ...
- Sanitizes names: path separators, control characters and Windows-forbidden characters become `_`; trailing dots/spaces are trimmed; Windows device names (`CON`, `NUL`, ...) get a `_` prefix; `.`, `..` and empty names are rejected.

## Still to specify
- Cleanup of abandoned partial files
- Multi-file streams (Phase 4)
- Error codes
