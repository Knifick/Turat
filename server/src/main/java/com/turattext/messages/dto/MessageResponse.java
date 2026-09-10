package com.turattext.messages.dto;

import com.turattext.messages.MessageStatus;

import java.time.Instant;
import java.util.UUID;
import java.util.List;

public record MessageResponse(
        UUID id,
        UUID chatId,
        UUID senderId,
        String encryptedContent,
        String nonce,
        String encryptionKeyId,
        MessageStatus status,
        Instant timestamp,
        UUID replyToMessageId,
        boolean pinned,
        List<MessageReactionResponse> reactions
) {
}
