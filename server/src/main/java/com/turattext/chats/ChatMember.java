package com.turattext.chats;

import com.turattext.users.UserAccount;
import jakarta.persistence.Column;
import jakarta.persistence.Entity;
import jakarta.persistence.EnumType;
import jakarta.persistence.Enumerated;
import jakarta.persistence.Id;
import jakarta.persistence.IdClass;
import jakarta.persistence.JoinColumn;
import jakarta.persistence.ManyToOne;
import jakarta.persistence.PrePersist;
import jakarta.persistence.Table;

import java.time.Instant;

@Entity
@Table(name = "chat_members")
@IdClass(ChatMemberId.class)
public class ChatMember {
    @Id
    @ManyToOne(optional = false)
    @JoinColumn(name = "chat_id")
    private Chat chat;

    @Id
    @ManyToOne(optional = false)
    @JoinColumn(name = "user_id")
    private UserAccount user;

    @Enumerated(EnumType.STRING)
    @Column(nullable = false, length = 30)
    private ChatMemberRole role;

    @Column(name = "joined_at", nullable = false)
    private Instant joinedAt;

    @PrePersist
    void prePersist() {
        if (joinedAt == null) joinedAt = Instant.now();
    }

    public Chat getChat() {
        return chat;
    }

    public void setChat(Chat chat) {
        this.chat = chat;
    }

    public UserAccount getUser() {
        return user;
    }

    public void setUser(UserAccount user) {
        this.user = user;
    }

    public ChatMemberRole getRole() {
        return role;
    }

    public void setRole(ChatMemberRole role) {
        this.role = role;
    }

    public Instant getJoinedAt() {
        return joinedAt;
    }

    public void setJoinedAt(Instant joinedAt) {
        this.joinedAt = joinedAt;
    }
}
