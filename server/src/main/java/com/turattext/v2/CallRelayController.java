package com.turattext.v2;

import jakarta.servlet.http.HttpServletRequest;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.http.HttpStatus;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

import java.util.Map;

/** Выдача комнат ретранслятора. Звонящий берёт комнату и передаёт её собеседнику сам. */
@RestController
@RequestMapping("/v2/calls")
public class CallRelayController {
    private final CallRelayService relay;
    private final boolean trustForwardedFor;

    public CallRelayController(
            CallRelayService relay,
            @Value("${turattext.v2.trust-forwarded-for:false}") boolean trustForwardedFor
    ) {
        this.relay = relay;
        this.trustForwardedFor = trustForwardedFor;
    }

    @PostMapping("/rooms")
    public ResponseEntity<?> create(HttpServletRequest request) {
        CallRelayService.RoomTicket ticket = relay.create(clientAddress(request));
        if (ticket == null) {
            return ResponseEntity.status(HttpStatus.TOO_MANY_REQUESTS)
                    .header("Retry-After", "60")
                    .body(Map.of("error", "too_many_calls", "message", "Слишком много звонков, попробуйте позже"));
        }
        return ResponseEntity.ok(ticket);
    }

    private String clientAddress(HttpServletRequest request) {
        if (!trustForwardedFor) return request.getRemoteAddr();
        String forwarded = request.getHeader("X-Forwarded-For");
        if (forwarded == null || forwarded.isBlank()) return request.getRemoteAddr();
        String first = forwarded.split(",", 2)[0].trim();
        return first.isEmpty() || first.length() > 64 ? request.getRemoteAddr() : first;
    }
}
