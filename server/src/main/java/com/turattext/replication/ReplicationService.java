package com.turattext.replication;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.turattext.common.UnauthorizedException;
import com.turattext.replication.dto.ReplicationEnvelope;
import com.turattext.replication.dto.ReplicationPayload;
import com.turattext.servers.ServerNode;
import com.turattext.servers.ServerNodeRepository;
import org.springframework.stereotype.Service;

import java.time.Duration;
import java.time.Instant;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

@Service
public class ReplicationService {
    private static final Duration MAX_CLOCK_SKEW = Duration.ofMinutes(2);
    private static final int MAX_PLAINTEXT_BYTES = 64 * 1024 * 1024;

    private final ServerNodeRepository servers;
    private final ReplicationSnapshotService snapshots;
    private final ReplicationCrypto crypto;
    private final ObjectMapper objectMapper;
    private final Map<String, Instant> seenNonces = new ConcurrentHashMap<>();

    public ReplicationService(
            ServerNodeRepository servers,
            ReplicationSnapshotService snapshots,
            ReplicationCrypto crypto,
            ObjectMapper objectMapper
    ) {
        this.servers = servers;
        this.snapshots = snapshots;
        this.crypto = crypto;
        this.objectMapper = objectMapper;
    }

    public ReplicationEnvelope synchronize(ReplicationEnvelope request) {
        if (request.serverId() == null || request.nonce() == null || request.ciphertext() == null) {
            throw new UnauthorizedException("Invalid replication request");
        }
        String replayKey = request.serverId() + ":" + request.nonce();
        cleanupNonces();
        if (seenNonces.putIfAbsent(replayKey, Instant.now()) != null) {
            throw new UnauthorizedException("Replication request was already used");
        }

        ServerNode node = servers.findById(request.serverId())
                .orElseThrow(() -> new UnauthorizedException("Replication node is not registered"));
        if (node.getReplicationKey() == null || node.getReplicationKey().isBlank()) {
            throw new UnauthorizedException("Replication is not configured for this node");
        }
        try {
            byte[] plaintext = crypto.decrypt(request, node.getReplicationKey());
            if (plaintext.length > MAX_PLAINTEXT_BYTES) {
                throw new UnauthorizedException("Replication snapshot is too large");
            }
            ReplicationPayload payload = objectMapper.readValue(plaintext, ReplicationPayload.class);
            requireFresh(payload.sentAt());
            snapshots.mergeFromPeer(payload.snapshot());
            ReplicationPayload response = new ReplicationPayload(Instant.now(), snapshots.exportSnapshot());
            return crypto.encrypt(
                    request.serverId(),
                    node.getReplicationKey(),
                    objectMapper.writeValueAsBytes(response)
            );
        } catch (UnauthorizedException exception) {
            throw exception;
        } catch (Exception exception) {
            throw new IllegalStateException("Replication synchronization failed", exception);
        }
    }

    private void requireFresh(Instant sentAt) {
        if (sentAt == null || Duration.between(sentAt, Instant.now()).abs().compareTo(MAX_CLOCK_SKEW) > 0) {
            throw new UnauthorizedException("Replication request timestamp is outside the allowed window");
        }
    }

    private void cleanupNonces() {
        Instant expired = Instant.now().minus(MAX_CLOCK_SKEW);
        seenNonces.entrySet().removeIf(entry -> entry.getValue().isBefore(expired));
    }
}
