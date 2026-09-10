package com.turattext.servers;

import com.turattext.servers.dto.BecomeHostRequest;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.boot.context.event.ApplicationReadyEvent;
import org.springframework.context.event.EventListener;
import org.springframework.scheduling.annotation.Scheduled;
import org.springframework.stereotype.Service;

import java.net.URI;

@Service
public class SelfServerHeartbeatService {
    private static final Logger log = LoggerFactory.getLogger(SelfServerHeartbeatService.class);

    private final ServerService serverService;
    private final boolean selfRegister;
    private final String serverName;
    private final String baseUrl;

    public SelfServerHeartbeatService(
            ServerService serverService,
            @Value("${turattext.server.self-register:false}") boolean selfRegister,
            @Value("${turattext.server.name:TuratText Server}") String serverName,
            @Value("${turattext.server.base-url:http://localhost:8080}") String baseUrl
    ) {
        this.serverService = serverService;
        this.selfRegister = selfRegister;
        this.serverName = serverName;
        this.baseUrl = baseUrl;
    }

    @EventListener(ApplicationReadyEvent.class)
    public void registerOnStartup() {
        heartbeat();
    }

    @Scheduled(fixedDelay = 30000, initialDelay = 30000)
    public void heartbeat() {
        if (!selfRegister || isLocalhost(baseUrl)) {
            return;
        }

        try {
            serverService.becomeHost(new BecomeHostRequest(null, serverName, baseUrl, null));
        } catch (Exception ex) {
            log.warn("Self server heartbeat failed: {}", ex.getMessage());
        }
    }

    private static boolean isLocalhost(String baseUrl) {
        try {
            URI uri = new URI(baseUrl);
            String host = uri.getHost();
            return host == null
                    || uri.isOpaque()
                    || host.equalsIgnoreCase("localhost")
                    || host.equals("127.0.0.1")
                    || host.equals("0.0.0.0")
                    || host.equals("::1");
        } catch (Exception ex) {
            return true;
        }
    }
}
