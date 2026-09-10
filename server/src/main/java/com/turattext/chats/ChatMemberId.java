package com.turattext.chats;

import java.io.Serializable;
import java.util.Objects;
import java.util.UUID;

public class ChatMemberId implements Serializable {
    private UUID chat;
    private UUID user;

    public ChatMemberId() {
    }

    public ChatMemberId(UUID chat, UUID user) {
        this.chat = chat;
        this.user = user;
    }

    @Override
    public boolean equals(Object o) {
        if (this == o) {
            return true;
        }
        if (!(o instanceof ChatMemberId that)) {
            return false;
        }
        return Objects.equals(chat, that.chat) && Objects.equals(user, that.user);
    }

    @Override
    public int hashCode() {
        return Objects.hash(chat, user);
    }
}

