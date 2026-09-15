package com.turattext.v2;

import com.turattext.common.BadRequestException;
import com.turattext.common.NotFoundException;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.dao.DuplicateKeyException;
import org.springframework.dao.DataAccessException;
import org.springframework.jdbc.core.JdbcTemplate;
import org.springframework.scheduling.annotation.Scheduled;
import org.springframework.stereotype.Service;
import org.springframework.transaction.annotation.Transactional;
import org.springframework.transaction.support.TransactionSynchronization;
import org.springframework.transaction.support.TransactionSynchronizationManager;
import org.springframework.web.context.request.async.DeferredResult;

import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.time.Instant;
import java.time.temporal.ChronoUnit;
import java.util.List;
import java.util.UUID;

import static com.turattext.v2.V2Jdbc.timestamp;

@Service
public class MailboxV2Service {
    private final JdbcTemplate jdbc;
    private final long maxTtlHours;
    private final int maxEnvelopeBytes;
    private final int maxEnvelopes;
    private final int registrationPowBits;
    private final int envelopePowBits;
    private final int contactPowBits;
    private final int contactMaxEnvelopeBytes;
    private final int contactMaxEnvelopes;
    private final int maxWaitSeconds;
    private final MailboxWaitRegistry waits;

    public MailboxV2Service(
            JdbcTemplate jdbc,
            MailboxWaitRegistry waits,
            @Value("${turattext.v2.mailbox-max-ttl-hours:336}") long maxTtlHours,
            @Value("${turattext.v2.mailbox-max-envelope-bytes:524288}") int maxEnvelopeBytes,
            @Value("${turattext.v2.mailbox-max-envelopes:2000}") int maxEnvelopes,
            @Value("${turattext.v2.registration-pow-bits:0}") int registrationPowBits,
            @Value("${turattext.v2.envelope-pow-bits:0}") int envelopePowBits,
            @Value("${turattext.v2.contact-pow-bits:0}") int contactPowBits,
            @Value("${turattext.v2.contact-max-envelope-bytes:65536}") int contactMaxEnvelopeBytes,
            @Value("${turattext.v2.contact-max-envelopes:64}") int contactMaxEnvelopes,
            @Value("${turattext.v2.mailbox-max-wait-seconds:25}") int maxWaitSeconds
    ) {
        this.jdbc = jdbc;
        this.waits = waits;
        this.maxWaitSeconds = Math.clamp(maxWaitSeconds, 0, 90);
        this.maxTtlHours = maxTtlHours;
        this.maxEnvelopeBytes = maxEnvelopeBytes;
        this.maxEnvelopes = maxEnvelopes;
        this.registrationPowBits = Math.clamp(registrationPowBits, 0, 28);
        this.envelopePowBits = Math.clamp(envelopePowBits, 0, 24);
        this.contactPowBits = Math.clamp(contactPowBits, 0, 28);
        this.contactMaxEnvelopeBytes = Math.clamp(contactMaxEnvelopeBytes, 1024, maxEnvelopeBytes);
        this.contactMaxEnvelopes = Math.clamp(contactMaxEnvelopes, 1, maxEnvelopes);
    }

    @Transactional
    public MailboxRegistration register(RegisterMailbox request) {
        requirePow(request.readCapability(), request.writeCapability(), request.contactCapability(), request.proofNonce());
        String readHash = hashCapability(request.readCapability());
        String writeHash = hashCapability(request.writeCapability());
        String contactHash = hashCapability(request.contactCapability());
        if (readHash.equals(writeHash) || readHash.equals(contactHash) || writeHash.equals(contactHash)) {
            throw new BadRequestException("Mailbox capabilities must differ");
        }
        if (request.deviceHint() == null || request.deviceHint().isBlank() || request.deviceHint().length() > 160) {
            throw new BadRequestException("Device hint is required");
        }
        Instant now = Instant.now();
        Instant expiresAt = clampExpiry(request.expiresAt(), now, maxTtlHours);
        UUID id = UUID.randomUUID();
        try {
            jdbc.update("""
                    insert into v2_mailboxes(
                        id, read_capability_hash, write_capability_hash, contact_capability_hash,
                        device_hint, created_at, expires_at)
                    values (?, ?, ?, ?, ?, ?, ?)
                    """, id, readHash, writeHash, contactHash, request.deviceHint(), timestamp(now), timestamp(expiresAt));
        } catch (DuplicateKeyException exception) {
            List<MailboxRegistration> existing = jdbc.query("""
                    select id, device_hint, created_at, expires_at from v2_mailboxes
                    where read_capability_hash = ? and write_capability_hash = ? and contact_capability_hash = ?
                    """, (row, ignored) -> new MailboxRegistration(
                    row.getObject("id", UUID.class),
                    row.getString("device_hint"),
                    row.getTimestamp("created_at").toInstant(),
                    row.getTimestamp("expires_at").toInstant()), readHash, writeHash, contactHash);
            if (!existing.isEmpty()) return existing.getFirst();
            throw new BadRequestException("Capability collision");
        }
        return new MailboxRegistration(id, request.deviceHint(), now, expiresAt);
    }

    @Transactional
    public StoredEnvelope put(UUID mailboxId, String writeCapability, String proofNonce, PutEnvelope request) {
        MailboxAccess access = requireWriteAccess(mailboxId, writeCapability);
        Mailbox mailbox = access.mailbox();
        validateEnvelope(request);
        requireEnvelopePow(mailboxId, request, proofNonce, access.contact() ? contactPowBits : envelopePowBits);
        Integer count = jdbc.queryForObject(
                "select count(*) from v2_envelopes where mailbox_id = ?", Integer.class, mailboxId);
        if (count != null && count >= maxEnvelopes) {
            throw new BadRequestException("Mailbox envelope quota exceeded");
        }
        if (access.contact()) {
            Integer contactCount = jdbc.queryForObject(
                    "select count(*) from v2_envelopes where mailbox_id = ? and access_class = 'contact'",
                    Integer.class, mailboxId);
            if (contactCount != null && contactCount >= contactMaxEnvelopes) {
                throw new BadRequestException("Public contact inbox quota exceeded");
            }
        }

        Instant now = Instant.now();
        Instant requestedExpiry = request.expiresAt() == null
                ? now.plus(maxTtlHours, ChronoUnit.HOURS)
                : request.expiresAt();
        Instant expiresAt = requestedExpiry.isAfter(mailbox.expiresAt())
                ? mailbox.expiresAt()
                : clampExpiry(requestedExpiry, now, maxTtlHours);
        int decodedBytes;
        try {
            decodedBytes = java.util.Base64.getDecoder().decode(request.opaquePayload()).length;
        } catch (IllegalArgumentException exception) {
            throw new BadRequestException("Opaque payload must be base64");
        }
        if (decodedBytes > maxEnvelopeBytes) throw new BadRequestException("Envelope is too large");
        if (access.contact() && decodedBytes > contactMaxEnvelopeBytes) {
            throw new BadRequestException("Public contact request is too large");
        }
        int sizeClass = normalizedSizeClass(request.sizeClass(), decodedBytes);

        try {
            jdbc.update("""
                    insert into v2_envelopes(
                        id, mailbox_id, protocol_version, recipient_device_hint, opaque_payload,
                        size_class, created_at, expires_at, received_at, access_class)
                    values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """, request.envelopeId(), mailboxId, request.protocolVersion(),
                    request.recipientDeviceHint(), request.opaquePayload(), sizeClass,
                    timestamp(request.createdAt() == null ? now : request.createdAt()), timestamp(expiresAt), timestamp(now),
                    access.contact() ? "contact" : "private");
            notifyReaders(mailboxId);
        } catch (DuplicateKeyException ignored) {
            // At-least-once delivery: the same envelope ID is idempotent.
        }
        return new StoredEnvelope(
                request.envelopeId(), request.protocolVersion(), request.recipientDeviceHint(),
                request.opaquePayload(), sizeClass,
                request.createdAt() == null ? now : request.createdAt(), expiresAt);
    }

    public List<StoredEnvelope> fetch(UUID mailboxId, String readCapability, int requestedLimit) {
        requireMailbox(mailboxId, readCapability, true);
        return select(mailboxId, requestedLimit);
    }

    /**
     * Выдача конвертов с ожиданием: если ящик пуст, Node держит запрос открытым до
     * {@code waitSeconds} и отвечает сразу, как только конверт придёт. Пустой ответ по концу
     * окна — обычное дело, клиент просто спрашивает снова.
     *
     * <p>Возможность чтения проверяется до того, как читатель попадёт в очередь ожидания:
     * держать открытый запрос без прав нельзя.
     */
    public DeferredResult<List<StoredEnvelope>> fetchOrWait(
            UUID mailboxId, String readCapability, int requestedLimit, int requestedWait) {
        requireMailbox(mailboxId, readCapability, true);
        int wait = Math.clamp(requestedWait, 0, maxWaitSeconds);
        List<StoredEnvelope> ready = select(mailboxId, requestedLimit);
        DeferredResult<List<StoredEnvelope>> result =
                new DeferredResult<>(wait * 1000L + 1000L, List::of);
        if (wait == 0 || !ready.isEmpty()) {
            result.setResult(ready);
            return result;
        }

        Runnable wake = () -> {
            if (!result.isSetOrExpired()) result.setResult(select(mailboxId, requestedLimit));
        };
        if (!waits.await(mailboxId, wake)) {
            result.setResult(List.of());
            return result;
        }
        result.onCompletion(() -> waits.cancel(mailboxId, wake));
        // Конверт мог лечь между выборкой и постановкой в очередь: перепроверяем, иначе
        // это сообщение прождало бы всё окно впустую.
        List<StoredEnvelope> raced = select(mailboxId, requestedLimit);
        if (!raced.isEmpty() && !result.isSetOrExpired()) result.setResult(raced);
        return result;
    }

    private void notifyReaders(UUID mailboxId) {
        if (!TransactionSynchronizationManager.isSynchronizationActive()) {
            waits.awaken(mailboxId);
            return;
        }
        // Будить до коммита нельзя: читатель успел бы сделать выборку и не увидеть конверт.
        TransactionSynchronizationManager.registerSynchronization(new TransactionSynchronization() {
            @Override
            public void afterCommit() {
                waits.awaken(mailboxId);
            }
        });
    }

    private List<StoredEnvelope> select(UUID mailboxId, int requestedLimit) {
        int limit = Math.clamp(requestedLimit, 1, 200);
        return jdbc.query("""
                select id, protocol_version, recipient_device_hint, opaque_payload,
                       size_class, created_at, expires_at
                from v2_envelopes
                where mailbox_id = ? and expires_at > ?
                order by received_at, id
                limit ?
                """, (row, ignored) -> new StoredEnvelope(
                row.getString("id"),
                row.getInt("protocol_version"),
                row.getString("recipient_device_hint"),
                row.getString("opaque_payload"),
                row.getInt("size_class"),
                row.getTimestamp("created_at").toInstant(),
                row.getTimestamp("expires_at").toInstant()), mailboxId, timestamp(Instant.now()), limit);
    }

    @Transactional
    public int acknowledge(UUID mailboxId, String readCapability, List<String> envelopeIds) {
        requireMailbox(mailboxId, readCapability, true);
        if (envelopeIds == null || envelopeIds.isEmpty() || envelopeIds.size() > 200) {
            throw new BadRequestException("One to 200 envelope IDs are required");
        }
        int deleted = 0;
        for (String envelopeId : envelopeIds) {
            if (envelopeId != null && envelopeId.matches("[A-Za-z0-9_-]{16,100}")) {
                deleted += jdbc.update(
                        "delete from v2_envelopes where mailbox_id = ? and id = ?", mailboxId, envelopeId);
            }
        }
        return deleted;
    }

    public Mailbox requireWriteMailbox(UUID mailboxId, String capability) {
        MailboxAccess access = requireWriteAccess(mailboxId, capability);
        if (access.contact()) throw new NotFoundException("Mailbox not found");
        return access.mailbox();
    }

    @Scheduled(fixedDelayString = "${turattext.v2.cleanup-interval-ms:3600000}")
    @Transactional
    public void cleanupExpired() {
        Instant now = Instant.now();
        try {
            jdbc.update("delete from v2_envelopes where expires_at <= ?", timestamp(now));
            jdbc.update("delete from v2_mailboxes where expires_at <= ?", timestamp(now));
            jdbc.update("delete from v2_routing_records where expires_at <= ?", timestamp(now));
            jdbc.update("delete from v2_username_claims where expires_at <= ?", timestamp(now));
            jdbc.update("delete from v2_blob_objects where expires_at <= ?", timestamp(now));
        } catch (DataAccessException ignored) {
            // The dev H2 compatibility runner may still be creating the v2 schema during startup.
        }
    }

    private Mailbox requireMailbox(UUID mailboxId, String capability, boolean read) {
        String hash;
        try {
            hash = hashCapability(capability);
        } catch (IllegalArgumentException exception) {
            throw new NotFoundException("Mailbox not found");
        }
        String column = read ? "read_capability_hash" : "write_capability_hash";
        List<Mailbox> values = jdbc.query(
                "select id, device_hint, expires_at from v2_mailboxes where id = ? and " + column + " = ? and expires_at > ?",
                (row, ignored) -> new Mailbox(
                        row.getObject("id", UUID.class),
                        row.getString("device_hint"),
                        row.getTimestamp("expires_at").toInstant()),
                mailboxId, hash, timestamp(Instant.now()));
        if (values.isEmpty()) throw new NotFoundException("Mailbox not found");
        return values.getFirst();
    }

    private MailboxAccess requireWriteAccess(UUID mailboxId, String capability) {
        String hash;
        try {
            hash = hashCapability(capability);
        } catch (IllegalArgumentException exception) {
            throw new NotFoundException("Mailbox not found");
        }
        List<MailboxAccess> values = jdbc.query("""
                select id, device_hint, expires_at, write_capability_hash, contact_capability_hash
                from v2_mailboxes
                where id = ? and expires_at > ?
                  and (write_capability_hash = ? or contact_capability_hash = ?)
                """, (row, ignored) -> new MailboxAccess(
                new Mailbox(
                        row.getObject("id", UUID.class), row.getString("device_hint"),
                        row.getTimestamp("expires_at").toInstant()),
                hash.equals(row.getString("contact_capability_hash"))),
                mailboxId, timestamp(Instant.now()), hash, hash);
        if (values.isEmpty()) throw new NotFoundException("Mailbox not found");
        return values.getFirst();
    }

    private void validateEnvelope(PutEnvelope request) {
        if (request.envelopeId() == null || !request.envelopeId().matches("[A-Za-z0-9_-]{16,100}")) {
            throw new BadRequestException("Invalid envelope ID");
        }
        if (request.protocolVersion() < 2 || request.protocolVersion() > 1000) {
            throw new BadRequestException("Unsupported protocol version");
        }
        if (request.recipientDeviceHint() != null && request.recipientDeviceHint().length() > 160) {
            throw new BadRequestException("Recipient hint is too long");
        }
        if (request.opaquePayload() == null || request.opaquePayload().isBlank()) {
            throw new BadRequestException("Opaque payload is required");
        }
    }

    private void requirePow(String readCapability, String writeCapability, String contactCapability, String nonce) {
        if (registrationPowBits == 0) return;
        if (nonce == null || nonce.length() > 100) throw new BadRequestException("Registration proof of work is required");
        try {
            byte[] digest = MessageDigest.getInstance("SHA-256").digest(
                    (readCapability + ":" + writeCapability + ":" + contactCapability + ":" + nonce)
                            .getBytes(StandardCharsets.UTF_8));
            int zeros = 0;
            for (byte value : digest) {
                int unsigned = value & 0xff;
                if (unsigned == 0) {
                    zeros += 8;
                    continue;
                }
                zeros += Integer.numberOfLeadingZeros(unsigned) - 24;
                break;
            }
            if (zeros < registrationPowBits) throw new BadRequestException("Invalid registration proof of work");
        } catch (java.security.NoSuchAlgorithmException exception) {
            throw new IllegalStateException(exception);
        }
    }

    private static String hashCapability(String capability) {
        try {
            return V2Encoding.capabilityHash(capability);
        } catch (IllegalArgumentException exception) {
            throw new BadRequestException(exception.getMessage());
        }
    }

    private void requireEnvelopePow(UUID mailboxId, PutEnvelope request, String nonce, int requiredBits) {
        if (requiredBits == 0) return;
        if (nonce == null || nonce.length() > 100) throw new BadRequestException("Envelope proof of work is required");
        String payloadHash = V2Encoding.sha256Hex(request.opaquePayload().getBytes(StandardCharsets.UTF_8));
        requireLeadingZeroProof(
                mailboxId + ":" + request.envelopeId() + ":" + payloadHash + ":" + nonce,
                requiredBits,
                "Invalid envelope proof of work");
    }

    private static void requireLeadingZeroProof(String material, int requiredBits, String error) {
        try {
            byte[] digest = MessageDigest.getInstance("SHA-256").digest(material.getBytes(StandardCharsets.UTF_8));
            int zeros = 0;
            for (byte value : digest) {
                int unsigned = value & 0xff;
                if (unsigned == 0) { zeros += 8; continue; }
                zeros += Integer.numberOfLeadingZeros(unsigned) - 24;
                break;
            }
            if (zeros < requiredBits) throw new BadRequestException(error);
        } catch (java.security.NoSuchAlgorithmException exception) {
            throw new IllegalStateException(exception);
        }
    }

    private static Instant clampExpiry(Instant requested, Instant now, long maxHours) {
        Instant maximum = now.plus(maxHours, ChronoUnit.HOURS);
        Instant value = requested == null ? maximum : requested;
        if (!value.isAfter(now.plus(5, ChronoUnit.MINUTES))) {
            throw new BadRequestException("Expiry must be in the future");
        }
        return value.isAfter(maximum) ? maximum : value;
    }

    private static int normalizedSizeClass(Integer requested, int actualBytes) {
        int[] classes = {1024, 4096, 16384, 65536, 262144, 524288};
        int minimum = actualBytes;
        if (requested != null) minimum = Math.max(minimum, requested);
        for (int value : classes) if (value >= minimum) return value;
        throw new BadRequestException("Envelope exceeds the largest size class");
    }

    public record RegisterMailbox(
            String readCapability,
            String writeCapability,
            String contactCapability,
            String deviceHint,
            Instant expiresAt,
            String proofNonce
    ) {
    }

    public record MailboxRegistration(UUID mailboxId, String deviceHint, Instant createdAt, Instant expiresAt) {
    }

    public record PutEnvelope(
            String envelopeId,
            int protocolVersion,
            String recipientDeviceHint,
            String opaquePayload,
            Integer sizeClass,
            Instant createdAt,
            Instant expiresAt
    ) {
    }

    public record StoredEnvelope(
            String envelopeId,
            int protocolVersion,
            String recipientDeviceHint,
            String opaquePayload,
            int sizeClass,
            Instant createdAt,
            Instant expiresAt
    ) {
    }

    public record Mailbox(UUID id, String deviceHint, Instant expiresAt) {
    }

    private record MailboxAccess(Mailbox mailbox, boolean contact) {
    }
}
