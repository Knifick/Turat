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
@RequestMapping("/v2/profiles")
public class ProfileV2Controller {
    private final ProfileV2Service profiles;

    public ProfileV2Controller(ProfileV2Service profiles) {
        this.profiles = profiles;
    }

    @PutMapping("/{userId}")
    public ProfileV2Service.ProfileRecord publish(
            @PathVariable String userId,
            @Valid @RequestBody PublishProfileRequest request
    ) {
        return profiles.publish(new ProfileV2Service.PublishProfile(
                userId, request.identityPublicKey(), request.sequence(),
                request.claimJson(), request.signature(), request.expiresAt()));
    }

    @GetMapping("/{userId}")
    public ProfileV2Service.ProfileRecord resolve(@PathVariable String userId) {
        return profiles.byUser(userId);
    }

    public record PublishProfileRequest(
            @NotBlank String identityPublicKey,
            @Positive long sequence,
            @NotBlank @Size(max = 200_000) String claimJson,
            @NotBlank String signature,
            @NotNull Instant expiresAt
    ) {
    }
}
