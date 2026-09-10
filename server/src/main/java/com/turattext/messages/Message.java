package com.turattext.messages;

import com.turattext.chats.Chat;
import com.turattext.users.UserAccount;
import jakarta.persistence.Column;
import jakarta.persistence.Entity;
import jakarta.persistence.EnumType;
import jakarta.persistence.Enumerated;
import jakarta.persistence.Id;
import jakarta.persistence.JoinColumn;
import jakarta.persistence.Lob;
import jakarta.persistence.ManyToOne;
import jakarta.persistence.PrePersist;
import jakarta.persistence.PreUpdate;
import jakarta.persistence.Table;

import java.time.Instant;
import java.util.UUID;

@Entity
@Table(name = "messages")
public class Message {
    @Id
    private UUID id;

    @ManyToOne(optional = false)
    @JoinColumn(name = "chat_id")
    private Chat chat;

    @ManyToOne(optional = false)
    @JoinColumn(name = "sender_id")
    private UserAccount sender;

    @Lob
    @Column(name = "encrypted_content", nullable = false, columnDefinition = "TEXT")
    private String encryptedContent;

    @Column(nullable = false, length = 200)
    private String nonce;

    @Column(name = "encryption_key_id", nullable = false, length = 120)
    private String encryptionKeyId;

    @Column(name = "reply_to_message_id")
    private UUID replyToMessageId;

    @Column
    private boolean pinned;

    @Enumerated(EnumType.STRING)
    @Column(nullable = false, length = 30)
    private MessageStatus status = MessageStatus.SENT;

    @Column(name = "created_at", nullable = false)
    private Instant createdAt;

    @Column(name = "server_received_at", nullable = false)
    private Instant serverReceivedAt;

    @Column(name = "updated_at")
    private Instant updatedAt;

    @PrePersist
    void prePersist() {
        Instant now = Instant.now();
        if (id == null) {
            id = UUID.randomUUID();
        }
        if (createdAt == null) createdAt = now;
        if (serverReceivedAt == null) serverReceivedAt = now;
        if (updatedAt == null) updatedAt = now;
    }

    @PreUpdate
    void preUpdate() {
        if (updatedAt == null) updatedAt = Instant.now();
    }

    public UUID getId() {
        return id;
    }

    public void setId(UUID id) {
        this.id = id;
    }

    public Chat getChat() {
        return chat;
    }

    public void setChat(Chat chat) {
        this.chat = chat;
    }

    public UserAccount getSender() {
        return sender;
    }

    public void setSender(UserAccount sender) {
        this.sender = sender;
    }

    public String getEncryptedContent() {
        return encryptedContent;
    }

    public void setEncryptedContent(String encryptedContent) {
        this.encryptedContent = encryptedContent;
    }

    public String getNonce() {
        return nonce;
    }

    public void setNonce(String nonce) {
        this.nonce = nonce;
    }

    public String getEncryptionKeyId() {
        return encryptionKeyId;
    }

    public void setEncryptionKeyId(String encryptionKeyId) {
        this.encryptionKeyId = encryptionKeyId;
    }

    public UUID getReplyToMessageId() {
        return replyToMessageId;
    }

    public void setReplyToMessageId(UUID replyToMessageId) {
        this.replyToMessageId = replyToMessageId;
    }

    public boolean isPinned() {
        return pinned;
    }

    public void setPinned(boolean pinned) {
        this.pinned = pinned;
    }

    public MessageStatus getStatus() {
        return status;
    }

    public void setStatus(MessageStatus status) {
        this.status = status;
    }

    public Instant getCreatedAt() {
        return createdAt;
    }

    public Instant getServerReceivedAt() {
        return serverReceivedAt;
    }

    public void setCreatedAt(Instant createdAt) {
        this.createdAt = createdAt;
    }

    public void setServerReceivedAt(Instant serverReceivedAt) {
        this.serverReceivedAt = serverReceivedAt;
    }

    public Instant getUpdatedAt() {
        return updatedAt;
    }

    public void setUpdatedAt(Instant updatedAt) {
        this.updatedAt = updatedAt;
    }
}
