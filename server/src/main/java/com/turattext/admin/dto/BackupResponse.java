package com.turattext.admin.dto;

import java.time.Instant;

public record BackupResponse(
        String fileName,
        long sizeBytes,
        Instant createdAt
) {
}
