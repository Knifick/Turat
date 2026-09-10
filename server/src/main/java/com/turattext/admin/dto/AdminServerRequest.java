package com.turattext.admin.dto;

import com.turattext.servers.ServerRole;
import com.turattext.servers.ServerStatus;
import jakarta.validation.constraints.NotBlank;
import jakarta.validation.constraints.NotNull;
import jakarta.validation.constraints.Size;

public record AdminServerRequest(
        @NotBlank @Size(max = 120) String name,
        @NotBlank @Size(max = 500) String baseUrl,
        String publicKey,
        @NotNull ServerRole role,
        @NotNull ServerStatus status
) {
}
