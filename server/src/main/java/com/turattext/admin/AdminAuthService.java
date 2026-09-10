package com.turattext.admin;

import com.turattext.common.UnauthorizedException;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.stereotype.Service;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.MessageDigest;
import java.security.SecureRandom;
import java.time.Duration;
import java.time.Instant;
import java.util.Base64;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

@Service
public class AdminAuthService {
    private final SecureRandom secureRandom = new SecureRandom();
    private final Map<String, AdminSession> sessions = new ConcurrentHashMap<>();
    private final String configuredPassword;
    private final Path passwordFile;
    private final Duration tokenTtl;

    public AdminAuthService(
            @Value("${turattext.admin.password:}") String adminPassword,
            @Value("${turattext.admin.password-file:}") String adminPasswordFile,
            @Value("${turattext.admin.token-ttl-minutes:60}") long tokenTtlMinutes
    ) {
        this.configuredPassword = adminPassword;
        this.passwordFile = adminPasswordFile == null || adminPasswordFile.isBlank()
                ? null
                : Path.of(adminPasswordFile);
        this.tokenTtl = Duration.ofMinutes(tokenTtlMinutes);
    }

    public AdminLoginResponse login(String password) {
        CurrentPassword current = currentPassword();
        byte[] supplied = password == null ? new byte[0] : password.getBytes(StandardCharsets.UTF_8);
        if (!MessageDigest.isEqual(current.value(), supplied)) {
            throw new UnauthorizedException("Invalid admin password");
        }

        byte[] tokenBytes = new byte[32];
        secureRandom.nextBytes(tokenBytes);
        String token = Base64.getUrlEncoder().withoutPadding().encodeToString(tokenBytes);
        Instant expiresAt = Instant.now().plus(tokenTtl);
        sessions.put(token, new AdminSession(expiresAt, current.digest()));
        return new AdminLoginResponse(token, expiresAt);
    }

    public void requireAdmin(String token) {
        if (token == null || token.isBlank()) {
            throw new UnauthorizedException("Admin token is required");
        }

        AdminSession session = sessions.get(token);
        if (session == null
                || session.expiresAt().isBefore(Instant.now())
                || !MessageDigest.isEqual(session.passwordDigest(), currentPassword().digest())) {
            sessions.remove(token);
            throw new UnauthorizedException("Admin session expired");
        }
    }

    private CurrentPassword currentPassword() {
        String password = configuredPassword;
        if (passwordFile != null) {
            try {
                password = Files.readString(passwordFile, StandardCharsets.UTF_8).strip();
            } catch (IOException exception) {
                throw new UnauthorizedException("Admin password is not available on this server");
            }
        }
        if (password == null || password.isBlank()) {
            throw new UnauthorizedException("Admin password is not configured on this server");
        }
        byte[] value = password.getBytes(StandardCharsets.UTF_8);
        return new CurrentPassword(value, sha256(value));
    }

    private static byte[] sha256(byte[] value) {
        try {
            return MessageDigest.getInstance("SHA-256").digest(value);
        } catch (java.security.NoSuchAlgorithmException exception) {
            throw new IllegalStateException("SHA-256 is unavailable", exception);
        }
    }

    private record CurrentPassword(byte[] value, byte[] digest) {
    }

    private record AdminSession(Instant expiresAt, byte[] passwordDigest) {
    }
}
