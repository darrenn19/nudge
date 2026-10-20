# Nudge Wire Protocol

Status: **draft, written in Phase 1.** This file is the single source of truth for the wire format; change it in the same commit as any code that changes the format.

## Goals
- One TLS 1.3 connection per transfer, data streamed continuously (no per-chunk acks).
- Resume from an exact byte offset after interruption.
- BLAKE3 integrity per chunk, so a partial file can be verified before resuming.

## Sections to fill in
- Transport and TLS (mutual auth, certificate pinning by fingerprint)
- Handshake and version negotiation (`PROTOCOL_VERSION`)
- Transfer header (transfer ID, file metadata, start offset)
- Data stream and chunk hashes
- Resume flow
- Error codes
