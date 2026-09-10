package com.turattext.replication;

import com.turattext.chats.Chat;
import com.turattext.chats.ChatMember;
import com.turattext.chats.ChatMemberId;
import com.turattext.chats.ChatMemberRepository;
import com.turattext.chats.ChatMemberRole;
import com.turattext.chats.ChatRepository;
import com.turattext.chats.ChatType;
import com.turattext.messages.Message;
import com.turattext.messages.MessageRepository;
import com.turattext.messages.MessageReaction;
import com.turattext.messages.MessageReactionId;
import com.turattext.messages.MessageReactionRepository;
import com.turattext.messages.MessageStatus;
import com.turattext.replication.dto.ReplicationSnapshot;
import com.turattext.servers.ServerNode;
import com.turattext.servers.ServerNodeRepository;
import com.turattext.servers.ServerRole;
import com.turattext.servers.ServerStatus;
import com.turattext.users.EncryptedKeyBackup;
import com.turattext.users.EncryptedKeyBackupRepository;
import com.turattext.users.UserAccount;
import com.turattext.users.UserAccountRepository;
import com.turattext.users.UserProfile;
import com.turattext.users.UserProfileRepository;
import com.turattext.users.UserPublicKey;
import com.turattext.users.UserPublicKeyRepository;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.stereotype.Service;
import org.springframework.transaction.annotation.Transactional;

import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Instant;
import java.util.ArrayList;
import java.util.Base64;
import java.util.Comparator;
import java.util.List;
import java.util.Objects;
import java.util.UUID;

@Service
public class ReplicationSnapshotService {
    private static final int MAX_AVATAR_BYTES = 5 * 1024 * 1024;

    private final UserAccountRepository users;
    private final UserProfileRepository profiles;
    private final UserPublicKeyRepository publicKeys;
    private final EncryptedKeyBackupRepository keyBackups;
    private final ChatRepository chats;
    private final ChatMemberRepository chatMembers;
    private final MessageRepository messages;
    private final MessageReactionRepository messageReactions;
    private final ServerNodeRepository servers;
    private final Path avatarDirectory;
    private final boolean authority;

    public ReplicationSnapshotService(
            UserAccountRepository users,
            UserProfileRepository profiles,
            UserPublicKeyRepository publicKeys,
            EncryptedKeyBackupRepository keyBackups,
            ChatRepository chats,
            ChatMemberRepository chatMembers,
            MessageRepository messages,
            MessageReactionRepository messageReactions,
            ServerNodeRepository servers,
            @Value("${turattext.media.directory:./data/media}") String mediaDirectory,
            @Value("${turattext.replication.authority:false}") boolean authority
    ) {
        this.users = users;
        this.profiles = profiles;
        this.publicKeys = publicKeys;
        this.keyBackups = keyBackups;
        this.chats = chats;
        this.chatMembers = chatMembers;
        this.messages = messages;
        this.messageReactions = messageReactions;
        this.servers = servers;
        this.avatarDirectory = Path.of(mediaDirectory).toAbsolutePath().normalize().resolve("avatars");
        this.authority = authority;
    }

    @Transactional(readOnly = true)
    public ReplicationSnapshot exportSnapshot() {
        return new ReplicationSnapshot(
                Instant.now(),
                users.findAll().stream().map(user -> new ReplicationSnapshot.UserRecord(
                        user.getId(), user.getLogin(), user.getPasswordHash(), user.isEnabled(),
                        user.getCreatedAt(), user.getUpdatedAt())).toList(),
                profiles.findAll().stream().map(profile -> new ReplicationSnapshot.ProfileRecord(
                        profile.getUserId(), profile.getDisplayName(), profile.getAvatarUrl(),
                        profile.getStatus(), profile.getDescription(), profile.getUpdatedAt())).toList(),
                publicKeys.findAll().stream().map(key -> new ReplicationSnapshot.PublicKeyRecord(
                        key.getId(), key.getUser().getId(), key.getKeyId(), key.getAlgorithm(),
                        key.getPublicKey(), key.getCreatedAt(), key.getRevokedAt())).toList(),
                keyBackups.findAll().stream().map(backup -> new ReplicationSnapshot.KeyBackupRecord(
                        backup.getId(), backup.getUser().getId(), backup.getKeyId(), backup.getAlgorithm(),
                        backup.getPublicKey(), backup.getSalt(), backup.getNonce(), backup.getKdf(),
                        backup.getEncryptedPrivateKey(), backup.getCreatedAt())).toList(),
                chats.findAll().stream().map(chat -> new ReplicationSnapshot.ChatRecord(
                        chat.getId(), chat.getType().name(), chat.getCreatedAt(), chat.getUpdatedAt())).toList(),
                chatMembers.findAll().stream().map(member -> new ReplicationSnapshot.ChatMemberRecord(
                        member.getChat().getId(), member.getUser().getId(), member.getRole().name(),
                        member.getJoinedAt())).toList(),
                messages.findAll().stream().map(message -> new ReplicationSnapshot.MessageRecord(
                        message.getId(), message.getChat().getId(), message.getSender().getId(),
                        message.getEncryptedContent(), message.getNonce(), message.getEncryptionKeyId(),
                        message.getStatus().name(), message.getReplyToMessageId(), message.isPinned(),
                        message.getCreatedAt(), message.getServerReceivedAt(), message.getUpdatedAt())).toList(),
                messageReactions.findAll().stream().map(reaction ->
                        new ReplicationSnapshot.MessageReactionRecord(
                                reaction.getMessage().getId(), reaction.getUser().getId(),
                                reaction.getReaction(), reaction.isActive(), reaction.getUpdatedAt())).toList(),
                servers.findAll().stream().map(server -> new ReplicationSnapshot.ServerRecord(
                        server.getId(), server.getName(), server.getBaseUrl(), server.getPublicKey(),
                        server.getOwnerUserId(), server.getReplicationKey(), server.getRole().name(),
                        server.getStatus().name(), server.getLastSeenAt(), server.getCreatedAt(),
                        server.getUpdatedAt())).toList(),
                exportAvatars()
        );
    }

    @Transactional
    public void mergeFromPeer(ReplicationSnapshot snapshot) {
        merge(snapshot, false);
    }

    @Transactional
    public void applyCanonical(ReplicationSnapshot snapshot) {
        merge(snapshot, true);
    }

    private void merge(ReplicationSnapshot snapshot, boolean canonical) {
        mergeUsers(safe(snapshot.users()), canonical);
        mergeProfiles(safe(snapshot.profiles()), canonical);
        mergePublicKeys(safe(snapshot.publicKeys()), canonical);
        mergeKeyBackups(safe(snapshot.keyBackups()));
        mergeChats(safe(snapshot.chats()), canonical);
        mergeChatMembers(safe(snapshot.chatMembers()));
        mergeMessages(safe(snapshot.messages()), canonical);
        mergeMessageReactions(safe(snapshot.messageReactions()), canonical);
        if (canonical || !authority) {
            mergeServers(safe(snapshot.servers()), canonical);
        }
        importAvatars(safe(snapshot.avatars()));
    }

    private void mergeUsers(List<ReplicationSnapshot.UserRecord> records, boolean canonical) {
        for (var record : records) {
            UserAccount existing = users.findById(record.id()).orElse(null);
            if (existing == null) {
                if (users.findByLoginIgnoreCase(record.login()).isPresent()) continue;
                UserAccount user = new UserAccount();
                user.setId(record.id());
                user.setLogin(record.login());
                user.setPasswordHash(record.passwordHash());
                user.setEnabled(record.enabled());
                user.setCreatedAt(record.createdAt());
                user.setUpdatedAt(record.updatedAt());
                users.save(user);
            } else if (canonical || !authority) {
                if (isNotOlder(record.updatedAt(), existing.getUpdatedAt())) {
                    existing.setPasswordHash(record.passwordHash());
                    existing.setEnabled(record.enabled());
                    users.save(existing);
                }
            }
        }
    }

    private void mergeProfiles(List<ReplicationSnapshot.ProfileRecord> records, boolean canonical) {
        for (var record : records) {
            UserAccount user = users.findById(record.userId()).orElse(null);
            if (user == null) continue;
            UserProfile profile = profiles.findById(record.userId()).orElse(null);
            boolean isNew = profile == null;
            if (profile == null) {
                profile = new UserProfile();
                user.setProfile(profile);
            } else if (!canonical && authority
                    && !isNotOlder(record.updatedAt(), profile.getUpdatedAt())) {
                continue;
            }
            profile.setDisplayName(record.displayName());
            profile.setAvatarUrl(record.avatarUrl());
            profile.setStatus(record.status());
            profile.setDescription(record.description());
            profile.setUpdatedAt(record.updatedAt());
            if (isNew) {
                users.save(user);
            } else {
                profiles.save(profile);
            }
        }
    }

    private void mergePublicKeys(List<ReplicationSnapshot.PublicKeyRecord> records, boolean canonical) {
        for (var record : records) {
            UserAccount user = users.findById(record.userId()).orElse(null);
            if (user == null) continue;
            UserPublicKey key = publicKeys.findByKeyId(record.keyId()).orElse(null);
            if (key == null) {
                key = new UserPublicKey();
                key.setId(record.id());
                key.setUser(user);
                key.setKeyId(record.keyId());
                key.setAlgorithm(record.algorithm());
                key.setPublicKey(record.publicKey());
                key.setCreatedAt(record.createdAt());
            }
            if (canonical || key.getRevokedAt() == null || record.revokedAt() != null) {
                key.setRevokedAt(record.revokedAt());
            }
            publicKeys.save(key);
        }
    }

    private void mergeKeyBackups(List<ReplicationSnapshot.KeyBackupRecord> records) {
        for (var record : records) {
            if (keyBackups.existsById(record.id())) continue;
            UserAccount user = users.findById(record.userId()).orElse(null);
            if (user == null) continue;
            EncryptedKeyBackup backup = new EncryptedKeyBackup();
            backup.setId(record.id());
            backup.setUser(user);
            backup.setKeyId(record.keyId());
            backup.setAlgorithm(record.algorithm());
            backup.setPublicKey(record.publicKey());
            backup.setSalt(record.salt());
            backup.setNonce(record.nonce());
            backup.setKdf(record.kdf());
            backup.setEncryptedPrivateKey(record.encryptedPrivateKey());
            backup.setCreatedAt(record.createdAt());
            keyBackups.save(backup);
        }
    }

    private void mergeChats(List<ReplicationSnapshot.ChatRecord> records, boolean canonical) {
        for (var record : records) {
            Chat chat = chats.findById(record.id()).orElse(null);
            if (chat == null) {
                chat = new Chat();
                chat.setId(record.id());
                chat.setType(ChatType.valueOf(record.type()));
                chat.setCreatedAt(record.createdAt());
                chat.setUpdatedAt(record.updatedAt());
                chats.save(chat);
            } else if (canonical || isNotOlder(record.updatedAt(), chat.getUpdatedAt())) {
                chat.setUpdatedAt(record.updatedAt());
                chats.save(chat);
            }
        }
    }

    private void mergeChatMembers(List<ReplicationSnapshot.ChatMemberRecord> records) {
        for (var record : records) {
            ChatMemberId id = new ChatMemberId(record.chatId(), record.userId());
            if (chatMembers.existsById(id)) continue;
            Chat chat = chats.findById(record.chatId()).orElse(null);
            UserAccount user = users.findById(record.userId()).orElse(null);
            if (chat == null || user == null) continue;
            ChatMember member = new ChatMember();
            member.setChat(chat);
            member.setUser(user);
            member.setRole(ChatMemberRole.valueOf(record.role()));
            member.setJoinedAt(record.joinedAt());
            chatMembers.save(member);
        }
    }

    private void mergeMessages(List<ReplicationSnapshot.MessageRecord> records, boolean canonical) {
        for (var record : records) {
            Message message = messages.findById(record.id()).orElse(null);
            boolean isNew = message == null;
            if (!isNew && messageMatches(message, record)) continue;
            if (!isNew && !canonical && !isNotOlder(record.updatedAt(), message.getUpdatedAt())) continue;
            Chat chat = chats.findById(record.chatId()).orElse(null);
            UserAccount sender = users.findById(record.senderId()).orElse(null);
            if (chat == null || sender == null) continue;
            if (isNew) {
                message = new Message();
                message.setId(record.id());
                message.setChat(chat);
                message.setSender(sender);
            }
            message.setEncryptedContent(record.encryptedContent());
            message.setNonce(record.nonce());
            message.setEncryptionKeyId(record.encryptionKeyId());
            message.setStatus(MessageStatus.valueOf(record.status()));
            message.setReplyToMessageId(record.replyToMessageId());
            message.setPinned(record.pinned());
            message.setCreatedAt(record.createdAt());
            message.setServerReceivedAt(record.serverReceivedAt());
            message.setUpdatedAt(record.updatedAt());
            messages.save(message);
        }
    }

    private void mergeMessageReactions(
            List<ReplicationSnapshot.MessageReactionRecord> records,
            boolean canonical
    ) {
        for (var record : records) {
            Message message = messages.findById(record.messageId()).orElse(null);
            UserAccount user = users.findById(record.userId()).orElse(null);
            if (message == null || user == null) continue;
            MessageReactionId id = new MessageReactionId(record.messageId(), record.userId());
            MessageReaction reaction = messageReactions.findById(id).orElse(null);
            if (reaction != null
                    && Objects.equals(reaction.getReaction(), record.reaction())
                    && reaction.isActive() == record.active()) continue;
            if (reaction != null && !canonical
                    && !isNotOlder(record.updatedAt(), reaction.getUpdatedAt())) continue;
            if (reaction == null) {
                reaction = new MessageReaction();
                reaction.setId(id);
                reaction.setMessage(message);
                reaction.setUser(user);
            }
            reaction.setReaction(record.reaction());
            reaction.setActive(record.active());
            reaction.setUpdatedAt(record.updatedAt());
            messageReactions.save(reaction);
        }
    }

    private static boolean messageMatches(
            Message message,
            ReplicationSnapshot.MessageRecord record
    ) {
        return Objects.equals(message.getEncryptedContent(), record.encryptedContent())
                && Objects.equals(message.getNonce(), record.nonce())
                && Objects.equals(message.getEncryptionKeyId(), record.encryptionKeyId())
                && Objects.equals(message.getReplyToMessageId(), record.replyToMessageId())
                && message.isPinned() == record.pinned()
                && message.getStatus().name().equals(record.status());
    }

    private void mergeServers(List<ReplicationSnapshot.ServerRecord> records, boolean canonical) {
        for (var record : records) {
            ServerNode server = servers.findById(record.id()).orElseGet(ServerNode::new);
            boolean isNew = server.getId() == null;
            if (isNew) server.setId(record.id());
            if (!isNew && !canonical && server.getRole() == ServerRole.PRIMARY) continue;
            server.setName(record.name());
            server.setBaseUrl(record.baseUrl());
            server.setPublicKey(record.publicKey());
            server.setOwnerUserId(record.ownerUserId());
            server.setReplicationKey(record.replicationKey());
            server.setRole(ServerRole.valueOf(record.role()));
            server.setStatus(ServerStatus.valueOf(record.status()));
            server.setLastSeenAt(record.lastSeenAt());
            server.setCreatedAt(record.createdAt());
            server.setUpdatedAt(record.updatedAt());
            servers.save(server);
        }
    }

    private List<ReplicationSnapshot.AvatarRecord> exportAvatars() {
        if (!Files.isDirectory(avatarDirectory)) return List.of();
        List<ReplicationSnapshot.AvatarRecord> result = new ArrayList<>();
        try (var paths = Files.list(avatarDirectory)) {
            for (Path path : paths.filter(Files::isRegularFile).toList()) {
                String fileName = path.getFileName().toString();
                int dot = fileName.lastIndexOf('.');
                if (dot <= 0) continue;
                try {
                    UUID userId = UUID.fromString(fileName.substring(0, dot));
                    byte[] bytes = Files.readAllBytes(path);
                    if (bytes.length <= MAX_AVATAR_BYTES) {
                        result.add(new ReplicationSnapshot.AvatarRecord(
                                userId,
                                fileName.substring(dot),
                                Base64.getEncoder().encodeToString(bytes)));
                    }
                } catch (Exception ignored) {
                }
            }
        } catch (Exception ignored) {
        }
        return result;
    }

    private void importAvatars(List<ReplicationSnapshot.AvatarRecord> records) {
        for (var record : records) {
            if (!(record.extension().equals(".jpg") || record.extension().equals(".png"))) continue;
            try {
                byte[] bytes = Base64.getDecoder().decode(record.base64Content());
                if (bytes.length > MAX_AVATAR_BYTES) continue;
                Files.createDirectories(avatarDirectory);
                Files.write(avatarDirectory.resolve(record.userId() + record.extension()), bytes);
            } catch (Exception ignored) {
            }
        }
    }

    private static boolean isNotOlder(Instant incoming, Instant existing) {
        return existing == null || incoming == null || !incoming.isBefore(existing);
    }

    private static <T> List<T> safe(List<T> values) {
        return values == null ? List.of() : values.stream().filter(Objects::nonNull).toList();
    }
}
