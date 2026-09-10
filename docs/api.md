# TuratText API

## Auth

- `POST /api/auth/register`
- `POST /api/auth/login`
- `POST /api/auth/refresh`

## Users

- `GET /api/users/search?q=...`
- `GET /api/users/{id}`
- `PATCH /api/users/me`

## Chats

- `POST /api/chats/direct`
- `GET /api/chats`
- `GET /api/chats/{id}/messages`

## Message actions

- `PUT /api/messages/{messageId}/reaction` - toggle or replace the current user's reaction.
- `PUT /api/messages/{messageId}/pin` - pin or unpin a message for the chat.
- `DELETE /api/messages/{messageId}` - soft-delete the current user's message.

Supported reactions: `❤`, `🔥`, `👌`, `😱`, `😭`, `🤨`, `👍`, `💔`.

## Servers

- `GET /api/servers`
- `GET /api/servers/health`
- `POST /api/servers/become-host`
- `POST /api/servers/heartbeat`
- `GET /api/servers/sync/status`

## Keys

- `GET /api/keys/public/{userId}`
- `POST /api/keys/public`
- `POST /api/keys/backup/encrypted`

## WebSocket

Endpoint: `/ws?token=<jwt>`

Client event examples:

```json
{ "type": "message.send", "payload": { "chatId": "...", "encryptedContent": "...", "nonce": "...", "encryptionKeyId": "...", "replyToMessageId": "..." } }
```

Server event examples:

```json
{ "type": "message.received", "payload": { "id": "...", "chatId": "...", "senderId": "...", "encryptedContent": "...", "timestamp": "..." } }
```

Interaction updates are broadcast as `message.reaction` and `message.state`. Reply previews are kept inside the encrypted payload; the server only sees the referenced message ID.
