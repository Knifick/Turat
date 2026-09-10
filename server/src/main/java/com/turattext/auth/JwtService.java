package com.turattext.auth;

import io.jsonwebtoken.Claims;
import io.jsonwebtoken.Jwts;
import io.jsonwebtoken.security.Keys;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.stereotype.Service;

import javax.crypto.SecretKey;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.time.Duration;
import java.time.Instant;
import java.util.Date;
import java.util.UUID;

@Service
public class JwtService {
    private final String secret;
    private final long accessTtlMinutes;
    private final long refreshTtlDays;

    public JwtService(
            @Value("${turattext.jwt.secret}") String secret,
            @Value("${turattext.jwt.access-ttl-minutes}") long accessTtlMinutes,
            @Value("${turattext.jwt.refresh-ttl-days}") long refreshTtlDays
    ) {
        this.secret = secret;
        this.accessTtlMinutes = accessTtlMinutes;
        this.refreshTtlDays = refreshTtlDays;
    }

    public String createAccessToken(UUID userId, String login) {
        return createToken(userId, login, Duration.ofMinutes(accessTtlMinutes), "access");
    }

    public String createRefreshToken(UUID userId, String login) {
        return createToken(userId, login, Duration.ofDays(refreshTtlDays), "refresh");
    }

    public UUID requireUserId(String token) {
        return UUID.fromString(claims(token).getSubject());
    }

    public String requireTokenType(String token) {
        return claims(token).get("typ", String.class);
    }

    private String createToken(UUID userId, String login, Duration ttl, String type) {
        Instant now = Instant.now();
        return Jwts.builder()
                .subject(userId.toString())
                .claim("login", login)
                .claim("typ", type)
                .issuedAt(Date.from(now))
                .expiration(Date.from(now.plus(ttl)))
                .signWith(signingKey())
                .compact();
    }

    private Claims claims(String token) {
        return Jwts.parser()
                .verifyWith(signingKey())
                .build()
                .parseSignedClaims(token)
                .getPayload();
    }

    private SecretKey signingKey() {
        try {
            byte[] digest = MessageDigest.getInstance("SHA-256")
                    .digest(secret.getBytes(StandardCharsets.UTF_8));
            return Keys.hmacShaKeyFor(digest);
        } catch (Exception ex) {
            throw new IllegalStateException("Cannot create JWT signing key", ex);
        }
    }
}

