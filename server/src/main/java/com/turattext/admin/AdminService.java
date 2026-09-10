package com.turattext.admin;

import com.turattext.admin.dto.AdminServerRequest;
import com.turattext.admin.dto.AdminUserResponse;
import com.turattext.admin.dto.AdminUserUpdateRequest;
import com.turattext.common.NotFoundException;
import com.turattext.servers.ServerNode;
import com.turattext.servers.ServerNodeRepository;
import com.turattext.servers.ServerRole;
import com.turattext.servers.ServerStatus;
import com.turattext.servers.ServerSyncStateRepository;
import com.turattext.servers.dto.ServerResponse;
import com.turattext.users.UserAccount;
import com.turattext.users.UserAccountRepository;
import com.turattext.users.UserProfile;
import org.springframework.stereotype.Service;
import org.springframework.transaction.annotation.Transactional;

import java.time.Instant;
import java.util.Comparator;
import java.util.List;
import java.util.UUID;

@Service
public class AdminService {
    private final ServerNodeRepository servers;
    private final ServerSyncStateRepository syncStates;
    private final UserAccountRepository users;

    public AdminService(
            ServerNodeRepository servers,
            ServerSyncStateRepository syncStates,
            UserAccountRepository users
    ) {
        this.servers = servers;
        this.syncStates = syncStates;
        this.users = users;
    }

    public List<ServerResponse> listServers() {
        return servers.findAll().stream()
                .sorted(Comparator.comparing(ServerNode::getCreatedAt))
                .map(this::toServerResponse)
                .toList();
    }

    @Transactional
    public ServerResponse saveServer(AdminServerRequest request) {
        ServerNode server = servers.findByBaseUrl(request.baseUrl()).orElseGet(ServerNode::new);
        server.setName(request.name());
        server.setBaseUrl(request.baseUrl());
        server.setPublicKey(request.publicKey());
        server.setRole(request.role());
        server.setStatus(request.status());
        if (request.status() == ServerStatus.ONLINE) {
            server.setLastSeenAt(Instant.now());
        }

        ServerNode saved = servers.save(server);
        if (request.role() == ServerRole.PRIMARY) {
            demoteOtherPrimaries(saved.getId());
        }
        return toServerResponse(saved);
    }

    @Transactional
    public ServerResponse makePrimary(UUID serverId) {
        ServerNode selected = servers.findById(serverId)
                .orElseThrow(() -> new NotFoundException("Server not found"));
        selected.setRole(ServerRole.PRIMARY);
        selected.setStatus(ServerStatus.ONLINE);
        selected.setLastSeenAt(Instant.now());
        ServerNode saved = servers.save(selected);
        demoteOtherPrimaries(saved.getId());
        return toServerResponse(saved);
    }

    @Transactional
    public void deleteServer(UUID serverId) {
        if (!servers.existsById(serverId)) {
            throw new NotFoundException("Server not found");
        }
        syncStates.deleteById(serverId);
        servers.deleteById(serverId);
    }

    public List<AdminUserResponse> listUsers() {
        return users.findAll().stream()
                .sorted(Comparator.comparing(UserAccount::getCreatedAt).reversed())
                .map(this::toUserResponse)
                .toList();
    }

    @Transactional
    public AdminUserResponse updateUser(UUID userId, AdminUserUpdateRequest request) {
        UserAccount user = users.findById(userId)
                .orElseThrow(() -> new NotFoundException("User not found"));
        if (request.enabled() != null) {
            user.setEnabled(request.enabled());
        }
        if (user.getProfile() != null) {
            if (request.displayName() != null && !request.displayName().isBlank()) {
                user.getProfile().setDisplayName(request.displayName());
            }
            if (request.status() != null) {
                user.getProfile().setStatus(request.status());
            }
        }
        return toUserResponse(users.save(user));
    }

    private void demoteOtherPrimaries(UUID primaryId) {
        List<ServerNode> all = servers.findAll();
        for (ServerNode server : all) {
            if (!server.getId().equals(primaryId) && server.getRole() == ServerRole.PRIMARY) {
                server.setRole(ServerRole.MIRROR);
            }
        }
        servers.saveAll(all);
    }

    private ServerResponse toServerResponse(ServerNode server) {
        return new ServerResponse(
                server.getId(),
                server.getName(),
                server.getBaseUrl(),
                server.getRole(),
                server.getStatus(),
                server.getLastSeenAt()
        );
    }

    private AdminUserResponse toUserResponse(UserAccount user) {
        UserProfile profile = user.getProfile();
        return new AdminUserResponse(
                user.getId(),
                user.getLogin(),
                profile == null ? user.getLogin() : profile.getDisplayName(),
                profile == null ? null : profile.getStatus(),
                user.isEnabled(),
                user.getCreatedAt()
        );
    }
}
