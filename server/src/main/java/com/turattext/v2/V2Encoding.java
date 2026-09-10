package com.turattext.v2;

import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.Base64;

public final class V2Encoding {
    private V2Encoding() {
    }

    public static String sha256Hex(byte[] value) {
        try {
            return java.util.HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(value));
        } catch (NoSuchAlgorithmException exception) {
            throw new IllegalStateException("SHA-256 is unavailable", exception);
        }
    }

    public static String capabilityHash(String capability) {
        if (capability == null || capability.length() < 32 || capability.length() > 256) {
            throw new IllegalArgumentException("Capability must contain 32-256 characters");
        }
        return sha256Hex(capability.getBytes(StandardCharsets.UTF_8));
    }

    public static String randomToken(int bytes) {
        byte[] value = new byte[bytes];
        new java.security.SecureRandom().nextBytes(value);
        return Base64.getUrlEncoder().withoutPadding().encodeToString(value);
    }
}
