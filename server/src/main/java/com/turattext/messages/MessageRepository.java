package com.turattext.messages;

import org.springframework.data.jpa.repository.JpaRepository;

import java.util.List;
import java.util.UUID;

public interface MessageRepository extends JpaRepository<Message, UUID> {
    List<Message> findByChat_IdOrderByCreatedAtAsc(UUID chatId);
}

