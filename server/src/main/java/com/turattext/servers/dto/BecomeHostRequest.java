package com.turattext.servers.dto;

import jakarta.validation.constraints.NotBlank;
import jakarta.validation.constraints.Size;

import java.util.UUID;

public record BecomeHostRequest(
        UUID serverId,
        @NotBlank @Size(max = 120) String name,
        @NotBlank @Size(max = 500) String baseUrl,
        String publicKey
) {
}
