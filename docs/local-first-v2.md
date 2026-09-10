# Local-first protocol v2

## Trust model

Client devices own identities, message keys and history. A Node is an untrusted, replaceable store
for opaque expiring data. A Relay is an untrusted fixed-target byte forwarder. Compromising either
must not reveal message plaintext or authorize a different cryptographic identity. Availability is
obtained by publishing routes to several independently identified Nodes.

Absolute traffic-analysis resistance is not claimed. The UI separates low-latency delivery from
optional metadata protection, and never describes either as anonymity against a global observer.

## Identity and devices

`UserID = tt1-SHA256(identity-SPKI)` and `DeviceID = ttd1-SHA256(device-SPKI)`. The identity key
certifies device keys. Only the authority installation and encrypted backup hold the identity
private key. A one-time `TTLINKV2` package contains a pre-generated device private key, its
root-signed certificate and the updated signed `DeviceList`; it does not contain the master key.

Routing records are signed by an active device key and carry the root-signed `DeviceList`. Both
client and Directory verify it. The Directory persists the greatest observed DeviceList sequence,
so a lost device cannot republish an older list after revocation even with a larger routing
sequence. Username and revocation changes remain authority-only operations.

## Local event log and message plane

Each event has a random 128-bit ID, monotonically increasing per-device sequence, device signature
and opaque payload. SQLite uses WAL; event bodies and durable delivery jobs are AES-256-GCM
encrypted with a platform-protected vault key. Inserts are append-only and deduplicate by event ID,
device sequence and outer-envelope ID.

The projection supports text, attachments, edit, delete, reaction, delivery and read events.
Successful Node upload is transport delivery; an E2EE receipt is end-device delivery. Network
failure retains encrypted jobs with bounded exponential retry.

## Direct-message encryption

Initial asynchronous sessions combine X25519 and ML-KEM-768 secrets, authenticate the claimed
signed/one-time prekeys and consume each one-time prekey once. Subsequent messages use ratcheted
chain keys, DH updates, AEAD associated data, replay checks and a bounded skipped-key cache.

Prekey state is generated independently per Node, so published one-time prekey batches do not
overlap. A routing descriptor fans one logical event out to every active recipient device and all
of its selected Mailbox Nodes. Each copy receives fresh outer randomness and fixed size-class
padding.

## Mailboxes and contact requests

Registration creates independent read, private-write and public-contact capabilities. Nodes store
only their SHA-256 hashes. Public routing exposes only the contact capability. Unknown senders may
deliver bounded text-only requests after stronger PoW; control events and attachments are dropped.
The E2EE delivery package returns random private write capabilities, used after manual acceptance.

Production defaults are registration PoW 18 bits, private envelope PoW 10 bits and contact PoW 18
bits. Per-IP rate, body size, mailbox count, TTL and cleanup limits are also enforced.

## Attachments, discovery and transports

Files are encrypted locally in 512 KiB chunks with a random FileKey. Ciphertext may be replicated
to several Blob Nodes; digests, read capabilities and FileKey exist only in the E2EE manifest.

Node and Relay descriptors are Ed25519-signed and expiring. Previously verified descriptors are
cached. Signed `.ttbridge` bundles provide social discovery; `.ttenv` bundles carry opaque
envelopes through removable media or another delay-tolerant path.

The HTTP client negotiates HTTP/2 or higher and Caddy advertises HTTP/3/QUIC. Direct connections
fall back to signed fixed-target TLS relays. High privacy mode tries a randomized matching relay
first and adds 3–15 seconds of per-copy send jitter; balanced mode adds up to 2 seconds; fast mode
keeps only randomized padded envelopes.

## Transparency and updates

Routing and username mutations append node-signed immutable operations. Clients reconstruct and
pin Merkle checkpoints and reject inconsistent roots for an already observed tree size.

Release metadata uses an embedded 2-of-3 Ed25519 trust root. The client checks canonical body JSON,
signature threshold, expiry, platform artifact size/SHA-256 and pins channel sequence/hash to reject
rollback and same-sequence equivocation. Signing keys and Android keystore remain offline.

## Groups

The current group control plane has root-authenticated membership commits, epoch-secret
commitments, parent hashes, explicit fork detection and per-sender chains with bounded out-of-order
keys. Epoch secrets must be distributed over existing pairwise E2EE sessions. This is an
MLS-analogous implementation, not RFC 9420 conformance; replacing it with an audited MLS core is a
pre-public-audit task.

## Compatibility

Protocol structures carry explicit versions and releases have long manifest lifetimes. The v2 Node
can run without legacy API (`TURATTEXT_V2_ONLY=true`), while old client/server source remains in the
repository only for local history migration. No expiry switch disables old clients.
