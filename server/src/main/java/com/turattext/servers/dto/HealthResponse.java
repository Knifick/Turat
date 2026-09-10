package com.turattext.servers.dto;

import java.time.Instant;

public record HealthResponse(
        String status,
        String serverTime,
        Instant checkedAt
) {
}

