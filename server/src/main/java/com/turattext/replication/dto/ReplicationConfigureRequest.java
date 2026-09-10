package com.turattext.replication.dto;

import jakarta.validation.constraints.NotBlank;
import jakarta.validation.constraints.NotNull;

import java.util.UUID;

public record ReplicationConfigureRequest(
        @NotNull UUID serverId,
        @NotBlank String serverName,
        @NotBlank String baseUrl,
        @NotBlank String upstreamUrl,
        @NotBlank String replicationKey
) {
}
