package com.turattext.users.dto;

import jakarta.validation.constraints.NotBlank;
import jakarta.validation.constraints.Size;

public record PublicKeyRequest(
        @NotBlank @Size(max = 120) String keyId,
        @NotBlank @Size(max = 80) String algorithm,
        @NotBlank String publicKey
) {
}

