package com.turattext.admin.dto;

public record AdminUserUpdateRequest(
        Boolean enabled,
        String displayName,
        String status
) {
}
