package com.turattext.v2;

import jakarta.validation.Valid;
import jakarta.validation.constraints.NotBlank;
import jakarta.validation.constraints.NotNull;
import jakarta.validation.constraints.Positive;
import jakarta.validation.constraints.Size;
import org.springframework.http.MediaType;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.PutMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestHeader;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

import java.time.Instant;

@RestController
@RequestMapping("/v2/blobs")
public class BlobV2Controller {
    private final BlobV2Service blobs;

    public BlobV2Controller(BlobV2Service blobs) {
        this.blobs = blobs;
    }

    @PostMapping
    public BlobV2Service.BlobRegistration register(@Valid @RequestBody RegisterBlobRequest request) {
        return blobs.register(new BlobV2Service.RegisterBlob(
                request.objectId(), request.readCapability(), request.writeCapability(),
                request.expectedSize(), request.chunkSize(), request.expiresAt()));
    }

    @PutMapping(value = "/{objectId}/chunks/{chunkIndex}", consumes = MediaType.APPLICATION_OCTET_STREAM_VALUE)
    public BlobV2Service.BlobChunk putChunk(
            @PathVariable String objectId,
            @PathVariable int chunkIndex,
            @RequestHeader("X-Blob-Write-Capability") String capability,
            @RequestHeader("X-Chunk-SHA256") String digest,
            @RequestBody byte[] ciphertext
    ) {
        return blobs.putChunk(objectId, chunkIndex, capability, digest, ciphertext);
    }

    @GetMapping(value = "/{objectId}/chunks/{chunkIndex}", produces = MediaType.APPLICATION_OCTET_STREAM_VALUE)
    public ResponseEntity<byte[]> getChunk(
            @PathVariable String objectId,
            @PathVariable int chunkIndex,
            @RequestHeader("X-Blob-Read-Capability") String capability
    ) {
        return ResponseEntity.ok().contentType(MediaType.APPLICATION_OCTET_STREAM)
                .body(blobs.getChunk(objectId, chunkIndex, capability));
    }

    @GetMapping("/{objectId}/manifest")
    public BlobV2Service.BlobManifest manifest(
            @PathVariable String objectId,
            @RequestHeader("X-Blob-Read-Capability") String capability
    ) {
        return blobs.manifest(objectId, capability);
    }

    public record RegisterBlobRequest(
            @NotBlank @Size(max = 100) String objectId,
            @NotBlank @Size(min = 32, max = 256) String readCapability,
            @NotBlank @Size(min = 32, max = 256) String writeCapability,
            @Positive long expectedSize,
            @Positive int chunkSize,
            @NotNull Instant expiresAt
    ) {
    }
}
