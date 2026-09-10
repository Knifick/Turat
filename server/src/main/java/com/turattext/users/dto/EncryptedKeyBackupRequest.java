package com.turattext.users.dto;

import jakarta.validation.constraints.NotBlank;
import jakarta.validation.constraints.Size;

public record EncryptedKeyBackupRequest(
        @NotBlank @Size(max = 120) String keyId,
        @NotBlank @Size(max = 80) String algorithm,
        @NotBlank String publicKey,
        @NotBlank @Size(max = 200) String salt,
        @NotBlank @Size(max = 200) String nonce,
        @NotBlank @Size(max = 120) String kdf,
        @NotBlank String encryptedPrivateKey
) {
}
