# Nudge Wire Protocol

Status: **v5 (`PROTOCOL_VERSION` = 5).** This file is the single source of truth for the wire format; change it in the same commit as any code that changes the format. Code: `core/src/protocol.rs`. Discovery and pairing are described below too (`core/src/discovery.rs`, `core/src/pairing.rs`).

## Goals
- One connection per transfer, data streamed continuously (no per-chunk acks).
- Resume from an exact byte offset after interruption.
- BLAKE3 integrity per chunk, so corruption is caught on arrival and a partial file can be verified before resuming.

## Transport
TCP, default port 47800, wrapped in **TLS 1.3 only** (rustls with the aws-lc-rs provider). Code: `core/src/tls.rs`, `core/src/identity.rs`.

- **Identity:** each device has one self-signed certificate (ECDSA P-256), created on first run and stored as `cert.der` / `key.der` in its identity folder.
- **Fingerprint:** BLAKE3 of the certificate's DER bytes, shown as 64 hex characters.
- **Mutual authentication:** both sides present their certificate. Each side accepts the other only if the peer's fingerprint is in its `trusted_peers.txt` (or equals its own). No certificate authority; the TLS server name (`nudge.local`) is not checked.
- **Unknown devices:** the listening side completes handshakes with any client, so nearby devices can pair without a QR code. A connection from an unpaired device may only send a `PairRequest` (with a valid code) or a `PairAsk`; a file offer gets Not paired, anything else is dropped. Outgoing connections are always strict: the client trusts exactly the fingerprint it means to reach.
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
| kind | u8 | 1 = file `Offer`, 2 = `PairRequest`, 3 = `PairAsk`, 4 = `Status` |

### File transfer (kind 1, single file)

**1. Offer** (sender → receiver), after the header. Only accepted from a trusted device.

| Field | Type | Notes |
|---|---|---|
| name_len | u16 | ≤ 1024 |
| name | UTF-8 | file name only; receiver sanitizes it |
| size | u64 | total file size in bytes |
| modified | u64 | sender's modification time, ns since the Unix epoch; 0 = unknown (disables resume) |
| batch_id | u64 | random per send, the same for every file in it; 0 = a file sent on its own |
| batch_count | u32 | files in the send |
| batch_size | u64 | bytes in the whole send |

**Asking first.** Unless the sender is set to auto-accept, the receiver asks its user before answering: once per batch (`NodeEvent::OfferReceived`, with the first file's name, `batch_count` and `batch_size`), and later files of that batch follow the answer for 30 minutes. Meanwhile it replies Asking (tag 6) at once, so the sender can show the send as delivered and pending; the real answer follows on the same connection. With no answer in 5 minutes the receiver replies No answer. The sender stops offering the rest of a batch after Declined or No answer. Devices set to auto-accept (`auto_accept.txt` in the identity folder, one fingerprint per line) are answered Accept at once, as are all paired devices when asking is off (the command-line tool).

**2. Response** (receiver → sender), one tag byte:

| Tag | Meaning | Payload |
|---|---|---|
| 0 | Reject | none |
| 1 | Accept | u64 offset to start streaming from (0 = fresh); must be a multiple of `CHUNK_SIZE` |
| 2 | Done | none |
| 3 | Declined (the user said no) | none |
| 4 | No answer (nobody answered in time) | none |
| 5 | Not paired (the receiver doesn't trust the sender) | none |
| 6 | Asking (not an answer: the user is being asked; another response follows) | none |
| 7 | Held (all there, kept until the user answers) | none |
| 8 | Approved (the user said yes; the reply to `AwaitAnswer`) | none |
| 9 | Cancelled (the receiver's user stopped the send) | none |

**3. Data** (sender → receiver): the file from `offset` to `size`, in chunks of `CHUNK_SIZE` = 4 MiB (4,194,304 bytes; the last may be shorter). Each chunk is its raw bytes followed by the 32-byte BLAKE3 hash of those bytes. Chunk boundaries are fixed multiples of `CHUNK_SIZE` from the start of the file. An empty file has no chunks.

The receiver hashes each chunk as it arrives. On a mismatch it closes the connection and reports `ChunkMismatch { chunk }` (`chunk` counts from the start of the file); the file is never renamed into place.

**4. Done** (receiver → sender): sent only after every byte is written, synced to disk, and the file renamed into place.

### Cancelling (kind 10)
Either user can stop a send while it moves; stopping one file stops the rest of its batch.

- **The sender's user:** the sender drops the data connection, then opens a new one and sends **Cancel**: `batch_id` (u64), `name_len` (u16, ≤ 1024), `name` and `size` (u64) of the file it was on. No reply. The receiver stops that file and any other of the batch arriving, deletes their partial files (they won't resume), closes any open question about the batch, and ends files of it held for an answer.
- **The receiver's user:** the receiver deletes what arrived, replies Cancelled (tag 9) where it can, and closes the connection. For 10 minutes it answers any offer from that batch (or of that file, for batch 0) with Cancelled, so a sender that only saw the connection drop learns why when it reconnects; `AwaitAnswer` for the batch is answered Cancelled too. The sender ends the rest of the batch as cancelled there.

### Folders and packs (kind 8)
A file from a folder is offered under its path in that folder, `/`-separated (`Trip/day 2/plan.txt`). The receiver sanitizes each part like a file name, refuses empty, `.` and `..` parts, and puts the file in those folders under its download folder (never overwriting: a clash gets a " (1)" suffix).

Many small files (4 or more, each ≤ 8 MiB) go as **packs**: a kind-8 header with exactly the body of a file offer (kind 1), whose name is only for showing ("Trip", "37 files") and whose modified time is a hash of the pack's file list (so a resume only matches the same files). The data, streamed, hashed and resumed exactly like one file's, is:

`"NDGP"`, u16 version (1), u32 count, then per file: u16 path length, path (UTF-8, as above), u64 size, u64 modified; then every file's bytes in the same order. A path ending in `/` is an empty folder (size 0): only a pack carries those, so a send with one always uses a pack.

The receiver lands it as one partial file and, once kept, checks the list adds up to the size and unpacks it. A pack holds at most 4,000 files and about 256 MiB; larger files go on their own. Code: `core/src/pack.rs`.

### Pairing without a code (kind 3)
A device that sees another nearby (by discovery) can ask to pair. The asker connects trusting exactly the fingerprint discovery reported, and sends **PairAsk**: `name_len` (u16, ≤ 255) and its device name. The other side asks its user, showing a six-digit code; the asker shows the same code. The code is `BLAKE3-derive-key("nudge pairing compare code v1", lower fingerprint ‖ higher fingerprint)`, first four bytes as a big-endian u32 mod 1,000,000, so both sides compute it from the fingerprints the handshake proved: a device in the middle would make the codes differ. The answer is a **PairReply** (as for kind 2), after up to 90 s. On Accept both sides trust each other. One question per device at a time.

### Settings between paired devices (kind 4)
**Status**: one byte of flags. Bit 0 `auto_accepts_you`: files from the receiving device are accepted without asking. Bit 1 `cable_unused`: this phone is plugged into a computer by a cable that carries data, but USB tethering is off, so files can't use it (computers then say what to switch on). Older builds only ever send 0 or 1. The reply is the other side's own Status byte. Sent when a paired device first shows up in discovery, when either setting changes, and inferred from transfers (an Asking reply means "asks").

**Cables.** Files go over a cable (any kind that carries data: USB-C, micro-USB, USB-A, Lightning) once the phone shares a network on it (USB tethering). It's just another network: an address on it is ranked like any other, by measured speed. A computer recognises a phone's cable by the adapter's driver ("Remote NDIS…", "UsbNcm Host Device", "Apple Mobile Device Ethernet"), since Windows names it only "Ethernet N"; a phone by its interface name (`rndis0`, `ncm0`, `usb0`). A device is "connected by cable" when one of its addresses is on such a network.

### Clipboard (kinds 6 and 9)
Paired devices only, applied without a question. **Clip** (kind 6): `len` (u32, ≤ 1 MiB) and UTF-8 text. **ClipImage** (kind 9): `mime_len` (u16, ≤ 64) and the image type (must start with `image/`: `image/png`, `image/jpeg`...), then `len` (u32, ≤ 48 MiB) and the image bytes. The reply is `Done`, or `Reject` if the receiver doesn't sync its clipboard with the sender. A received image is saved as `<data dir>/clipboard/clip-<ms>.<ext>` (only the newest is kept) and put on the clipboard from there. Windows sends copied images as PNG; Android sends what was copied, as is.

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

**Without waiting for discovery.** Discovery can lag: after a network comes back (Windows rejoins Wi-Fi a few seconds after a phone's cable comes out) or for a phone whose screen is off. So:
- Where each paired device was last seen (and last connected from) is kept in `last_seen.txt` beside the identity, and survives restarts.
- On start, and whenever this device's networks change (checked every second), each paired device discovery isn't seeing is sent a `Status` at those addresses; on a change, a beacon query goes out on every network there is now too.
- A paired device that answers a `Status`, or connects, counts as nearby for 90 s even if discovery hasn't seen it.

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
- **v7:** cancelling: `Cancel` (kind 10) and the Cancelled response (tag 9). Later, without a version bump: screen mirroring (`Mirror`, kind 11), which older v7 builds refuse.

## Receiver file handling
- Writes to `<name>.nudgepart` (preallocated to `size`) plus `<name>.nudgemeta`; on completion the data file is renamed to `<name>` and the meta file deleted. A failed transfer leaves both for resume.
- Never overwrites: a clash becomes `<stem> (1).<ext>`, `(2)`, ...
- Sanitizes names: path separators, control characters and Windows-forbidden characters become `_`; trailing dots/spaces are trimmed; Windows device names (`CON`, `NUL`, ...) get a `_` prefix; `.`, `..` and empty names are rejected.

## Still to specify
- Cleanup of abandoned partial files
- Multi-file streams (Phase 4)
- Error codes

## Browser sharing
Code: `core/src/node/web.rs`, page: `core/src/node/web_page.html`. For people without Nudge. Off until the user turns it on ("Share without the app"); stops after 10 minutes with no browser connected.

**Transport.** Plain HTTP/1.1 on TCP port 47802 (any free port if taken), one request per connection (`Connection: close`). A browser can't trust Nudge's self-signed certificate without a warning, so there's no TLS: anyone who can watch the network traffic can read the files, and the link's secret is the only access control. Meant for networks the user trusts.

**The link** (shown as a QR code): `http://<this device's LAN IPv4>:<port>/t/<secret>`, where the secret is 16 random bytes as 32 hex characters. Every path outside `/t/<secret>` is a 404; the secret is compared in constant time. Responses carry `Cache-Control: no-store` and the page `Referrer-Policy: no-referrer`.

**Routes** (all under `/t/<secret>`; `s` is the tab's session ID, 64 hex):

| Route | Purpose |
|---|---|
| `GET /` | The page. |
| `GET /events[?s=]` | Server-sent events. Opening it makes the tab a device; an unknown or missing `s` starts a new session. Events: `hello` (`session`, `from` = this device's name, `platform` = its platform tag, `name` = what the tab appears as, `via` = how the tab reached it: `cable`, `wifi`, `wired`..., `cable` = this link's address on a phone's cable when the tab came another way, else null: a page that can reach it is on that phone, and moves there once nothing is under way), `offer` (`batch`, `files[]` of `id`, `name`, `size`), `withdrawn` (`batch`; no answer in 5 minutes), `sync` (sent on every (re)connect, before anything else: `owed[]` = sends still to download, as `id` (the send's batch ID), `name`, `size`, `ids[]` (its files); `done[]` = how recent downloads ended, as `id`, `ok`; `up[]` = keys of recent uploads that were saved; the page settles whatever it missed from this), `download` (an accepted send is ready: `id`, `name`, `size`, `ids[]`, `streaming`; true means a request already has it), `started` (`id`: a request for it arrived), `progress` (`id`, `bytes`, `size`, about 4 times a second while it streams), `delivered` (`id`: all of it has been streamed), `failed` (`id`: it broke off and wasn't asked for again), `received` (`k`: the upload with that key was saved), `stopped` (`why` = `user` or `idle`: sharing stopped; the stream closes after it), `clip` (`text`: clipboard text from the app; the page shows it with a Copy button, since a page can't write the clipboard by itself), `cancelled` (a send stopped: `what` = `down` with the `ids` taken off its list, or `up` for what the tab was sending; `by` = `them` when this device's user stopped it, `you` when the tab did). A `ping` event every 10 s: the page treats 25 s of silence as a lost connection. |
| `GET /ping` | `200 {}` while sharing. The page checks it when its connection drops, to tell "stopped" from "out of reach" (see below). |
| `POST /answer?s=&batch=&accept=1\|0` | Accept or decline an offer. |
| `GET /f/<batch>?s=` | Download an accepted send (`Content-Disposition: attachment`): its one file as it is, or several as one zip. A request that arrives before the Accept does (the page sends both from the same tap) waits up to 15 s for it. Asked for again meanwhile (a browser retrying a broken download, or the page's Download button), the newer request takes over and the older one is closed. A download that breaks off waits 10 s for such a retry before it fails; one the browser takes nothing of for 45 s has broken off. |
| `POST /ask?s=&count=&size=&name=` | "Can I send you these?": asks the user here (like a file offer). Replies when they answer: `{"answer":"accepted"|"declined"|"none"|"cancelled"}`. Accepting allows `count` uploads. |
| `POST /u?s=&name=[&k=]` | Send a file to this device. Needs `Content-Length`; the body is the file. Only as many as accepted through `/ask` (else 403); the name is sanitized like an `Offer` name. `k` (up to 32 letters and digits) is the page's key for it: once saved, a `received` event names it too, so a reply lost on the way doesn't make the page think it failed. |
| `POST /cancel?s=&what=up\|down` | The tab's Cancel. `up`: stop its upload under way, its open `/ask` (answered `cancelled`) and any uploads still allowed. `down`: stop the download under way and drop the rest it accepted. Uploads this device's user stops are answered 409. |
| `POST /clip?s=` | Clipboard text for this device (UTF-8 body, at most 1 MiB, needs `Content-Length`). Lands on the clipboard like text from a paired device, without a question. |

**Sessions.** A tab is listed while its event stream is open. When the stream drops (phone locked, tab in the background) it keeps its place for 30 s; reconnecting with its `s` within that time restores it, including any unanswered offer and the pending download. Tabs are named after the device type from the User-Agent ("iPhone", "Android phone", ...), with " 2", " 3" for repeats. Apps see them as peers with platform `browser`. A tab is temporary: it's never paired or remembered, and there's no "always accept" for it.

**Sending to a tab.** One offer per send (all its files), and one download per send. Browsers hold back a download no tap started once a page has had one (Chrome on Android then blocks the site's downloads outright, taps included), so one download per file doesn't work: several files go as one zip, named "3 files from Laptop.zip", and the page starts it in the Accept tap itself. The zip is written on the fly: entries stored (not compressed), names in UTF-8 with folders kept, each CRC-32 in a data descriptor after the file, Zip64 only where needed; its exact size is known up front (`core/src/archive.rs`). Sends download side by side. The app still sees each file, with its own progress; they finish together when the zip has gone. The page shows the same progress as the app (`progress`), and if a download hasn't started 4 s after it was asked for, it shows a Download button for it.

**Closing.** After each reply the server reads (and drops) what the browser is still sending for up to 0.5 s before closing: closing with a request body unread makes the system reset the connection, and a reset can wipe out the reply (a refused upload would see a network error instead of its 403).

**Stopping.** When sharing stops (the user, or 10 minutes with nobody connected), open tabs get `stopped` and their streams close; files on their way fail with "sharing was stopped". The link then answers for 15 more minutes: the page itself still loads, and every other route is `410 Gone` with `{"stopped":"user"|"idle"}`. Then the port closes. Sharing again makes a new link (on the usual port, which the old link lets go of), and the old one is a 404 there. So a page that loses its connection asks `/ping`, and:
- `200`: still sharing; reconnect.
- `410` or `404`: sharing stopped (or moved to a new link): the page says so and stops trying.
- No answer: out of reach (out of range, asleep, another network); it keeps trying, and says so after the second miss. If the browser reports no network at all, it says the phone is offline.
- Turned away at once, three times in a row (nothing listening, but the device is there): probably stopped; the page says so but keeps checking every 8 s. (A Windows firewall drops these silently, so there it looks like "out of reach" once the 15 minutes are up.)

## Real-time channel (screen mirroring)

**Starting a session (kind 11, Mirror).** Paired devices only. One byte: 1 = ShowMe (the asker wants to see the other's screen), 2 = ShowYou (the asker's user is offering its own screen). The reply is one byte: 0 Declined, 1 Accepted, 2 NoAnswer, 3 Locked (the device that would share is locked), 4 Busy (a question from this device is already open). For ShowMe the shared device asks its user first (up to 60 s, covering Android's screen-capture prompt); ShowYou is accepted without a question. A locked device never shares: it answers Locked, refuses to offer, and ends sharing sessions when it locks. After Accepted, the hello below follows on the same connection, which stays open for the session. Added within v7: an older build closes the connection on kind 11, which the asker reports as "needs an update"; nothing else changes for it.

Set up over a TLS connection the two devices already have. Each side sends a hello: `NURT`, version u8 (1), nonce [16] (chosen by the side that asks; echoed by the other), UDP port u16, count u8, then that many IPv4 addresses. Keys: TLS exporter, label `EXPORTER-nudge-realtime-1`, context = nonce, 68 bytes: key for asker→asked [32], key for asked→asker [32], session tag u32.

UDP packets: `0xA7`, session tag u32, sequence u64 (the AES-256-GCM nonce; the 13 bytes are authenticated), encrypted message, 16-byte tag. Messages (big-endian): see `core/src/realtime/wire.rs` (ping, pong, fragment, NACK, keyframe request, bye). Closing the TLS connection ends the session.
