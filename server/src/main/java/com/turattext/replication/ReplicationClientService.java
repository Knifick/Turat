package com.turattext.replication;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.turattext.replication.ReplicationConfiguration.NodeConfig;
import com.turattext.replication.dto.ReplicationEnvelope;
import com.turattext.replication.dto.ReplicationPayload;
import com.turattext.servers.ServerNode;
import com.turattext.servers.ServerNodeRepository;
import com.turattext.servers.ServerService;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.scheduling.annotation.Async;
import org.springframework.scheduling.annotation.Scheduled;
import org.springframework.stereotype.Service;

import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.time.Duration;
import java.time.Instant;
import java.util.Comparator;
import java.util.List;
import java.util.concurrent.atomic.AtomicBoolean;

@Service
public class ReplicationClientService {
    private static final Logger log = LoggerFactory.getLogger(ReplicationClientService.class);

    private final ReplicationConfiguration configuration;
    private final ReplicationSnapshotService snapshots;
    private final ReplicationCrypto crypto;
    private final ObjectMapper objectMapper;
    private final ServerNodeRepository servers;
    private final ServerService serverService;
    private final HttpClient httpClient;
    private final Duration failoverAfter;
    private final AtomicBoolean synchronizing = new AtomicBoolean();
    private volatile Instant lastSuccessAt;
    private volatile Instant firstFailureAt;
    private volatile String state = "NOT_CONFIGURED";

    public ReplicationClientService(
            ReplicationConfiguration configuration,
            ReplicationSnapshotService snapshots,
            ReplicationCrypto crypto,
            ObjectMapper objectMapper,
            ServerNodeRepository servers,
            ServerService serverService,
            @Value("${turattext.replication.failover-ms:90000}") long failoverMs
    ) {
        this.configuration = configuration;
        this.snapshots = snapshots;
        this.crypto = crypto;
        this.objectMapper = objectMapper;
        this.servers = servers;
        this.serverService = serverService;
        this.failoverAfter = Duration.ofMillis(Math.max(10_000, failoverMs));
        this.httpClient = HttpClient.newBuilder()
                .connectTimeout(Duration.ofSeconds(5))
                .build();
    }

    @Scheduled(
            fixedDelayString = "${turattext.replication.interval-ms:10000}",
            initialDelayString = "${turattext.replication.interval-ms:10000}"
    )
    public void scheduledSync() {
        synchronize();
    }

    @Async
    public void synchronizeSoon() {
        synchronize();
    }

    public String state() {
        return state;
    }

    public Instant lastSuccessAt() {
        return lastSuccessAt;
    }

    private void synchronize() {
        NodeConfig config = configuration.current();
        if (config == null) {
            state = "NOT_CONFIGURED";
            return;
        }
        if (!synchronizing.compareAndSet(false, true)) return;
        try {
            state = "SYNCING";
            syncAgainst(config.upstreamUrl(), config);
            firstFailureAt = null;
            lastSuccessAt = Instant.now();
            state = "IN_SYNC";
        } catch (Exception exception) {
            if (firstFailureAt == null) firstFailureAt = Instant.now();
            state = "UPSTREAM_UNAVAILABLE";
            log.warn("Replication with {} failed: {}", config.upstreamUrl(), exception.getMessage());
            if (Duration.between(firstFailureAt, Instant.now()).compareTo(failoverAfter) >= 0) {
                attemptFailover(config);
            }
        } finally {
            synchronizing.set(false);
        }
    }

    private void syncAgainst(String baseUrl, NodeConfig config) throws Exception {
        ReplicationPayload payload = new ReplicationPayload(Instant.now(), snapshots.exportSnapshot());
        ReplicationEnvelope envelope = crypto.encrypt(
                config.serverId(),
                config.replicationKey(),
                objectMapper.writeValueAsBytes(payload)
        );
        HttpRequest request = HttpRequest.newBuilder(
                        URI.create(baseUrl.replaceAll("/+$", "") + "/api/replication/sync"))
                .timeout(Duration.ofSeconds(45))
                .header("Content-Type", "application/json")
                .POST(HttpRequest.BodyPublishers.ofByteArray(objectMapper.writeValueAsBytes(envelope)))
                .build();
        HttpResponse<byte[]> response = httpClient.send(request, HttpResponse.BodyHandlers.ofByteArray());
        if (response.statusCode() / 100 != 2) {
            throw new IllegalStateException("Replication endpoint returned HTTP " + response.statusCode());
        }
        ReplicationEnvelope encryptedResponse = objectMapper.readValue(response.body(), ReplicationEnvelope.class);
        ReplicationPayload canonical = objectMapper.readValue(
                crypto.decrypt(encryptedResponse, config.replicationKey()),
                ReplicationPayload.class
        );
        if (canonical.sentAt() == null
                || Duration.between(canonical.sentAt(), Instant.now()).abs().compareTo(Duration.ofMinutes(2)) > 0) {
            throw new IllegalStateException("Replication response timestamp is invalid");
        }
        snapshots.applyCanonical(canonical.snapshot());
    }

    private void attemptFailover(NodeConfig config) {
        try {
            List<ServerNode> candidates = servers.findAll().stream()
                    .filter(server -> server.getReplicationKey() != null)
                    .sorted(Comparator.comparing(ServerNode::getCreatedAt))
                    .toList();
            for (ServerNode candidate : candidates) {
                if (candidate.getId().equals(config.serverId())) {
                    serverService.promoteLocal(config.serverId());
                    state = "PRIMARY_FAILOVER";
                    return;
                }
                if (candidate.getBaseUrl().equalsIgnoreCase(config.upstreamUrl())) continue;
                if (!isHealthy(candidate.getBaseUrl())) continue;
                try {
                    syncAgainst(candidate.getBaseUrl(), config);
                    lastSuccessAt = Instant.now();
                    state = "SYNCING_WITH_MIRROR";
                    return;
                } catch (Exception ignored) {
                }
            }
            serverService.promoteLocal(config.serverId());
            state = "PRIMARY_FAILOVER";
        } catch (Exception exception) {
            state = "FAILOVER_ERROR";
            log.warn("Replication failover failed: {}", exception.getMessage());
        }
    }

    private boolean isHealthy(String baseUrl) {
        try {
            HttpRequest request = HttpRequest.newBuilder(
                            URI.create(baseUrl.replaceAll("/+$", "") + "/api/servers/health"))
                    .timeout(Duration.ofSeconds(3))
                    .GET()
                    .build();
            return httpClient.send(request, HttpResponse.BodyHandlers.discarding()).statusCode() / 100 == 2;
        } catch (Exception exception) {
            return false;
        }
    }
}
