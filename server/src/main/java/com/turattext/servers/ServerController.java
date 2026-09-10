package com.turattext.servers;

import com.turattext.common.SecurityUtils;
import com.turattext.servers.dto.BecomeHostRequest;
import com.turattext.servers.dto.HealthResponse;
import com.turattext.servers.dto.HostHeartbeatRequest;
import com.turattext.servers.dto.HostRegistrationResponse;
import com.turattext.servers.dto.ServerResponse;
import com.turattext.servers.dto.SyncStatusResponse;
import jakarta.validation.Valid;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

import java.util.List;

@RestController
@RequestMapping("/api/servers")
public class ServerController {
    private final ServerService serverService;

    public ServerController(ServerService serverService) {
        this.serverService = serverService;
    }

    @GetMapping
    List<ServerResponse> list() {
        return serverService.listServers();
    }

    @GetMapping("/health")
    HealthResponse health() {
        return serverService.health();
    }

    @PostMapping("/become-host")
    HostRegistrationResponse becomeHost(@Valid @RequestBody BecomeHostRequest request) {
        return serverService.becomeHost(SecurityUtils.currentUserId(), request);
    }

    @PostMapping("/heartbeat")
    ServerResponse heartbeat(@Valid @RequestBody HostHeartbeatRequest request) {
        return serverService.heartbeat(request);
    }

    @GetMapping("/sync/status")
    List<SyncStatusResponse> syncStatus() {
        return serverService.syncStatus();
    }
}
