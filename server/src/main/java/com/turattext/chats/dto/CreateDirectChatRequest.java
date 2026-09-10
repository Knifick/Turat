package com.turattext.chats.dto;

import jakarta.validation.constraints.NotNull;

import java.util.UUID;

public record CreateDirectChatRequest(@NotNull UUID userId) {
}

