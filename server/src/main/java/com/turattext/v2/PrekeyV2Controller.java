package com.turattext.v2;

import jakarta.validation.Valid;
import jakarta.validation.constraints.NotBlank;
import jakarta.validation.constraints.NotNull;
import jakarta.validation.constraints.Positive;
import jakarta.validation.constraints.Size;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.PutMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestHeader;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

import java.time.Instant;
import java.util.List;
import java.util.UUID;

@RestController
@RequestMapping("/v2/prekeys")
public class PrekeyV2Controller {
    private final PrekeyV2Service prekeys;

    public PrekeyV2Controller(PrekeyV2Service prekeys) {
        this.prekeys = prekeys;
    }

    @PutMapping("/{mailboxId}")
    public PrekeyV2Service.PublishResult publish(
            @PathVariable UUID mailboxId,
            @RequestHeader("X-Mailbox-Write-Capability") String capability,
            @Valid @RequestBody PublishBundleRequest request
    ) {
        return prekeys.publish(mailboxId, capability, new PrekeyV2Service.PublishBundle(
                request.userId(), request.deviceId(), request.identityJson(), request.signedPrekeyJson(),
                request.sequence(), request.expiresAt(), request.oneTimePrekeys().stream()
                .map(value -> new PrekeyV2Service.OneTimePrekey(value.prekeyId(), value.prekeyJson()))
                .toList()));
    }

    @GetMapping("/{userId}/{deviceId}/claim")
    public PrekeyV2Service.ClaimedBundle claim(@PathVariable String userId, @PathVariable String deviceId) {
        return prekeys.claim(userId, deviceId);
    }

    public record PublishBundleRequest(
            @NotBlank @Size(max = 80) String userId,
            @NotBlank @Size(max = 90) String deviceId,
            @NotBlank @Size(max = 32_000) String identityJson,
            @NotBlank @Size(max = 64_000) String signedPrekeyJson,
            @Positive long sequence,
            @NotNull Instant expiresAt,
            @NotNull @Size(max = 200) List<@Valid OneTimePrekeyRequest> oneTimePrekeys
    ) {
    }

    public record OneTimePrekeyRequest(
            @NotBlank @Size(max = 100) String prekeyId,
            @NotBlank @Size(max = 16_000) String prekeyJson
    ) {
    }
}
