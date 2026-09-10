package com.turattext.users.dto;

import java.time.Instant;

public record EncryptedKeyBackupResponse(
        String keyId,
        String algorithm,
        String publicKey,
        String salt,
        String nonce,
        String kdf,
        String encryptedPrivateKey,
        Instant createdAt
) {
}
