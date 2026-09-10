package com.turattext.auth.dto;

import jakarta.validation.constraints.NotBlank;
import jakarta.validation.constraints.Pattern;
import jakarta.validation.constraints.Size;

public record RegisterRequest(
        @NotBlank
        @Pattern(regexp = "^[a-zA-Z0-9_.-]{3,64}$")
        String login,

        @NotBlank
        @Size(min = 8, max = 200)
        String password,

        @Size(max = 100)
        String displayName,

        String publicKey
) {
}

