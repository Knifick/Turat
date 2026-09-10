package com.turattext.admin;

import com.turattext.admin.dto.AdminLoginRequest;
import com.turattext.admin.dto.AdminServerRequest;
import com.turattext.admin.dto.AdminUserResponse;
import com.turattext.admin.dto.AdminUserUpdateRequest;
import com.turattext.admin.dto.BackupResponse;
import com.turattext.servers.dto.ServerResponse;
import jakarta.validation.Valid;
import org.springframework.web.bind.annotation.DeleteMapping;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PatchMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestHeader;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.ResponseStatus;
import org.springframework.web.bind.annotation.RestController;

import java.io.IOException;
import java.util.List;
import java.util.UUID;

import static org.springframework.http.HttpStatus.NO_CONTENT;

@RestController
@RequestMapping("/api/admin")
public class AdminController {
    private final AdminAuthService authService;
    private final AdminService adminService;
    private final BackupService backupService;

    public AdminController(
            AdminAuthService authService,
            AdminService adminService,
            BackupService backupService
    ) {
        this.authService = authService;
        this.adminService = adminService;
        this.backupService = backupService;
    }

    @PostMapping("/login")
    AdminLoginResponse login(@Valid @RequestBody AdminLoginRequest request) {
        return authService.login(request.password());
    }

    @GetMapping("/servers")
    List<ServerResponse> servers(@RequestHeader("X-Admin-Token") String token) {
        authService.requireAdmin(token);
        return adminService.listServers();
    }

    @PostMapping("/servers")
    ServerResponse saveServer(
            @RequestHeader("X-Admin-Token") String token,
            @Valid @RequestBody AdminServerRequest request
    ) {
        authService.requireAdmin(token);
        return adminService.saveServer(request);
    }

    @PostMapping("/servers/{serverId}/primary")
    ServerResponse makePrimary(
            @RequestHeader("X-Admin-Token") String token,
            @PathVariable UUID serverId
    ) {
        authService.requireAdmin(token);
        return adminService.makePrimary(serverId);
    }

    @DeleteMapping("/servers/{serverId}")
    @ResponseStatus(NO_CONTENT)
    void deleteServer(
            @RequestHeader("X-Admin-Token") String token,
            @PathVariable UUID serverId
    ) {
        authService.requireAdmin(token);
        adminService.deleteServer(serverId);
    }

    @GetMapping("/users")
    List<AdminUserResponse> users(@RequestHeader("X-Admin-Token") String token) {
        authService.requireAdmin(token);
        return adminService.listUsers();
    }

    @PatchMapping("/users/{userId}")
    AdminUserResponse updateUser(
            @RequestHeader("X-Admin-Token") String token,
            @PathVariable UUID userId,
            @RequestBody AdminUserUpdateRequest request
    ) {
        authService.requireAdmin(token);
        return adminService.updateUser(userId, request);
    }

    @GetMapping("/backups")
    List<BackupResponse> backups(@RequestHeader("X-Admin-Token") String token) throws IOException {
        authService.requireAdmin(token);
        return backupService.listBackups();
    }

    @PostMapping("/backups")
    BackupResponse createBackup(@RequestHeader("X-Admin-Token") String token) throws IOException {
        authService.requireAdmin(token);
        return backupService.createBackup();
    }
}
