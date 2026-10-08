package com.turattext.v2;

import org.springframework.stereotype.Component;
import org.springframework.web.socket.BinaryMessage;
import org.springframework.web.socket.CloseStatus;
import org.springframework.web.socket.WebSocketSession;
import org.springframework.web.socket.handler.BinaryWebSocketHandler;
import org.springframework.web.socket.handler.ConcurrentWebSocketSessionDecorator;

import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Запасной транспорт звонка: те же кадры, что и по UDP, но внутри WebSocket поверх TLS на
 * 443-м порту. Снаружи это неотличимо от обычной работы клиента с Node, поэтому звонок
 * проходит там, где UDP закрыт или голосовой трафик режут.
 */
@Component
public class CallWebSocketHandler extends BinaryWebSocketHandler {
    private final CallRelayService relay;
    /** Отправка в сессию из разных потоков должна быть упорядочена: декоратор это делает. */
    private final Map<String, WebSocketSession> sessions = new ConcurrentHashMap<>();

    public CallWebSocketHandler(CallRelayService relay) {
        this.relay = relay;
    }

    @Override
    public void afterConnectionEstablished(WebSocketSession session) {
        session.setBinaryMessageSizeLimit(4096);
        sessions.put(session.getId(), new ConcurrentWebSocketSessionDecorator(
                session, 2_000, 256 * 1024, ConcurrentWebSocketSessionDecorator.OverflowStrategy.DROP));
    }

    @Override
    protected void handleBinaryMessage(WebSocketSession session, BinaryMessage message) {
        WebSocketSession decorated = sessions.get(session.getId());
        if (decorated == null) return;
        relay.receive(message.getPayload(), new CallRelayService.WsEndpoint(decorated));
    }

    @Override
    public void afterConnectionClosed(WebSocketSession session, CloseStatus status) {
        WebSocketSession decorated = sessions.remove(session.getId());
        if (decorated != null) relay.forget(decorated);
    }
}
