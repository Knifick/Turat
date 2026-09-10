package com.turattext.v2;

import org.springframework.jdbc.core.JdbcTemplate;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

import java.time.Instant;

@RestController
@RequestMapping("/v2")
public class V2HealthController {
    private final JdbcTemplate jdbc;
    private final NodeIdentityService identity;

    public V2HealthController(JdbcTemplate jdbc, NodeIdentityService identity) {
        this.jdbc = jdbc;
        this.identity = identity;
    }

    @GetMapping("/health")
    public Health health() {
        jdbc.queryForObject("select 1", Integer.class);
        return new Health("ok", 2, identity.descriptor().nodeId(), Instant.now());
    }

    public record Health(String status, int protocolVersion, String nodeId, Instant time) {}
}

