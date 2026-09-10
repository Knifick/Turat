package com.turattext.v2;

import com.turattext.common.BadRequestException;
import com.turattext.common.NotFoundException;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.dao.DuplicateKeyException;
import org.springframework.jdbc.core.JdbcTemplate;
import org.springframework.stereotype.Service;
import org.springframework.transaction.annotation.Transactional;

import java.security.MessageDigest;
import java.time.Instant;
import java.time.temporal.ChronoUnit;
import java.util.Base64;
import java.util.List;

import static com.turattext.v2.V2Jdbc.timestamp;

@Service
public class BlobV2Service {
    private final JdbcTemplate jdbc;
    private final long maxBlobBytes;
    private final int maxChunkBytes;

    public BlobV2Service(
            JdbcTemplate jdbc,
            @Value("${turattext.v2.blob-max-bytes:104857600}") long maxBlobBytes,
            @Value("${turattext.v2.blob-max-chunk-bytes:1048576}") int maxChunkBytes
    ) {
        this.jdbc = jdbc;
        this.maxBlobBytes = maxBlobBytes;
        this.maxChunkBytes = maxChunkBytes;
    }

    @Transactional
    public BlobRegistration register(RegisterBlob request) {
        if (request.objectId() == null || !request.objectId().matches("blob1-[A-Za-z0-9_-]{16,90}")) {
            throw new BadRequestException("Invalid blob object ID");
        }
        String readHash = capabilityHash(request.readCapability());
        String writeHash = capabilityHash(request.writeCapability());
        if (readHash.equals(writeHash)) throw new BadRequestException("Blob capabilities must differ");
        if (request.expectedSize() < 1 || request.expectedSize() > maxBlobBytes) {
            throw new BadRequestException("Blob size is outside node limits");
        }
        if (request.chunkSize() < 4096 || request.chunkSize() > maxChunkBytes) {
            throw new BadRequestException("Blob chunk size is outside node limits");
        }
        Instant now = Instant.now();
        Instant maximum = now.plus(14, ChronoUnit.DAYS);
        Instant expiresAt = request.expiresAt() == null || request.expiresAt().isAfter(maximum)
                ? maximum : request.expiresAt();
        if (!expiresAt.isAfter(now.plus(5, ChronoUnit.MINUTES))) {
            throw new BadRequestException("Blob expiry must be in the future");
        }
        try {
            jdbc.update("""
                    insert into v2_blob_objects(
                        id, read_capability_hash, write_capability_hash, expected_size,
                        chunk_size, created_at, expires_at)
                    values (?, ?, ?, ?, ?, ?, ?)
                    """, request.objectId(), readHash, writeHash, request.expectedSize(),
                    request.chunkSize(), timestamp(now), timestamp(expiresAt));
        } catch (DuplicateKeyException exception) {
            throw new BadRequestException("Blob object or capability already exists");
        }
        return new BlobRegistration(
                request.objectId(), request.expectedSize(), request.chunkSize(), now, expiresAt);
    }

    @Transactional
    public BlobChunk putChunk(
            String objectId,
            int chunkIndex,
            String writeCapability,
            String declaredDigest,
            byte[] ciphertext
    ) {
        BlobObject object = requireObject(objectId, writeCapability, false);
        if (chunkIndex < 0 || chunkIndex > 1_000_000) throw new BadRequestException("Invalid chunk index");
        if (ciphertext.length == 0 || ciphertext.length > object.chunkSize() || ciphertext.length > maxChunkBytes) {
            throw new BadRequestException("Blob chunk is outside node limits");
        }
        String actualDigest = V2Encoding.sha256Hex(ciphertext);
        if (declaredDigest == null || !MessageDigest.isEqual(
                actualDigest.getBytes(java.nio.charset.StandardCharsets.US_ASCII),
                declaredDigest.toLowerCase().getBytes(java.nio.charset.StandardCharsets.US_ASCII))) {
            throw new BadRequestException("Blob chunk digest mismatch");
        }
        Long existingSize = jdbc.queryForObject(
                "select coalesce(sum(size_bytes), 0) from v2_blob_chunks where object_id = ? and chunk_index <> ?",
                Long.class, objectId, chunkIndex);
        if ((existingSize == null ? 0 : existingSize) + ciphertext.length > object.expectedSize() + object.chunkSize()) {
            throw new BadRequestException("Blob exceeds declared size");
        }
        jdbc.update("delete from v2_blob_chunks where object_id = ? and chunk_index = ?", objectId, chunkIndex);
        jdbc.update("""
                insert into v2_blob_chunks(
                    object_id, chunk_index, ciphertext, digest, size_bytes, created_at)
                values (?, ?, ?, ?, ?, ?)
                """, objectId, chunkIndex, Base64.getEncoder().encodeToString(ciphertext),
                actualDigest, ciphertext.length, timestamp(Instant.now()));
        return new BlobChunk(chunkIndex, actualDigest, ciphertext.length);
    }

    public byte[] getChunk(String objectId, int chunkIndex, String readCapability) {
        requireObject(objectId, readCapability, true);
        List<byte[]> values = jdbc.query("""
                select ciphertext from v2_blob_chunks where object_id = ? and chunk_index = ?
                """, (row, ignored) -> Base64.getDecoder().decode(row.getString(1)), objectId, chunkIndex);
        if (values.isEmpty()) throw new NotFoundException("Blob chunk not found");
        return values.getFirst();
    }

    public BlobManifest manifest(String objectId, String readCapability) {
        BlobObject object = requireObject(objectId, readCapability, true);
        List<BlobChunk> chunks = jdbc.query("""
                select chunk_index, digest, size_bytes from v2_blob_chunks
                where object_id = ? order by chunk_index
                """, (row, ignored) -> new BlobChunk(
                row.getInt("chunk_index"), row.getString("digest"), row.getInt("size_bytes")), objectId);
        return new BlobManifest(objectId, object.expectedSize(), object.chunkSize(), object.expiresAt(), chunks);
    }

    private BlobObject requireObject(String objectId, String capability, boolean read) {
        String hash;
        try {
            hash = capabilityHash(capability);
        } catch (BadRequestException exception) {
            throw new NotFoundException("Blob object not found");
        }
        String column = read ? "read_capability_hash" : "write_capability_hash";
        List<BlobObject> objects = jdbc.query(
                "select id, expected_size, chunk_size, expires_at from v2_blob_objects where id = ? and "
                        + column + " = ? and expires_at > ?",
                (row, ignored) -> new BlobObject(
                        row.getString("id"), row.getLong("expected_size"), row.getInt("chunk_size"),
                        row.getTimestamp("expires_at").toInstant()), objectId, hash, timestamp(Instant.now()));
        if (objects.isEmpty()) throw new NotFoundException("Blob object not found");
        return objects.getFirst();
    }

    private static String capabilityHash(String value) {
        try {
            return V2Encoding.capabilityHash(value);
        } catch (IllegalArgumentException exception) {
            throw new BadRequestException(exception.getMessage());
        }
    }

    public record RegisterBlob(
            String objectId,
            String readCapability,
            String writeCapability,
            long expectedSize,
            int chunkSize,
            Instant expiresAt
    ) {
    }

    public record BlobRegistration(
            String objectId,
            long expectedSize,
            int chunkSize,
            Instant createdAt,
            Instant expiresAt
    ) {
    }

    public record BlobChunk(int index, String digest, int size) {
    }

    public record BlobManifest(
            String objectId,
            long expectedSize,
            int chunkSize,
            Instant expiresAt,
            List<BlobChunk> chunks
    ) {
    }

    private record BlobObject(String id, long expectedSize, int chunkSize, Instant expiresAt) {
    }
}
