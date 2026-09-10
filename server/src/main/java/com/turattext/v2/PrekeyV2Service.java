package com.turattext.v2;

import com.turattext.common.BadRequestException;
import com.turattext.common.NotFoundException;
import org.springframework.dao.DuplicateKeyException;
import org.springframework.jdbc.core.JdbcTemplate;
import org.springframework.stereotype.Service;
import org.springframework.transaction.annotation.Transactional;

import java.time.Instant;
import java.time.temporal.ChronoUnit;
import java.util.List;
import java.util.UUID;

import static com.turattext.v2.V2Jdbc.timestamp;

@Service
public class PrekeyV2Service {
    private final JdbcTemplate jdbc;
    private final MailboxV2Service mailboxes;

    public PrekeyV2Service(JdbcTemplate jdbc, MailboxV2Service mailboxes) {
        this.jdbc = jdbc;
        this.mailboxes = mailboxes;
    }

    @Transactional
    public PublishResult publish(UUID mailboxId, String writeCapability, PublishBundle request) {
        MailboxV2Service.Mailbox mailbox = mailboxes.requireWriteMailbox(mailboxId, writeCapability);
        if (!mailbox.deviceHint().equals(request.deviceId())) {
            throw new BadRequestException("Mailbox device does not match prekey device");
        }
        validate(request);
        Instant now = Instant.now();
        Instant expiresAt = request.expiresAt().isAfter(mailbox.expiresAt())
                ? mailbox.expiresAt()
                : request.expiresAt();

        List<ExistingBundle> existing = jdbc.query("""
                select sequence_number, user_id, device_id, identity_json, signed_prekey_json
                from v2_prekey_bundles where mailbox_id = ?
                """, (row, ignored) -> new ExistingBundle(
                row.getLong("sequence_number"), row.getString("user_id"),
                row.getString("device_id"), row.getString("identity_json"),
                row.getString("signed_prekey_json")), mailboxId);
        if (existing.isEmpty()) {
            jdbc.update("""
                    insert into v2_prekey_bundles(
                        mailbox_id, user_id, device_id, identity_json, signed_prekey_json,
                        sequence_number, expires_at, updated_at)
                    values (?, ?, ?, ?, ?, ?, ?, ?)
                    """, mailboxId, request.userId(), request.deviceId(), request.identityJson(),
                    request.signedPrekeyJson(), request.sequence(), timestamp(expiresAt), timestamp(now));
        } else {
            ExistingBundle current = existing.getFirst();
            if (request.sequence() < current.sequence()
                    || (request.sequence() == current.sequence()
                    && (!request.userId().equals(current.userId())
                    || !request.deviceId().equals(current.deviceId())
                    || !request.identityJson().equals(current.identityJson())
                    || !request.signedPrekeyJson().equals(current.signedPrekeyJson())))) {
                throw new BadRequestException("Prekey bundle sequence must increase");
            }
            if (request.sequence() > current.sequence()) jdbc.update("""
                    update v2_prekey_bundles
                    set user_id = ?, device_id = ?, identity_json = ?, signed_prekey_json = ?,
                        sequence_number = ?, expires_at = ?, updated_at = ?
                    where mailbox_id = ?
                    """, request.userId(), request.deviceId(), request.identityJson(),
                    request.signedPrekeyJson(), request.sequence(), timestamp(expiresAt), timestamp(now), mailboxId);
        }

        int inserted = 0;
        for (OneTimePrekey prekey : request.oneTimePrekeys()) {
            try {
                inserted += jdbc.update("""
                        insert into v2_one_time_prekeys(id, mailbox_id, prekey_id, prekey_json, created_at)
                        values (?, ?, ?, ?, ?)
                        """, UUID.randomUUID(), mailboxId, prekey.prekeyId(), prekey.prekeyJson(), timestamp(now));
            } catch (DuplicateKeyException ignored) {
                // A one-time prekey is never reinserted under the same mailbox batch.
            }
        }
        Integer available = jdbc.queryForObject(
                "select count(*) from v2_one_time_prekeys where mailbox_id = ?", Integer.class, mailboxId);
        return new PublishResult(request.sequence(), inserted, available == null ? 0 : available, expiresAt);
    }

    @Transactional
    public ClaimedBundle claim(String userId, String deviceId) {
        List<BundleRow> bundles = jdbc.query("""
                select mailbox_id, identity_json, signed_prekey_json, sequence_number, expires_at
                from v2_prekey_bundles
                where user_id = ? and device_id = ? and expires_at > ?
                """, (row, ignored) -> new BundleRow(
                row.getObject("mailbox_id", UUID.class),
                row.getString("identity_json"),
                row.getString("signed_prekey_json"),
                row.getLong("sequence_number"),
                row.getTimestamp("expires_at").toInstant()), userId, deviceId, timestamp(Instant.now()));
        if (bundles.isEmpty()) throw new NotFoundException("Prekey bundle not found");
        BundleRow bundle = bundles.getFirst();

        List<PrekeyRow> available = jdbc.query("""
                select id, prekey_id, prekey_json
                from v2_one_time_prekeys
                where mailbox_id = ?
                order by created_at, id
                limit 1
                for update
                """, (row, ignored) -> new PrekeyRow(
                row.getObject("id", UUID.class), row.getString("prekey_id"), row.getString("prekey_json")),
                bundle.mailboxId());
        OneTimePrekey oneTime = null;
        if (!available.isEmpty()) {
            PrekeyRow selected = available.getFirst();
            jdbc.update("delete from v2_one_time_prekeys where id = ?", selected.id());
            oneTime = new OneTimePrekey(selected.prekeyId(), selected.prekeyJson());
        }
        return new ClaimedBundle(
                userId, deviceId, bundle.identityJson(), bundle.signedPrekeyJson(), oneTime,
                bundle.sequence(), bundle.expiresAt());
    }

    private static void validate(PublishBundle request) {
        if (request.userId() == null || !request.userId().matches("tt1-[a-f0-9]{64}")) {
            throw new BadRequestException("Invalid UserID");
        }
        if (request.deviceId() == null || !request.deviceId().matches("ttd1-[a-f0-9]{64}")) {
            throw new BadRequestException("Invalid DeviceID");
        }
        if (request.sequence() <= 0) throw new BadRequestException("Sequence must be positive");
        if (request.identityJson() == null || request.identityJson().length() > 32_000
                || request.signedPrekeyJson() == null || request.signedPrekeyJson().length() > 64_000) {
            throw new BadRequestException("Prekey bundle is too large");
        }
        if (request.expiresAt() == null || request.expiresAt().isBefore(Instant.now().plus(5, ChronoUnit.MINUTES))) {
            throw new BadRequestException("Prekey bundle expiry is invalid");
        }
        if (request.oneTimePrekeys() == null || request.oneTimePrekeys().size() > 200) {
            throw new BadRequestException("At most 200 one-time prekeys are accepted per request");
        }
        for (OneTimePrekey prekey : request.oneTimePrekeys()) {
            if (prekey.prekeyId() == null || !prekey.prekeyId().matches("[A-Za-z0-9_-]{8,100}")
                    || prekey.prekeyJson() == null || prekey.prekeyJson().length() > 16_000) {
                throw new BadRequestException("Invalid one-time prekey");
            }
        }
    }

    public record OneTimePrekey(String prekeyId, String prekeyJson) {
    }

    public record PublishBundle(
            String userId,
            String deviceId,
            String identityJson,
            String signedPrekeyJson,
            long sequence,
            Instant expiresAt,
            List<OneTimePrekey> oneTimePrekeys
    ) {
    }

    public record PublishResult(long sequence, int inserted, int available, Instant expiresAt) {
    }

    public record ClaimedBundle(
            String userId,
            String deviceId,
            String identityJson,
            String signedPrekeyJson,
            OneTimePrekey oneTimePrekey,
            long sequence,
            Instant expiresAt
    ) {
    }

    private record BundleRow(
            UUID mailboxId,
            String identityJson,
            String signedPrekeyJson,
            long sequence,
            Instant expiresAt
    ) {
    }

    private record PrekeyRow(UUID id, String prekeyId, String prekeyJson) {
    }

    private record ExistingBundle(
            long sequence,
            String userId,
            String deviceId,
            String identityJson,
            String signedPrekeyJson
    ) {
    }
}
