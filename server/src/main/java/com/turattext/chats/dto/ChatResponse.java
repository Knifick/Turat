package com.turattext.chats.dto;

import com.turattext.chats.ChatType;
import com.turattext.users.dto.UserSummaryResponse;

import java.time.Instant;
import java.util.List;
import java.util.UUID;

public record ChatResponse(
        UUID id,
        ChatType type,
        String title,
        String avatarUrl,
        List<UserSummaryResponse> members,
        Instant updatedAt
) {
}

