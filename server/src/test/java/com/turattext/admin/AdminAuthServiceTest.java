package com.turattext.admin;

import com.turattext.common.UnauthorizedException;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.nio.file.Files;
import java.nio.file.Path;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertThrows;

class AdminAuthServiceTest {
    @TempDir
    Path temporaryDirectory;

    @Test
    void rotatingPasswordInvalidatesExistingSessions() throws Exception {
        Path passwordFile = temporaryDirectory.resolve("admin-password");
        Files.writeString(passwordFile, "first-password\n");
        AdminAuthService service = new AdminAuthService("", passwordFile.toString(), 60);

        String oldToken = service.login("first-password").token();
        assertDoesNotThrow(() -> service.requireAdmin(oldToken));

        Files.writeString(passwordFile, "second-password\n");

        assertThrows(UnauthorizedException.class, () -> service.requireAdmin(oldToken));
        assertThrows(UnauthorizedException.class, () -> service.login("first-password"));
        String newToken = service.login("second-password").token();
        assertDoesNotThrow(() -> service.requireAdmin(newToken));
    }
}
