package com.turattext.messages;

import org.springframework.data.jpa.repository.JpaRepository;

import java.util.List;
import java.util.UUID;

public interface MessageReactionRepository extends JpaRepository<MessageReaction, MessageReactionId> {
    List<MessageReaction> findByMessage_IdAndActiveTrue(UUID messageId);
}
