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
import org.springframework.web.bind.annotation.RequestParam;
import org.springframework.web.bind.annotation.RestController;

import java.time.Instant;
import java.util.List;

@RestController
@RequestMapping("/v2")
public class RoutingV2Controller {
    private final RoutingV2Service routing;

    public RoutingV2Controller(RoutingV2Service routing) {
        this.routing = routing;
    }

    @PutMapping("/routing/{userId}")
    public RoutingV2Service.RoutingRecord publish(
            @PathVariable String userId,
            @Valid @RequestBody PublishRoutingRequest request
    ) {
        return routing.publish(new RoutingV2Service.PublishRouting(
                userId, request.identityPublicKey(), request.sequence(), request.descriptorJson(),
                request.signature(), request.expiresAt()));
    }

    @GetMapping("/routing/{userId}")
    public RoutingV2Service.RoutingRecord get(@PathVariable String userId) {
        return routing.get(userId);
    }

    @GetMapping("/transparency/operations")
    public List<RoutingV2Service.TransparencyOperation> operations(
            @RequestParam(defaultValue = "0") long after,
            @RequestParam(defaultValue = "200") int limit
    ) {
        return routing.operations(after, limit);
    }

    @GetMapping("/transparency/checkpoint")
    public RoutingV2Service.TransparencyCheckpoint checkpoint() {
        return routing.checkpoint();
    }

    public record PublishRoutingRequest(
            @NotBlank String identityPublicKey,
            @Positive long sequence,
            @NotBlank @Size(max = 256_000) String descriptorJson,
            @NotBlank String signature,
            @NotNull Instant expiresAt
    ) {
    }
}
