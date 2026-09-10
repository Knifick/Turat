# Database Schema

PostgreSQL is used for the MVP because it handles concurrent users, indexed search, WebSocket-driven writes and future replication better than SQLite.

## Public/plain metadata

- `users.login`, dates and enabled flags.
- `user_profiles.display_name`, `avatar_url`, `status`, `description`.
- `user_public_keys.public_key`.
- chat membership and message timestamps.

## Encrypted-only content

- `messages.encrypted_content`.
- `encrypted_key_backups.encrypted_private_key`.

## Tables

- `users`: account and password hash.
- `user_profiles`: public profile fields.
- `user_public_keys`: public identity keys and future rotations.
- `chats`: direct/group-ready chat metadata.
- `chat_members`: membership table.
- `messages`: encrypted message storage.
- `servers`: primary and mirror server directory.
- `server_sync_state`: mirror sync checkpoints.
- `encrypted_key_backups`: optional client-encrypted private key backup.

See `server/src/main/resources/db/migration/V1__init.sql` for exact fields, indexes and foreign keys.

