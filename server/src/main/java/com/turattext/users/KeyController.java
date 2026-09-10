package com.turattext.users;

import com.turattext.common.SecurityUtils;
import com.turattext.users.dto.EncryptedKeyBackupRequest;
import com.turattext.users.dto.EncryptedKeyBackupResponse;
import com.turattext.users.dto.PublicKeyRequest;
import com.turattext.users.dto.PublicKeyResponse;
import jakarta.validation.Valid;
import org.springframework.http.HttpStatus;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.ResponseStatus;
import org.springframework.web.bind.annotation.RestController;

import java.util.UUID;

@RestController
@RequestMapping("/api/keys")
public class KeyController {
    private final UserService userService;

    public KeyController(UserService userService) {
        this.userService = userService;
    }

    @GetMapping("/public/{userId}")
    PublicKeyResponse publicKey(@PathVariable UUID userId) {
        return userService.getActivePublicKey(userId);
    }

    @PostMapping("/public")
    PublicKeyResponse addPublicKey(@Valid @RequestBody PublicKeyRequest request) {
        return userService.addPublicKey(SecurityUtils.currentUserId(), request);
    }

    @PostMapping("/backup/encrypted")
    @ResponseStatus(HttpStatus.NO_CONTENT)
    void saveEncryptedBackup(@Valid @RequestBody EncryptedKeyBackupRequest request) {
        userService.saveEncryptedBackup(SecurityUtils.currentUserId(), request);
    }

    @GetMapping("/backup/encrypted")
    EncryptedKeyBackupResponse encryptedBackup() {
        return userService.getEncryptedBackup(SecurityUtils.currentUserId());
    }
}
