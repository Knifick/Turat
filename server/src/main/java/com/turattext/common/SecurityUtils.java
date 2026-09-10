package com.turattext.common;

import com.turattext.auth.UserPrincipal;
import org.springframework.security.core.context.SecurityContextHolder;

import java.util.UUID;

public final class SecurityUtils {
    private SecurityUtils() {
    }

    public static UUID currentUserId() {
        Object principal = SecurityContextHolder.getContext().getAuthentication().getPrincipal();
        if (principal instanceof UserPrincipal userPrincipal) {
            return userPrincipal.id();
        }
        throw new UnauthorizedException("Authentication is required");
    }
}

