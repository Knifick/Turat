package com.turattext.replication;

import com.turattext.common.UnauthorizedException;
import org.junit.jupiter.api.Test;

import java.nio.charset.StandardCharsets;
import java.security.SecureRandom;
import java.util.Base64;
import java.util.UUID;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

class ReplicationCryptoTest {
    @Test
    void envelopeRoundTripsAndRejectsWrongKey() {
        ReplicationCrypto crypto = new ReplicationCrypto();
        UUID serverId = UUID.randomUUID();
        String key = randomKey();
        byte[] plaintext = "ciphertext-only snapshot".getBytes(StandardCharsets.UTF_8);

        var envelope = crypto.encrypt(serverId, key, plaintext);

        assertArrayEquals(plaintext, crypto.decrypt(envelope, key));
        assertThrows(UnauthorizedException.class, () -> crypto.decrypt(envelope, randomKey()));
    }

    private static String randomKey() {
        byte[] bytes = new byte[32];
        new SecureRandom().nextBytes(bytes);
        return Base64.getUrlEncoder().withoutPadding().encodeToString(bytes);
    }
}
