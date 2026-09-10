package com.turattext.users.dto;

import jakarta.validation.constraints.Size;

public record UpdateProfileRequest(
        @Size(max = 100) String displayName,
        @Size(max = 500) String avatarUrl,
        @Size(max = 100) String status,
        @Size(max = 1000) String description
) {
}

