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
import java.util.List;

@RestController
@RequestMapping("/v2/usernames")
public class UsernameV2Controller {
    private final UsernameV2Service usernames;

    public UsernameV2Controller(UsernameV2Service usernames) {
        this.usernames = usernames;
    }

    @PutMapping("/{username}")
    public UsernameV2Service.UsernameRecord publish(
            @PathVariable String username,
            @Valid @RequestBody PublishUsernameRequest request
    ) {
        return usernames.publish(new UsernameV2Service.PublishUsername(
                username, request.userId(), request.identityPublicKey(), request.sequence(),
                request.claimJson(), request.signature(), request.expiresAt()));
    }

    @GetMapping("/{username}")
    public List<UsernameV2Service.UsernameRecord> resolve(@PathVariable String username) {
        return usernames.resolve(username);
    }

    @GetMapping("/by-user/{userId}")
    public UsernameV2Service.UsernameRecord byUser(@PathVariable String userId) {
        return usernames.byUser(userId);
    }

    public record PublishUsernameRequest(
            @NotBlank String userId,
            @NotBlank String identityPublicKey,
            @Positive long sequence,
            @NotBlank @Size(max = 16_000) String claimJson,
            @NotBlank String signature,
            @NotNull Instant expiresAt
    ) {
    }
}
