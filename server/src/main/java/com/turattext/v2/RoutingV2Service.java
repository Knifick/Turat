package com.turattext.v2;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.turattext.common.BadRequestException;
import com.turattext.common.NotFoundException;
import org.springframework.jdbc.core.JdbcTemplate;
import org.springframework.stereotype.Service;
import org.springframework.transaction.annotation.Transactional;

import java.nio.charset.StandardCharsets;
import java.security.KeyFactory;
import java.security.MessageDigest;
import java.security.Signature;
import java.security.spec.X509EncodedKeySpec;
import java.time.Instant;
import java.time.temporal.ChronoUnit;
import java.util.ArrayList;
import java.util.Base64;
import java.util.HexFormat;
import java.util.List;

import static com.turattext.v2.V2Jdbc.timestamp;

@Service
public class RoutingV2Service {
    private final JdbcTemplate jdbc;
    private final ObjectMapper objectMapper;
    private final NodeIdentityService nodeIdentity;

    public RoutingV2Service(JdbcTemplate jdbc, ObjectMapper objectMapper, NodeIdentityService nodeIdentity) {
        this.jdbc = jdbc;
        this.objectMapper = objectMapper;
        this.nodeIdentity = nodeIdentity;
    }

    @Transactional
    public RoutingRecord publish(PublishRouting request) {
        long deviceListSequence = validateAndVerify(request);
        Instant now = Instant.now();
        List<ExistingSequence> existing = jdbc.query(
                "select sequence_number, device_list_sequence from v2_routing_records where user_id = ?",
                (row, ignored) -> new ExistingSequence(row.getLong(1), row.getLong(2)), request.userId());
        if (!existing.isEmpty() && request.sequence() <= existing.getFirst().routingSequence()) {
            throw new BadRequestException("Routing sequence must increase");
        }
        if (!existing.isEmpty() && deviceListSequence < existing.getFirst().deviceListSequence()) {
            throw new BadRequestException("DeviceList rollback was rejected");
        }
        if (existing.isEmpty()) {
            jdbc.update("""
                    insert into v2_routing_records(
                        user_id, identity_public_key, sequence_number, descriptor_json,
                        signature, device_list_sequence, expires_at, updated_at)
                    values (?, ?, ?, ?, ?, ?, ?, ?)
                    """, request.userId(), request.identityPublicKey(), request.sequence(),
                    request.descriptorJson(), request.signature(), deviceListSequence, timestamp(request.expiresAt()), timestamp(now));
        } else {
            jdbc.update("""
                    update v2_routing_records
                    set identity_public_key = ?, sequence_number = ?, descriptor_json = ?,
                        signature = ?, device_list_sequence = ?, expires_at = ?, updated_at = ?
                    where user_id = ?
                    """, request.identityPublicKey(), request.sequence(), request.descriptorJson(),
                    request.signature(), deviceListSequence, timestamp(request.expiresAt()), timestamp(now), request.userId());
        }

        String payloadHash = V2Encoding.sha256Hex(request.descriptorJson().getBytes(StandardCharsets.UTF_8));
        String operationId = "op1-" + V2Encoding.randomToken(18);
        String operationSignature = nodeIdentity.sign(
                (operationId + "\nroute.update\n" + request.userId() + "\n" + payloadHash)
                        .getBytes(StandardCharsets.UTF_8));
        jdbc.update("""
                insert into v2_transparency_operations(
                    operation_id, operation_type, subject_id, payload_hash, payload, signature, created_at)
                values (?, ?, ?, ?, ?, ?, ?)
                """, operationId, "route.update", request.userId(), payloadHash,
                request.descriptorJson(), operationSignature, timestamp(now));
        return get(request.userId());
    }

    public RoutingRecord get(String userId) {
        List<RoutingRecord> records = jdbc.query("""
                select user_id, identity_public_key, sequence_number, descriptor_json,
                       signature, expires_at, updated_at
                from v2_routing_records
                where user_id = ? and expires_at > ?
                """, (row, ignored) -> new RoutingRecord(
                row.getString("user_id"), row.getString("identity_public_key"),
                row.getLong("sequence_number"), row.getString("descriptor_json"),
                row.getString("signature"), row.getTimestamp("expires_at").toInstant(),
                row.getTimestamp("updated_at").toInstant()), userId, timestamp(Instant.now()));
        if (records.isEmpty()) throw new NotFoundException("Routing record not found");
        return records.getFirst();
    }

    public List<TransparencyOperation> operations(long after, int requestedLimit) {
        int limit = Math.clamp(requestedLimit, 1, 1000);
        return jdbc.query("""
                select sequence_number, operation_id, operation_type, subject_id,
                       payload_hash, payload, signature, created_at
                from v2_transparency_operations
                where sequence_number > ?
                order by sequence_number
                limit ?
                """, (row, ignored) -> new TransparencyOperation(
                row.getLong("sequence_number"), row.getString("operation_id"),
                row.getString("operation_type"), row.getString("subject_id"),
                row.getString("payload_hash"), row.getString("payload"),
                row.getString("signature"), row.getTimestamp("created_at").toInstant()), after, limit);
    }

    public TransparencyCheckpoint checkpoint() {
        List<String> hashes = jdbc.query(
                "select payload_hash from v2_transparency_operations order by sequence_number",
                (row, ignored) -> row.getString(1));
        String root = merkleRoot(hashes);
        long size = hashes.size();
        long createdAt = Instant.now().toEpochMilli();
        String signature = nodeIdentity.sign(
                ("TuratText.TransparencyCheckpoint\n" + size + "\n" + root + "\n" + createdAt)
                        .getBytes(StandardCharsets.UTF_8));
        return new TransparencyCheckpoint(size, root, createdAt, signature, nodeIdentity.descriptor().nodeId());
    }

    private long validateAndVerify(PublishRouting request) {
        try {
            if (request.userId() == null || !request.userId().matches("tt1-[a-f0-9]{64}")) {
                throw new BadRequestException("Invalid UserID");
            }
            byte[] publicKeyBytes = Base64.getDecoder().decode(request.identityPublicKey());
            if (!("tt1-" + V2Encoding.sha256Hex(publicKeyBytes)).equals(request.userId())) {
                throw new BadRequestException("UserID does not match identity key");
            }
            if (request.sequence() <= 0 || request.descriptorJson() == null
                    || request.descriptorJson().length() > 256_000) {
                throw new BadRequestException("Invalid routing descriptor");
            }
            if (request.expiresAt() == null
                    || request.expiresAt().isBefore(Instant.now().plus(5, ChronoUnit.MINUTES))
                    || request.expiresAt().isAfter(Instant.now().plus(90, ChronoUnit.DAYS))) {
                throw new BadRequestException("Routing expiry is invalid");
            }
            JsonNode descriptor = objectMapper.readTree(request.descriptorJson());
            if (!request.userId().equals(descriptor.path("userId").asText())
                    || request.sequence() != descriptor.path("sequence").asLong()
                    || request.expiresAt().toEpochMilli() != descriptor.path("expiresAtUnixMilliseconds").asLong()) {
                throw new BadRequestException("Routing descriptor fields do not match request");
            }
            JsonNode signedDeviceList = descriptor.path("deviceList");
            String documentJson = signedDeviceList.path("documentJson").asText();
            JsonNode document = objectMapper.readTree(documentJson);
            long deviceListSequence = document.path("sequence").asLong();
            if (deviceListSequence <= 0
                    || !request.userId().equals(document.path("userId").asText())
                    || !verifyEcdsa(publicKeyBytes, documentJson, signedDeviceList.path("signature").asText())) {
                throw new BadRequestException("Signed DeviceList is invalid");
            }
            List<String> activeDeviceIds = new ArrayList<>();
            document.path("devices").forEach(device -> activeDeviceIds.add(device.path("deviceId").asText()));
            boolean validDeviceSignature = false;
            for (JsonNode device : descriptor.path("devices")) {
                JsonNode identity = device.path("identity");
                if (!request.userId().equals(identity.path("userId").asText())
                        || !activeDeviceIds.contains(identity.path("deviceId").asText())) continue;
                byte[] devicePublicKey = Base64.getDecoder().decode(identity.path("devicePublicKey").asText());
                if (verifyEcdsa(devicePublicKey, request.descriptorJson(), request.signature())) {
                    validDeviceSignature = true;
                    break;
                }
            }
            if (!validDeviceSignature && !verifyEcdsa(publicKeyBytes, request.descriptorJson(), request.signature())) {
                throw new BadRequestException("Routing signature is invalid");
            }
            return deviceListSequence;
        } catch (BadRequestException exception) {
            throw exception;
        } catch (Exception exception) {
            throw new BadRequestException("Routing descriptor cannot be verified");
        }
    }

    private static boolean verifyEcdsa(byte[] subjectPublicKeyInfo, String value, String encodedSignature) {
        try {
            Signature verifier = Signature.getInstance("SHA256withECDSAinP1363Format");
            verifier.initVerify(KeyFactory.getInstance("EC").generatePublic(new X509EncodedKeySpec(subjectPublicKeyInfo)));
            verifier.update(value.getBytes(StandardCharsets.UTF_8));
            return verifier.verify(Base64.getDecoder().decode(encodedSignature));
        } catch (Exception ignored) {
            return false;
        }
    }

    private static String merkleRoot(List<String> encodedHashes) {
        if (encodedHashes.isEmpty()) return V2Encoding.sha256Hex(new byte[0]);
        try {
            MessageDigest sha = MessageDigest.getInstance("SHA-256");
            List<byte[]> level = new ArrayList<>();
            for (String hash : encodedHashes) level.add(HexFormat.of().parseHex(hash));
            while (level.size() > 1) {
                List<byte[]> next = new ArrayList<>();
                for (int index = 0; index < level.size(); index += 2) {
                    byte[] left = level.get(index);
                    byte[] right = level.get(Math.min(index + 1, level.size() - 1));
                    sha.reset();
                    sha.update((byte) 1);
                    sha.update(left);
                    sha.update(right);
                    next.add(sha.digest());
                }
                level = next;
            }
            return HexFormat.of().formatHex(level.getFirst());
        } catch (Exception exception) {
            throw new IllegalStateException("Could not build transparency tree", exception);
        }
    }

    public record PublishRouting(
            String userId,
            String identityPublicKey,
            long sequence,
            String descriptorJson,
            String signature,
            Instant expiresAt
    ) {
    }

    public record RoutingRecord(
            String userId,
            String identityPublicKey,
            long sequence,
            String descriptorJson,
            String signature,
            Instant expiresAt,
            Instant updatedAt
    ) {
    }

    private record ExistingSequence(long routingSequence, long deviceListSequence) {
    }

    public record TransparencyOperation(
            long sequence,
            String operationId,
            String operationType,
            String subjectId,
            String payloadHash,
            String payload,
            String signature,
            Instant createdAt
    ) {
    }

    public record TransparencyCheckpoint(
            long treeSize,
            String rootHash,
            long createdAtUnixMilliseconds,
            String signature,
            String nodeId
    ) {
    }
}
