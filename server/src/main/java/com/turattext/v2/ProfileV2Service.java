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
import java.security.Signature;
import java.security.spec.X509EncodedKeySpec;
import java.time.Instant;
import java.time.temporal.ChronoUnit;
import java.util.Base64;
import java.util.List;

import static com.turattext.v2.V2Jdbc.timestamp;

@Service
public class ProfileV2Service {
    private final JdbcTemplate jdbc;
    private final ObjectMapper objectMapper;
    private final NodeIdentityService nodeIdentity;

    public ProfileV2Service(JdbcTemplate jdbc, ObjectMapper objectMapper, NodeIdentityService nodeIdentity) {
        this.jdbc = jdbc;
        this.objectMapper = objectMapper;
        this.nodeIdentity = nodeIdentity;
    }

    @Transactional
    public ProfileRecord publish(PublishProfile request) {
        validateAndVerify(request);
        List<Long> existing = jdbc.query("""
                select sequence_number from v2_profile_claims where user_id = ?
                """, (row, ignored) -> row.getLong("sequence_number"), request.userId());
        if (!existing.isEmpty() && request.sequence() <= existing.getFirst()) {
            throw new BadRequestException("Profile claim sequence must increase");
        }

        Instant now = Instant.now();
        jdbc.update("delete from v2_profile_claims where user_id = ?", request.userId());
        jdbc.update("""
                insert into v2_profile_claims(
                    user_id, identity_public_key, sequence_number,
                    claim_json, signature, expires_at, updated_at)
                values (?, ?, ?, ?, ?, ?, ?)
                """, request.userId(), request.identityPublicKey(), request.sequence(),
                request.claimJson(), request.signature(), timestamp(request.expiresAt()), timestamp(now));

        String payloadHash = V2Encoding.sha256Hex(request.claimJson().getBytes(StandardCharsets.UTF_8));
        String operationId = "op1-" + V2Encoding.randomToken(18);
        String operationSignature = nodeIdentity.sign(
                (operationId + "\nprofile.update\n" + request.userId() + "\n" + payloadHash)
                        .getBytes(StandardCharsets.UTF_8));
        jdbc.update("""
                insert into v2_transparency_operations(
                    operation_id, operation_type, subject_id, payload_hash, payload, signature, created_at)
                values (?, ?, ?, ?, ?, ?, ?)
                """, operationId, "profile.update", request.userId(), payloadHash,
                request.claimJson(), operationSignature, timestamp(now));
        return byUser(request.userId());
    }

    public ProfileRecord byUser(String userId) {
        if (userId == null || !userId.matches("tt1-[a-f0-9]{64}")) {
            throw new BadRequestException("Invalid UserID");
        }
        List<ProfileRecord> records = jdbc.query("""
                select user_id, identity_public_key, sequence_number,
                       claim_json, signature, expires_at, updated_at
                from v2_profile_claims
                where user_id = ? and expires_at > ?
                """, ProfileV2Service::map, userId, timestamp(Instant.now()));
        if (records.isEmpty()) throw new NotFoundException("Profile claim not found");
        return records.getFirst();
    }

    private void validateAndVerify(PublishProfile request) {
        try {
            if (request.userId() == null || !request.userId().matches("tt1-[a-f0-9]{64}")) {
                throw new BadRequestException("Invalid UserID");
            }
            byte[] publicKey = Base64.getDecoder().decode(request.identityPublicKey());
            if (!("tt1-" + V2Encoding.sha256Hex(publicKey)).equals(request.userId())) {
                throw new BadRequestException("UserID does not match identity key");
            }
            if (request.sequence() <= 0 || request.claimJson() == null || request.claimJson().length() > 200_000) {
                throw new BadRequestException("Invalid profile claim");
            }
            if (request.expiresAt() == null
                    || request.expiresAt().isBefore(Instant.now().plus(5, ChronoUnit.MINUTES))
                    || request.expiresAt().isAfter(Instant.now().plus(90, ChronoUnit.DAYS))) {
                throw new BadRequestException("Profile claim expiry is invalid");
            }
            JsonNode claim = objectMapper.readTree(request.claimJson());
            if (claim.path("version").asInt() != 1
                    || !request.userId().equals(claim.path("userId").asText())
                    || request.sequence() != claim.path("sequence").asLong()
                    || request.expiresAt().toEpochMilli() != claim.path("expiresAtUnixMilliseconds").asLong()) {
                throw new BadRequestException("Profile claim fields do not match request");
            }
            String displayName = claim.path("displayName").asText("");
            String about = claim.path("about").asText("");
            if (displayName.length() > 64 || about.length() > 200) {
                throw new BadRequestException("Profile claim fields exceed length limits");
            }
            Signature verifier = Signature.getInstance("SHA256withECDSAinP1363Format");
            verifier.initVerify(KeyFactory.getInstance("EC").generatePublic(new X509EncodedKeySpec(publicKey)));
            verifier.update(request.claimJson().getBytes(StandardCharsets.UTF_8));
            if (!verifier.verify(Base64.getDecoder().decode(request.signature()))) {
                throw new BadRequestException("Profile claim signature is invalid");
            }
        } catch (BadRequestException exception) {
            throw exception;
        } catch (Exception exception) {
            throw new BadRequestException("Profile claim cannot be verified");
        }
    }

    private static ProfileRecord map(java.sql.ResultSet row, int ignored) throws java.sql.SQLException {
        return new ProfileRecord(
                row.getString("user_id"), row.getString("identity_public_key"), row.getLong("sequence_number"),
                row.getString("claim_json"), row.getString("signature"),
                row.getTimestamp("expires_at").toInstant(), row.getTimestamp("updated_at").toInstant());
    }

    public record PublishProfile(
            String userId,
            String identityPublicKey,
            long sequence,
            String claimJson,
            String signature,
            Instant expiresAt
    ) {
    }

    public record ProfileRecord(
            String userId,
            String identityPublicKey,
            long sequence,
            String claimJson,
            String signature,
            Instant expiresAt,
            Instant updatedAt
    ) {
    }
}
