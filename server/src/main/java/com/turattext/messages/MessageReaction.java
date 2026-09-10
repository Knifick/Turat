package com.turattext.messages;

import com.turattext.users.UserAccount;
import jakarta.persistence.Column;
import jakarta.persistence.EmbeddedId;
import jakarta.persistence.Entity;
import jakarta.persistence.FetchType;
import jakarta.persistence.JoinColumn;
import jakarta.persistence.ManyToOne;
import jakarta.persistence.MapsId;
import jakarta.persistence.PrePersist;
import jakarta.persistence.PreUpdate;
import jakarta.persistence.Table;

import java.time.Instant;

@Entity
@Table(name = "message_reactions")
public class MessageReaction {
    @EmbeddedId
    private MessageReactionId id;

    @MapsId("messageId")
    @ManyToOne(optional = false, fetch = FetchType.LAZY)
    @JoinColumn(name = "message_id")
    private Message message;

    @MapsId("userId")
    @ManyToOne(optional = false, fetch = FetchType.LAZY)
    @JoinColumn(name = "user_id")
    private UserAccount user;

    @Column(nullable = false, length = 24)
    private String reaction;

    @Column(nullable = false)
    private boolean active = true;

    @Column(name = "updated_at", nullable = false)
    private Instant updatedAt;

    @PrePersist
    @PreUpdate
    void touch() {
        if (updatedAt == null) updatedAt = Instant.now();
    }

    public MessageReactionId getId() { return id; }
    public void setId(MessageReactionId id) { this.id = id; }
    public Message getMessage() { return message; }
    public void setMessage(Message message) { this.message = message; }
    public UserAccount getUser() { return user; }
    public void setUser(UserAccount user) { this.user = user; }
    public String getReaction() { return reaction; }
    public void setReaction(String reaction) { this.reaction = reaction; }
    public boolean isActive() { return active; }
    public void setActive(boolean active) { this.active = active; }
    public Instant getUpdatedAt() { return updatedAt; }
    public void setUpdatedAt(Instant updatedAt) { this.updatedAt = updatedAt; }
}
