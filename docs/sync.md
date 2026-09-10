# Server Synchronization

## MVP model

The first publicly registered host becomes `PRIMARY`. Every later host is registered as `MIRROR`.

Hosts send heartbeat events to the bootstrap directory. If the current `PRIMARY` stops sending heartbeat and becomes stale, the oldest online mirror is promoted to `PRIMARY`.

Registered desktop hosts replicate in both directions. A mirror pushes its local changes to the authority and receives the resulting canonical snapshot every 10 seconds. If the authority is unreachable for about 90 seconds, the oldest reachable mirror is selected; when no older mirror is reachable, the local host promotes itself to `PRIMARY` and continues serving clients.

Clients cache the last server directory. When the VPS cannot be reached, the server picker probes cached mirrors and selects a reachable one. User JWT signing keys are local to each server, so switching servers requires one password login; account hashes and encrypted device-key backups are already replicated.

Mirrors can store:

- users and public profiles;
- public keys;
- chats and chat members;
- encrypted messages;
- encrypted reply references, pin state and soft-delete tombstones;
- message reactions, including inactive tombstones needed to prevent deleted reactions from reappearing;
- encrypted private-key backups;
- avatar files;
- server directory and sync state.

Mirrors must never receive private keys or plaintext messages.

Admin passwords, admin tokens and VPS configuration are not part of a replication snapshot. Administrative authentication remains on the VPS authority even while a mirror is serving normal messenger traffic during failover.

## Transport security

Each host receives a random 256-bit replication key after an authenticated `become-host` request. Snapshot requests and responses use AES-256-GCM with a fresh nonce, node ID as associated data, timestamp validation and replay rejection. The key is written to the local host configuration through a loopback-only endpoint protected by a per-installation control secret.

This protects replication payloads on the current HTTP deployment, but HTTPS is still required before treating the transport as production-grade.

## Statuses

- `online`: health check succeeds and sync lag is acceptable.
- `offline`: health check fails.
- `syncing`: mirror is reachable but catching up.
- `error`: last sync failed with a stored error message.

## Conflicts

Records have stable UUIDs. Chat memberships remain immutable. Messages, pins, deletion tombstones and reactions carry update timestamps and are merged by their newest state; mutable profiles use the same rule. The VPS snapshot is canonical after reconnection. If two disconnected nodes create different accounts with the same login, the account already accepted by the VPS wins rather than silently combining identities.

This is snapshot replication for the MVP. A production federation still needs an append-only event log, durable per-node offsets, explicit tombstones and a stronger partition-conflict model.

## Bootstrap VPS

A small VPS can run the TuratText backend as a bootstrap/rendezvous node:

- stores the public server directory;
- exposes `/api/servers`;
- receives `/api/servers/become-host`;
- receives `/api/servers/heartbeat`;
- tells clients which host is currently `PRIMARY`.
- acts as the canonical replication authority while it is online.

Do not store VPS root passwords in the repository. Use SSH keys and a non-root deployment user for production.
