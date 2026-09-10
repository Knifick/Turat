package com.turattext.admin;

import java.time.Instant;

public record AdminLoginResponse(
        String token,
        Instant expiresAt
) {
}
