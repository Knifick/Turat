package com.turattext.admin.dto;

import java.time.Instant;
import java.util.UUID;

public record AdminUserResponse(
        UUID id,
        String login,
        String displayName,
        String status,
        boolean enabled,
        Instant createdAt
) {
}
