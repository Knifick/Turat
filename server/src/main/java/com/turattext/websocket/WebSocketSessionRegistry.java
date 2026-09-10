package com.turattext.websocket;

import org.springframework.stereotype.Component;
import org.springframework.web.socket.TextMessage;
import org.springframework.web.socket.WebSocketSession;

import java.io.IOException;
import java.util.Set;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;

@Component
public class WebSocketSessionRegistry {
    private final ConcurrentHashMap<UUID, Set<WebSocketSession>> sessions = new ConcurrentHashMap<>();

    public void add(UUID userId, WebSocketSession session) {
        sessions.computeIfAbsent(userId, ignored -> ConcurrentHashMap.newKeySet()).add(session);
    }

    public void remove(UUID userId, WebSocketSession session) {
        Set<WebSocketSession> userSessions = sessions.get(userId);
        if (userSessions != null) {
            userSessions.remove(session);
            if (userSessions.isEmpty()) {
                sessions.remove(userId);
            }
        }
    }

    public void sendToUser(UUID userId, String json) {
        Set<WebSocketSession> userSessions = sessions.get(userId);
        if (userSessions == null) {
            return;
        }
        for (WebSocketSession session : userSessions) {
            if (session.isOpen()) {
                try {
                    session.sendMessage(new TextMessage(json));
                } catch (IOException ignored) {
                    // A stale socket will be removed on close/error callbacks.
                }
            }
        }
    }

    public boolean isOnline(UUID userId) {
        Set<WebSocketSession> userSessions = sessions.get(userId);
        return userSessions != null && userSessions.stream().anyMatch(WebSocketSession::isOpen);
    }

    public void broadcast(String json) {
        for (UUID userId : sessions.keySet()) {
            sendToUser(userId, json);
        }
    }
}
