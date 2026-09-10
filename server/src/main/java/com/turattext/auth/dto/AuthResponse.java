package com.turattext.auth.dto;

import java.time.Instant;
import java.util.UUID;

public record AuthResponse(
        String accessToken,
        String refreshToken,
        UUID userId,
        String login,
        String displayName,
        Instant registeredAt
) {
}

