package com.turattext.websocket;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.turattext.auth.JwtService;
import com.turattext.messages.MessageService;
import com.turattext.messages.dto.MessageResponse;
import com.turattext.messages.dto.SendMessageRequest;
import org.springframework.stereotype.Component;
import org.springframework.web.socket.CloseStatus;
import org.springframework.web.socket.TextMessage;
import org.springframework.web.socket.WebSocketSession;
import org.springframework.web.socket.handler.TextWebSocketHandler;
import org.springframework.web.util.UriComponentsBuilder;

import java.net.URI;
import java.util.Map;
import java.util.UUID;

@Component
public class TuratWebSocketHandler extends TextWebSocketHandler {
    private static final String USER_ID = "userId";

    private final JwtService jwtService;
    private final MessageService messageService;
    private final WebSocketSessionRegistry registry;
    private final ObjectMapper objectMapper;

    public TuratWebSocketHandler(
            JwtService jwtService,
            MessageService messageService,
            WebSocketSessionRegistry registry,
            ObjectMapper objectMapper
    ) {
        this.jwtService = jwtService;
        this.messageService = messageService;
        this.registry = registry;
        this.objectMapper = objectMapper;
    }

    @Override
    public void afterConnectionEstablished(WebSocketSession session) throws Exception {
        String token = tokenFrom(session.getUri());
        if (token == null || token.isBlank()) {
            session.close(CloseStatus.NOT_ACCEPTABLE.withReason("Missing token"));
            return;
        }

        try {
            UUID userId = jwtService.requireUserId(token);
            session.getAttributes().put(USER_ID, userId);
            registry.add(userId, session);
            sendEvent(session, "connection.ready", Map.of("userId", userId));
            broadcastPresence(userId, true);
        } catch (Exception ex) {
            session.close(CloseStatus.NOT_ACCEPTABLE.withReason("Invalid token"));
        }
    }

    @Override
    protected void handleTextMessage(WebSocketSession session, TextMessage message) throws Exception {
        UUID userId = (UUID) session.getAttributes().get(USER_ID);
        if (userId == null) {
            session.close(CloseStatus.NOT_ACCEPTABLE.withReason("Unauthenticated"));
            return;
        }

        try {
            WebSocketEnvelope envelope = objectMapper.readValue(message.getPayload(), WebSocketEnvelope.class);
            if ("message.send".equals(envelope.type())) {
                SendMessageRequest request = objectMapper.treeToValue(envelope.payload(), SendMessageRequest.class);
                MessageResponse saved = messageService.saveFromUser(userId, request);
                String json = objectMapper.writeValueAsString(Map.of("type", "message.received", "payload", saved));
                for (UUID memberId : messageService.memberIds(saved.chatId())) {
                    registry.sendToUser(memberId, json);
                }
            } else {
                sendEvent(session, "error", Map.of("message", "Unknown event type"));
            }
        } catch (Exception ex) {
            sendEvent(session, "error", Map.of("message", ex.getMessage()));
        }
    }

    @Override
    public void afterConnectionClosed(WebSocketSession session, CloseStatus status) {
        UUID userId = (UUID) session.getAttributes().get(USER_ID);
        if (userId != null) {
            registry.remove(userId, session);
            broadcastPresence(userId, registry.isOnline(userId));
        }
    }

    @Override
    public void handleTransportError(WebSocketSession session, Throwable exception) {
        UUID userId = (UUID) session.getAttributes().get(USER_ID);
        if (userId != null) {
            registry.remove(userId, session);
            broadcastPresence(userId, registry.isOnline(userId));
        }
    }

    private void sendEvent(WebSocketSession session, String type, Object payload) throws Exception {
        session.sendMessage(new TextMessage(objectMapper.writeValueAsString(Map.of(
                "type", type,
                "payload", payload
        ))));
    }

    private String tokenFrom(URI uri) {
        if (uri == null) {
            return null;
        }
        return UriComponentsBuilder.fromUri(uri).build().getQueryParams().getFirst("token");
    }

    private void broadcastPresence(UUID userId, boolean online) {
        try {
            String json = objectMapper.writeValueAsString(Map.of(
                    "type", "presence.changed",
                    "payload", Map.of("userId", userId, "online", online)
            ));
            registry.broadcast(json);
        } catch (Exception ignored) {
            // Presence is best-effort and will be refreshed by the REST chat list.
        }
    }
}
