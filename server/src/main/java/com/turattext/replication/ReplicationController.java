package com.turattext.replication;

import com.turattext.replication.dto.ReplicationConfigureRequest;
import com.turattext.replication.dto.ReplicationEnvelope;
import jakarta.validation.Valid;
import org.springframework.http.HttpStatus;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestHeader;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.ResponseStatus;
import org.springframework.web.bind.annotation.RestController;

import java.time.Instant;
import java.util.Map;

@RestController
@RequestMapping("/api/replication")
public class ReplicationController {
    private final ReplicationService replication;
    private final ReplicationConfiguration configuration;
    private final ReplicationClientService client;

    public ReplicationController(
            ReplicationService replication,
            ReplicationConfiguration configuration,
            ReplicationClientService client
    ) {
        this.replication = replication;
        this.configuration = configuration;
        this.client = client;
    }

    @PostMapping("/sync")
    ReplicationEnvelope synchronize(@RequestBody ReplicationEnvelope request) {
        return replication.synchronize(request);
    }

    @PostMapping("/configure")
    @ResponseStatus(HttpStatus.NO_CONTENT)
    void configure(
            @RequestHeader("X-Local-Host-Secret") String localSecret,
            @Valid @RequestBody ReplicationConfigureRequest request
    ) {
        configuration.configure(localSecret, request);
        client.synchronizeSoon();
    }

    @GetMapping("/status")
    Map<String, Object> status() {
        return Map.of(
                "configured", configuration.current() != null,
                "state", client.state(),
                "lastSuccessAt", client.lastSuccessAt() == null ? "" : client.lastSuccessAt().toString(),
                "checkedAt", Instant.now().toString()
        );
    }
}
