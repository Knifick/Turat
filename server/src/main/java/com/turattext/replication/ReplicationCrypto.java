package com.turattext.replication;

import com.turattext.common.UnauthorizedException;
import com.turattext.replication.dto.ReplicationEnvelope;
import org.springframework.stereotype.Component;

import javax.crypto.AEADBadTagException;
import javax.crypto.Cipher;
import javax.crypto.spec.GCMParameterSpec;
import javax.crypto.spec.SecretKeySpec;
import java.nio.charset.StandardCharsets;
import java.security.SecureRandom;
import java.util.Base64;
import java.util.UUID;

@Component
public class ReplicationCrypto {
    private static final int NONCE_BYTES = 12;
    private static final int TAG_BITS = 128;
    private final SecureRandom random = new SecureRandom();

    public ReplicationEnvelope encrypt(UUID serverId, String replicationKey, byte[] plaintext) {
        try {
            byte[] nonce = new byte[NONCE_BYTES];
            random.nextBytes(nonce);
            Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
            cipher.init(Cipher.ENCRYPT_MODE, key(replicationKey), new GCMParameterSpec(TAG_BITS, nonce));
            cipher.updateAAD(serverId.toString().getBytes(StandardCharsets.UTF_8));
            byte[] encrypted = cipher.doFinal(plaintext);
            return new ReplicationEnvelope(
                    serverId,
                    Base64.getUrlEncoder().withoutPadding().encodeToString(nonce),
                    Base64.getUrlEncoder().withoutPadding().encodeToString(encrypted)
            );
        } catch (Exception exception) {
            throw new IllegalStateException("Could not encrypt replication snapshot", exception);
        }
    }

    public byte[] decrypt(ReplicationEnvelope envelope, String replicationKey) {
        try {
            byte[] nonce = Base64.getUrlDecoder().decode(envelope.nonce());
            if (nonce.length != NONCE_BYTES) {
                throw new UnauthorizedException("Invalid replication nonce");
            }
            Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
            cipher.init(Cipher.DECRYPT_MODE, key(replicationKey), new GCMParameterSpec(TAG_BITS, nonce));
            cipher.updateAAD(envelope.serverId().toString().getBytes(StandardCharsets.UTF_8));
            return cipher.doFinal(Base64.getUrlDecoder().decode(envelope.ciphertext()));
        } catch (AEADBadTagException exception) {
            throw new UnauthorizedException("Invalid replication credentials");
        } catch (UnauthorizedException exception) {
            throw exception;
        } catch (Exception exception) {
            throw new UnauthorizedException("Invalid replication envelope");
        }
    }

    private SecretKeySpec key(String replicationKey) {
        byte[] bytes;
        try {
            bytes = Base64.getUrlDecoder().decode(replicationKey);
        } catch (IllegalArgumentException exception) {
            throw new UnauthorizedException("Invalid replication key");
        }
        if (bytes.length != 32) {
            throw new UnauthorizedException("Invalid replication key");
        }
        return new SecretKeySpec(bytes, "AES");
    }
}
