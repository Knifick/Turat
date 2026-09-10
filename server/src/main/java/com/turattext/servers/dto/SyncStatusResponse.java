package com.turattext.servers.dto;

import com.turattext.servers.SyncStatus;

import java.time.Instant;
import java.util.UUID;

public record SyncStatusResponse(
        UUID serverId,
        String serverName,
        long lastEventId,
        Instant lastSyncAt,
        SyncStatus status,
        String errorMessage
) {
}

