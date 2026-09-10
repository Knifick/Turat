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
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

import java.time.Instant;

@RestController
@RequestMapping("/v2/presence")
public class PresenceV2Controller {
    private final PresenceV2Service presence;

    public PresenceV2Controller(PresenceV2Service presence) {
        this.presence = presence;
    }

    @PutMapping("/{userId}")
    public PresenceV2Service.PresenceRecord publish(
            @PathVariable String userId,
            @Valid @RequestBody PublishPresenceRequest request
    ) {
        return presence.publish(new PresenceV2Service.PublishPresence(
                userId, request.identityPublicKey(), request.sequence(),
                request.claimJson(), request.signature(), request.expiresAt()));
    }

    @GetMapping("/{userId}")
    public PresenceV2Service.PresenceRecord resolve(@PathVariable String userId) {
        return presence.byUser(userId);
    }

    public record PublishPresenceRequest(
            @NotBlank String identityPublicKey,
            @Positive long sequence,
            @NotBlank @Size(max = 4_000) String claimJson,
            @NotBlank String signature,
            @NotNull Instant expiresAt
    ) {
    }
}
