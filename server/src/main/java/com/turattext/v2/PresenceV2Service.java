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

/**
 * «Последняя активность» — подписанная владельцем identity запись directory.
 * Запись публикуется только по явному согласию пользователя, живёт не дольше недели
 * и намеренно не попадает в append-only transparency log: это эфемерные данные,
 * а не привязка идентичности.
 */
@Service
public class PresenceV2Service {
    private static final long MAX_LIFETIME_DAYS = 8;

    private final JdbcTemplate jdbc;
    private final ObjectMapper objectMapper;

    public PresenceV2Service(JdbcTemplate jdbc, ObjectMapper objectMapper) {
        this.jdbc = jdbc;
        this.objectMapper = objectMapper;
    }

    @Transactional
    public PresenceRecord publish(PublishPresence request) {
        validateAndVerify(request);
        List<Long> existing = jdbc.query("""
                select sequence_number from v2_presence_claims where user_id = ?
                """, (row, ignored) -> row.getLong("sequence_number"), request.userId());
        if (!existing.isEmpty() && request.sequence() <= existing.getFirst()) {
            throw new BadRequestException("Presence claim sequence must increase");
        }

        Instant now = Instant.now();
        jdbc.update("delete from v2_presence_claims where user_id = ?", request.userId());
        jdbc.update("""
                insert into v2_presence_claims(
                    user_id, identity_public_key, sequence_number,
                    claim_json, signature, expires_at, updated_at)
                values (?, ?, ?, ?, ?, ?, ?)
                """, request.userId(), request.identityPublicKey(), request.sequence(),
                request.claimJson(), request.signature(), timestamp(request.expiresAt()), timestamp(now));
        return byUser(request.userId());
    }

    public PresenceRecord byUser(String userId) {
        if (userId == null || !userId.matches("tt1-[a-f0-9]{64}")) {
            throw new BadRequestException("Invalid UserID");
        }
        List<PresenceRecord> records = jdbc.query("""
                select user_id, identity_public_key, sequence_number,
                       claim_json, signature, expires_at, updated_at
                from v2_presence_claims
                where user_id = ? and expires_at > ?
                """, PresenceV2Service::map, userId, timestamp(Instant.now()));
        if (records.isEmpty()) throw new NotFoundException("Presence claim not found");
        return records.getFirst();
    }

    private void validateAndVerify(PublishPresence request) {
        try {
            if (request.userId() == null || !request.userId().matches("tt1-[a-f0-9]{64}")) {
                throw new BadRequestException("Invalid UserID");
            }
            byte[] publicKey = Base64.getDecoder().decode(request.identityPublicKey());
            if (!("tt1-" + V2Encoding.sha256Hex(publicKey)).equals(request.userId())) {
                throw new BadRequestException("UserID does not match identity key");
            }
            if (request.sequence() <= 0 || request.claimJson() == null || request.claimJson().length() > 4_000) {
                throw new BadRequestException("Invalid presence claim");
            }
            if (request.expiresAt() == null
                    || request.expiresAt().isBefore(Instant.now().plus(5, ChronoUnit.MINUTES))
                    || request.expiresAt().isAfter(Instant.now().plus(MAX_LIFETIME_DAYS, ChronoUnit.DAYS))) {
                throw new BadRequestException("Presence claim expiry is invalid");
            }
            JsonNode claim = objectMapper.readTree(request.claimJson());
            long lastSeen = claim.path("lastSeenUnixMilliseconds").asLong(-1);
            if (claim.path("version").asInt() != 1
                    || !request.userId().equals(claim.path("userId").asText())
                    || request.expiresAt().toEpochMilli() != claim.path("expiresAtUnixMilliseconds").asLong()) {
                throw new BadRequestException("Presence claim fields do not match request");
            }
            // 0 означает «активность скрыта»: так клиент отзывает ранее опубликованную запись.
            if (lastSeen < 0 || lastSeen > Instant.now().plus(1, ChronoUnit.HOURS).toEpochMilli()) {
                throw new BadRequestException("Presence timestamp is invalid");
            }
            Signature verifier = Signature.getInstance("SHA256withECDSAinP1363Format");
            verifier.initVerify(KeyFactory.getInstance("EC").generatePublic(new X509EncodedKeySpec(publicKey)));
            verifier.update(request.claimJson().getBytes(StandardCharsets.UTF_8));
            if (!verifier.verify(Base64.getDecoder().decode(request.signature()))) {
                throw new BadRequestException("Presence claim signature is invalid");
            }
        } catch (BadRequestException exception) {
            throw exception;
        } catch (Exception exception) {
            throw new BadRequestException("Presence claim cannot be verified");
        }
    }

    private static PresenceRecord map(java.sql.ResultSet row, int ignored) throws java.sql.SQLException {
        return new PresenceRecord(
                row.getString("user_id"), row.getString("identity_public_key"), row.getLong("sequence_number"),
                row.getString("claim_json"), row.getString("signature"),
                row.getTimestamp("expires_at").toInstant(), row.getTimestamp("updated_at").toInstant());
    }

    public record PublishPresence(
            String userId,
            String identityPublicKey,
            long sequence,
            String claimJson,
            String signature,
            Instant expiresAt
    ) {
    }

    public record PresenceRecord(
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
