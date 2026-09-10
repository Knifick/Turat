package com.turattext.users.dto;

import java.time.Instant;
import java.util.UUID;

public record PublicKeyResponse(
        UUID id,
        UUID userId,
        String keyId,
        String algorithm,
        String publicKey,
        Instant createdAt
) {
}

