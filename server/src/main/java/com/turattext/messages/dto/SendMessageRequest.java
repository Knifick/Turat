package com.turattext.messages.dto;

import jakarta.validation.constraints.NotBlank;
import jakarta.validation.constraints.NotNull;
import jakarta.validation.constraints.Size;

import java.util.UUID;

public record SendMessageRequest(
        @NotNull UUID chatId,
        @NotBlank @Size(max = 5_000_000) String encryptedContent,
        @NotBlank @Size(max = 200) String nonce,
        @NotBlank @Size(max = 120) String encryptionKeyId,
        UUID replyToMessageId
) {
}
