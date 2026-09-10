package com.turattext.replication;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.turattext.common.UnauthorizedException;
import com.turattext.replication.dto.ReplicationConfigureRequest;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.stereotype.Component;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.MessageDigest;

@Component
public class ReplicationConfiguration {
    private final ObjectMapper objectMapper;
    private final String localControlSecret;
    private final Path configFile;
    private volatile NodeConfig current;

    public ReplicationConfiguration(
            ObjectMapper objectMapper,
            @Value("${turattext.replication.local-control-secret:}") String localControlSecret,
            @Value("${turattext.replication.config-file:./data/replication-config.json}") String configFile
    ) {
        this.objectMapper = objectMapper;
        this.localControlSecret = localControlSecret;
        this.configFile = Path.of(configFile).toAbsolutePath().normalize();
        this.current = load();
    }

    public NodeConfig current() {
        return current;
    }

    public synchronized NodeConfig configure(String suppliedSecret, ReplicationConfigureRequest request) {
        if (localControlSecret == null || localControlSecret.isBlank()
                || suppliedSecret == null
                || !MessageDigest.isEqual(
                        localControlSecret.getBytes(StandardCharsets.UTF_8),
                        suppliedSecret.getBytes(StandardCharsets.UTF_8))) {
            throw new UnauthorizedException("Local host control secret is invalid");
        }
        NodeConfig configured = new NodeConfig(
                request.serverId(),
                request.serverName(),
                request.baseUrl().replaceAll("/+$", ""),
                request.upstreamUrl().replaceAll("/+$", ""),
                request.replicationKey()
        );
        try {
            Files.createDirectories(configFile.getParent());
            Path temporary = configFile.resolveSibling(configFile.getFileName() + ".new");
            objectMapper.writeValue(temporary.toFile(), configured);
            Files.move(temporary, configFile, java.nio.file.StandardCopyOption.REPLACE_EXISTING);
        } catch (Exception exception) {
            throw new IllegalStateException("Could not save replication configuration", exception);
        }
        current = configured;
        return configured;
    }

    private NodeConfig load() {
        if (!Files.isRegularFile(configFile)) return null;
        try {
            return objectMapper.readValue(configFile.toFile(), NodeConfig.class);
        } catch (Exception ignored) {
            return null;
        }
    }

    public record NodeConfig(
            java.util.UUID serverId,
            String serverName,
            String baseUrl,
            String upstreamUrl,
            String replicationKey
    ) {
    }
}
