package com.turattext.servers;

import com.turattext.common.BadRequestException;
import com.turattext.servers.dto.BecomeHostRequest;
import com.turattext.servers.dto.HealthResponse;
import com.turattext.servers.dto.HostHeartbeatRequest;
import com.turattext.servers.dto.HostRegistrationResponse;
import com.turattext.servers.dto.ServerResponse;
import com.turattext.servers.dto.SyncStatusResponse;
import org.springframework.stereotype.Service;
import org.springframework.transaction.annotation.Transactional;

import java.net.URI;
import java.net.URISyntaxException;
import java.time.Duration;
import java.time.Instant;
import java.util.Comparator;
import java.util.List;
import java.util.Optional;
import java.util.UUID;
import java.security.SecureRandom;
import java.util.Base64;

@Service
public class ServerService {
    private static final Duration PRIMARY_STALE_AFTER = Duration.ofSeconds(90);
    private static final SecureRandom SECURE_RANDOM = new SecureRandom();

    private final ServerNodeRepository servers;
    private final ServerSyncStateRepository syncStates;

    public ServerService(
            ServerNodeRepository servers,
            ServerSyncStateRepository syncStates
    ) {
        this.servers = servers;
        this.syncStates = syncStates;
    }

    @Transactional
    public List<ServerResponse> listServers() {
        reconcileLeadership();
        return servers.findAll().stream()
                .sorted(Comparator
                        .comparing((ServerNode server) -> server.getRole() == ServerRole.PRIMARY ? 0 : 1)
                        .thenComparing(ServerNode::getCreatedAt))
                .map(this::toResponse)
                .toList();
    }

    public HealthResponse health() {
        return new HealthResponse("online", Instant.now().toString(), Instant.now());
    }

    @Transactional
    public HostRegistrationResponse becomeHost(UUID ownerUserId, BecomeHostRequest request) {
        reconcileLeadership();
        validatePublicBaseUrl(request.baseUrl());

        ServerNode server = servers.findByBaseUrl(request.baseUrl()).orElseGet(ServerNode::new);
        boolean existingServer = server.getId() != null;
        if (!existingServer) {
            server.setId(request.serverId() == null ? UUID.randomUUID() : request.serverId());
        }
        server.setName(request.name());
        server.setBaseUrl(request.baseUrl());
        server.setPublicKey(request.publicKey());
        server.setRole(hasOnlinePrimaryExcluding(request.baseUrl()) ? ServerRole.MIRROR : ServerRole.PRIMARY);
        server.setStatus(ServerStatus.ONLINE);
        server.setLastSeenAt(Instant.now());
        if (ownerUserId != null) {
            server.setOwnerUserId(ownerUserId);
        }
        if (server.getReplicationKey() == null || server.getReplicationKey().isBlank()) {
            byte[] key = new byte[32];
            SECURE_RANDOM.nextBytes(key);
            server.setReplicationKey(Base64.getUrlEncoder().withoutPadding().encodeToString(key));
        }
        ServerNode saved = servers.save(server);

        syncStates.findById(saved.getId()).ifPresent(state -> {
            state.setLastSyncAt(Instant.now());
            state.setStatus(saved.getRole() == ServerRole.PRIMARY ? SyncStatus.IDLE : SyncStatus.SYNCING);
            state.setErrorMessage(null);
            syncStates.save(state);
        });
        String primaryUrl = servers.findAll().stream()
                .filter(node -> node.getRole() == ServerRole.PRIMARY && node.getStatus() == ServerStatus.ONLINE)
                .map(ServerNode::getBaseUrl)
                .findFirst()
                .orElse(saved.getBaseUrl());
        return new HostRegistrationResponse(
                saved.getId(),
                saved.getName(),
                saved.getBaseUrl(),
                saved.getRole(),
                saved.getStatus(),
                saved.getLastSeenAt(),
                saved.getReplicationKey(),
                primaryUrl
        );
    }

    public HostRegistrationResponse becomeHost(BecomeHostRequest request) {
        return becomeHost(null, request);
    }

    @Transactional
    public ServerResponse heartbeat(HostHeartbeatRequest request) {
        ServerNode server = servers.findByBaseUrl(request.baseUrl())
                .orElseThrow(() -> new BadRequestException("Host is not registered"));
        server.setStatus(ServerStatus.ONLINE);
        server.setLastSeenAt(Instant.now());
        ServerNode saved = servers.save(server);
        reconcileLeadership();
        return toResponse(saved);
    }

    public List<SyncStatusResponse> syncStatus() {
        return syncStates.findAll().stream()
                .map(state -> new SyncStatusResponse(
                        state.getServer().getId(),
                        state.getServer().getName(),
                        state.getLastEventId(),
                        state.getLastSyncAt(),
                        state.getStatus(),
                        state.getErrorMessage()
                ))
                .toList();
    }

    @Transactional
    public void promoteLocal(UUID serverId) {
        List<ServerNode> all = servers.findAll();
        for (ServerNode server : all) {
            if (server.getId().equals(serverId)) {
                server.setRole(ServerRole.PRIMARY);
                server.setStatus(ServerStatus.ONLINE);
                server.setLastSeenAt(Instant.now());
            } else if (server.getRole() == ServerRole.PRIMARY) {
                server.setRole(ServerRole.MIRROR);
            }
        }
        servers.saveAll(all);
    }

    private void reconcileLeadership() {
        Instant staleBefore = Instant.now().minus(PRIMARY_STALE_AFTER);
        List<ServerNode> allServers = servers.findAll();

        for (ServerNode server : allServers) {
            if (server.getLastSeenAt() != null && server.getLastSeenAt().isBefore(staleBefore)) {
                server.setStatus(ServerStatus.OFFLINE);
            }
        }

        Optional<ServerNode> activePrimary = allServers.stream()
                .filter(server -> server.getRole() == ServerRole.PRIMARY)
                .filter(server -> server.getStatus() == ServerStatus.ONLINE)
                .findFirst();

        if (activePrimary.isPresent()) {
            demoteExtraPrimaries(activePrimary.get(), allServers);
            servers.saveAll(allServers);
            return;
        }

        allServers.stream()
                .filter(server -> server.getStatus() == ServerStatus.ONLINE)
                .min(Comparator.comparing(ServerNode::getCreatedAt))
                .ifPresent(newPrimary -> {
                    for (ServerNode server : allServers) {
                        server.setRole(server == newPrimary ? ServerRole.PRIMARY : ServerRole.MIRROR);
                    }
                });

        servers.saveAll(allServers);
    }

    private void demoteExtraPrimaries(ServerNode activePrimary, List<ServerNode> allServers) {
        for (ServerNode server : allServers) {
            if (server.getRole() == ServerRole.PRIMARY && !server.getId().equals(activePrimary.getId())) {
                server.setRole(ServerRole.MIRROR);
            }
        }
    }

    private boolean hasOnlinePrimaryExcluding(String baseUrl) {
        return servers.findAll().stream()
                .anyMatch(server -> server.getRole() == ServerRole.PRIMARY
                        && server.getStatus() == ServerStatus.ONLINE
                        && !server.getBaseUrl().equals(baseUrl));
    }

    private void validatePublicBaseUrl(String baseUrl) {
        URI uri;
        try {
            uri = new URI(baseUrl);
        } catch (URISyntaxException ex) {
            throw new BadRequestException("Host URL is invalid");
        }

        String scheme = uri.getScheme();
        if (scheme == null
                || (!scheme.equalsIgnoreCase("http") && !scheme.equalsIgnoreCase("https"))
                || uri.getHost() == null) {
            throw new BadRequestException("Host URL must be an absolute http or https URL");
        }

        String host = uri.getHost();
        if (host.equalsIgnoreCase("localhost")
                || host.equals("127.0.0.1")
                || host.equals("0.0.0.0")
                || host.equals("::1")) {
            throw new BadRequestException("Host URL must be reachable by peers. localhost is only reachable from the host owner's computer");
        }
    }

    private ServerResponse toResponse(ServerNode server) {
        return new ServerResponse(
                server.getId(),
                server.getName(),
                server.getBaseUrl(),
                server.getRole(),
                server.getStatus(),
                server.getLastSeenAt()
        );
    }
}
