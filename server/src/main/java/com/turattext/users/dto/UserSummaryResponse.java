package com.turattext.users.dto;

import java.util.UUID;

public record UserSummaryResponse(
        UUID id,
        String login,
        String displayName,
        String avatarUrl,
        String status,
        String description,
        boolean online
) {
}
