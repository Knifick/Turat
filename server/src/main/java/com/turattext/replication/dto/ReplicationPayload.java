package com.turattext.replication.dto;

import java.time.Instant;

public record ReplicationPayload(
        Instant sentAt,
        ReplicationSnapshot snapshot
) {
}
