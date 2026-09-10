package com.turattext.replication.dto;

import java.time.Instant;
import java.util.List;
import java.util.UUID;

public record ReplicationSnapshot(
        Instant generatedAt,
        List<UserRecord> users,
        List<ProfileRecord> profiles,
        List<PublicKeyRecord> publicKeys,
        List<KeyBackupRecord> keyBackups,
        List<ChatRecord> chats,
        List<ChatMemberRecord> chatMembers,
        List<MessageRecord> messages,
        List<MessageReactionRecord> messageReactions,
        List<ServerRecord> servers,
        List<AvatarRecord> avatars
) {
    public record UserRecord(
            UUID id, String login, String passwordHash, boolean enabled,
            Instant createdAt, Instant updatedAt
    ) {}

    public record ProfileRecord(
            UUID userId, String displayName, String avatarUrl, String status,
            String description, Instant updatedAt
    ) {}

    public record PublicKeyRecord(
            UUID id, UUID userId, String keyId, String algorithm, String publicKey,
            Instant createdAt, Instant revokedAt
    ) {}

    public record KeyBackupRecord(
            UUID id, UUID userId, String keyId, String algorithm, String publicKey,
            String salt, String nonce, String kdf, String encryptedPrivateKey, Instant createdAt
    ) {}

    public record ChatRecord(UUID id, String type, Instant createdAt, Instant updatedAt) {}

    public record ChatMemberRecord(
            UUID chatId, UUID userId, String role, Instant joinedAt
    ) {}

    public record MessageRecord(
            UUID id, UUID chatId, UUID senderId, String encryptedContent, String nonce,
            String encryptionKeyId, String status, UUID replyToMessageId, boolean pinned,
            Instant createdAt, Instant serverReceivedAt, Instant updatedAt
    ) {}

    public record MessageReactionRecord(
            UUID messageId, UUID userId, String reaction, boolean active, Instant updatedAt
    ) {}

    public record ServerRecord(
            UUID id, String name, String baseUrl, String publicKey, UUID ownerUserId,
            String replicationKey, String role, String status, Instant lastSeenAt,
            Instant createdAt, Instant updatedAt
    ) {}

    public record AvatarRecord(UUID userId, String extension, String base64Content) {}
}
