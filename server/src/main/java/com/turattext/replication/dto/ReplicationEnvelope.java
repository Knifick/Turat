package com.turattext.replication.dto;

import java.util.UUID;

public record ReplicationEnvelope(
        UUID serverId,
        String nonce,
        String ciphertext
) {
}
