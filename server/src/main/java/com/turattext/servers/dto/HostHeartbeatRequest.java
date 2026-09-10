package com.turattext.servers.dto;

import jakarta.validation.constraints.NotBlank;
import jakarta.validation.constraints.Size;

public record HostHeartbeatRequest(
        @NotBlank @Size(max = 500) String baseUrl
) {
}

