package com.turattext.servers.dto;

import com.turattext.servers.ServerRole;
import com.turattext.servers.ServerStatus;

import java.time.Instant;
import java.util.UUID;

public record ServerResponse(
        UUID id,
        String name,
        String baseUrl,
        ServerRole role,
        ServerStatus status,
        Instant lastSeenAt
) {
}

