package com.turattext.messages.dto;

import java.util.UUID;

public record MessageStateResponse(UUID messageId, UUID chatId, boolean pinned, boolean deleted) {
}
