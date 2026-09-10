package com.turattext.messages;

import jakarta.persistence.Column;
import jakarta.persistence.Embeddable;

import java.io.Serializable;
import java.util.Objects;
import java.util.UUID;

@Embeddable
public class MessageReactionId implements Serializable {
    @Column(name = "message_id")
    private UUID messageId;

    @Column(name = "user_id")
    private UUID userId;

    protected MessageReactionId() {
    }

    public MessageReactionId(UUID messageId, UUID userId) {
        this.messageId = messageId;
        this.userId = userId;
    }

    public UUID getMessageId() {
        return messageId;
    }

    public UUID getUserId() {
        return userId;
    }

    @Override
    public boolean equals(Object value) {
        if (this == value) return true;
        if (!(value instanceof MessageReactionId other)) return false;
        return Objects.equals(messageId, other.messageId) && Objects.equals(userId, other.userId);
    }

    @Override
    public int hashCode() {
        return Objects.hash(messageId, userId);
    }
}
