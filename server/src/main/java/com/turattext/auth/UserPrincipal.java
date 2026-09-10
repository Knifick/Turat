package com.turattext.auth;

import java.util.UUID;

public record UserPrincipal(UUID id, String login) {
}

