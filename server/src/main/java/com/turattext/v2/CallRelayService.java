package com.turattext.v2;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.scheduling.annotation.Scheduled;
import org.springframework.stereotype.Service;
import org.springframework.web.socket.BinaryMessage;
import org.springframework.web.socket.WebSocketSession;

import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.URI;
import java.nio.ByteBuffer;
import java.security.MessageDigest;
import java.security.SecureRandom;
import java.time.Instant;
import java.util.Base64;
import java.util.HexFormat;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Ретранслятор голосовых звонков.
 *
 * <p>Node не участвует в звонке и не может его прослушать: клиенты шифруют каждый пакет
 * ключами, о которых договорились внутри своих сквозных каналов, а сюда приходят только
 * непрозрачные байты. Ретранслятор нужен потому, что телефоны почти всегда за NAT и не
 * могут достучаться друг до друга напрямую, а чужие серверы (STUN/TURN мессенджеров-
 * гигантов) — как раз то, что режут. Звонок идёт через Node пользователя.
 *
 * <p>Комната — пара мест: звонящий (0) и принимающий (1). Каждое место открывается своим
 * случайным токеном; токен принимающего звонящий передаёт ему внутри зашифрованного
 * приглашения. Пакет одной стороны пересылается другой — по UDP или по WebSocket поверх
 * TLS на 443-м порту, если UDP недоступен или его режут.
 *
 * <p>Формат кадра одинаков для обоих транспортов:
 * <pre>
 *   JOIN    0x01 | room[16] | side[1] | token[32]
 *   JOINED  0x03 | room[16] | side[1] | peerPresent[1]
 *   DATA    0x02 | room[16] | side[1] | payload (до 1400 байт, шифротекст клиента)
 * </pre>
 */
@Service
public class CallRelayService {
    private static final Logger log = LoggerFactory.getLogger(CallRelayService.class);

    public static final byte JOIN = 0x01;
    public static final byte DATA = 0x02;
    public static final byte JOINED = 0x03;
    public static final int ROOM_BYTES = 16;
    public static final int TOKEN_BYTES = 32;
    public static final int HEADER_BYTES = 1 + ROOM_BYTES + 1;
    public static final int MAX_PAYLOAD_BYTES = 1400;

    /** Поток одной стороны: голос — 50 кадров в секунду, с запасом на служебные пакеты. */
    private static final double PACKETS_PER_SECOND = 160;
    private static final double BURST_PACKETS = 320;
    private static final long ROOM_TTL_MILLIS = 6 * 3_600_000L;
    /** Комната, в которой никто не появился или все замолчали, освобождается. */
    private static final long IDLE_MILLIS = 120_000L;

    private final SecureRandom random = new SecureRandom();
    private final Map<String, Room> rooms = new ConcurrentHashMap<>();
    private final Map<String, Window> creations = new ConcurrentHashMap<>();
    private final int maxRooms;
    private final int roomsPerHour;
    private final int udpPort;
    private final String udpHost;
    private final String webSocketUrl;
    private volatile UdpSender udpSender;

    public CallRelayService(
            @Value("${turattext.v2.call-max-rooms:2000}") int maxRooms,
            @Value("${turattext.v2.call-rooms-per-hour:60}") int roomsPerHour,
            @Value("${turattext.v2.call-udp-port:3479}") int udpPort,
            @Value("${turattext.v2.call-public-host:}") String publicHost,
            @Value("${turattext.server.base-url:http://localhost:8080}") String baseUrl
    ) {
        this.maxRooms = Math.clamp(maxRooms, 1, 100_000);
        this.roomsPerHour = Math.clamp(roomsPerHour, 1, 10_000);
        this.udpPort = Math.clamp(udpPort, 0, 65_535);
        URI base = URI.create(baseUrl.trim());
        String host = publicHost == null || publicHost.isBlank() ? base.getHost() : publicHost.trim();
        this.udpHost = host == null ? "localhost" : host;
        String scheme = "https".equalsIgnoreCase(base.getScheme()) ? "wss" : "ws";
        String authority = base.getRawAuthority() == null ? this.udpHost : base.getRawAuthority();
        this.webSocketUrl = scheme + "://" + authority + "/v2/calls/ws";
    }

    public int udpPort() {
        return udpPort;
    }

    void attachUdp(UdpSender sender) {
        this.udpSender = sender;
    }

    /** Новая комната. {@code null} — превышен предел для этого адреса или всего Node. */
    public RoomTicket create(String clientAddress) {
        if (rooms.size() >= maxRooms || !allowCreation(clientAddress)) {
            return null;
        }
        byte[] id = randomBytes(ROOM_BYTES);
        byte[][] tokens = {randomBytes(TOKEN_BYTES), randomBytes(TOKEN_BYTES)};
        long now = System.currentTimeMillis();
        Room room = new Room(id, tokens, now + ROOM_TTL_MILLIS, now);
        rooms.put(HexFormat.of().formatHex(id), room);
        Base64.Encoder encoder = Base64.getUrlEncoder().withoutPadding();
        return new RoomTicket(
                encoder.encodeToString(id),
                encoder.encodeToString(tokens[0]),
                encoder.encodeToString(tokens[1]),
                udpPort > 0 ? udpHost : null,
                udpPort > 0 ? udpPort : 0,
                webSocketUrl,
                Instant.ofEpochMilli(room.expiresAt));
    }

    /** Кадр от клиента. Возвращает ответ на JOIN; пересылка уходит сама. */
    public void receive(ByteBuffer frame, Endpoint from) {
        int length = frame.remaining();
        if (length < HEADER_BYTES || length > HEADER_BYTES + MAX_PAYLOAD_BYTES) {
            return;
        }
        int start = frame.position();
        byte type = frame.get(start);
        byte[] id = new byte[ROOM_BYTES];
        frame.get(start + 1, id);
        int side = frame.get(start + 1 + ROOM_BYTES);
        if (side != 0 && side != 1) {
            return;
        }
        Room room = rooms.get(HexFormat.of().formatHex(id));
        if (room == null || room.expiresAt < System.currentTimeMillis()) {
            return;
        }
        if (type == JOIN) {
            if (length != HEADER_BYTES + TOKEN_BYTES) {
                return;
            }
            byte[] token = new byte[TOKEN_BYTES];
            frame.get(start + HEADER_BYTES, token);
            if (!MessageDigest.isEqual(token, room.tokens[side])) {
                return;
            }
            // Повторный JOIN — норма: так клиент держит NAT открытым и переезжает между
            // сетями (Wi-Fi → мобильная) без разрыва звонка.
            room.endpoints[side] = from;
            room.lastActivity = System.currentTimeMillis();
            ByteBuffer reply = ByteBuffer.allocate(HEADER_BYTES + 1);
            reply.put(JOINED).put(id).put((byte) side).put((byte) (room.endpoints[1 - side] != null ? 1 : 0));
            reply.flip();
            send(from, reply);
            return;
        }
        if (type != DATA) {
            return;
        }
        Endpoint registered = room.endpoints[side];
        if (registered == null || !registered.sameAs(from) || !room.limits[side].allow()) {
            return;
        }
        room.lastActivity = System.currentTimeMillis();
        Endpoint peer = room.endpoints[1 - side];
        if (peer == null) {
            return;
        }
        ByteBuffer copy = ByteBuffer.allocate(length);
        copy.put(frame.duplicate().position(start).limit(start + length));
        copy.flip();
        send(peer, copy);
    }

    /** WebSocket закрылся: место освобождается, но комната живёт — клиент может вернуться по UDP. */
    public void forget(WebSocketSession session) {
        for (Room room : rooms.values()) {
            for (int side = 0; side < 2; side++) {
                if (room.endpoints[side] instanceof WsEndpoint ws && ws.session.getId().equals(session.getId())) {
                    room.endpoints[side] = null;
                }
            }
        }
    }

    @Scheduled(fixedDelay = 30_000)
    void cleanup() {
        long now = System.currentTimeMillis();
        rooms.entrySet().removeIf(entry -> entry.getValue().expiresAt < now
                || now - entry.getValue().lastActivity > IDLE_MILLIS);
        long hour = now / 3_600_000L;
        creations.entrySet().removeIf(entry -> entry.getValue().hour < hour);
    }

    public int activeRooms() {
        return rooms.size();
    }

    private void send(Endpoint target, ByteBuffer data) {
        try {
            if (target instanceof UdpEndpoint udp) {
                UdpSender sender = udpSender;
                if (sender != null) sender.send(data, udp.address);
            } else if (target instanceof WsEndpoint ws && ws.session.isOpen()) {
                ws.session.sendMessage(new BinaryMessage(data));
            }
        } catch (IOException | IllegalStateException exception) {
            log.debug("Call relay send failed: {}", exception.getMessage());
        }
    }

    private boolean allowCreation(String clientAddress) {
        long hour = System.currentTimeMillis() / 3_600_000L;
        Window window = creations.compute(clientAddress == null ? "?" : clientAddress, (ignored, current) ->
                current == null || current.hour != hour ? new Window(hour) : current);
        return window.count.incrementAndGet() <= roomsPerHour;
    }

    private byte[] randomBytes(int count) {
        byte[] value = new byte[count];
        random.nextBytes(value);
        return value;
    }

    public record RoomTicket(
            String roomId,
            String callerToken,
            String calleeToken,
            String udpHost,
            int udpPort,
            String webSocketUrl,
            Instant expiresAt
    ) {
    }

    public sealed interface Endpoint permits UdpEndpoint, WsEndpoint {
        boolean sameAs(Endpoint other);
    }

    public record UdpEndpoint(InetSocketAddress address) implements Endpoint {
        @Override
        public boolean sameAs(Endpoint other) {
            return other instanceof UdpEndpoint udp && udp.address.equals(address);
        }
    }

    public record WsEndpoint(WebSocketSession session) implements Endpoint {
        @Override
        public boolean sameAs(Endpoint other) {
            return other instanceof WsEndpoint ws && ws.session.getId().equals(session.getId());
        }
    }

    interface UdpSender {
        void send(ByteBuffer data, InetSocketAddress target) throws IOException;
    }

    private static final class Room {
        private final byte[] id;
        private final byte[][] tokens;
        private final long expiresAt;
        private final Endpoint[] endpoints = new Endpoint[2];
        private final TokenBucket[] limits = {new TokenBucket(), new TokenBucket()};
        private volatile long lastActivity;

        private Room(byte[] id, byte[][] tokens, long expiresAt, long now) {
            this.id = id;
            this.tokens = tokens;
            this.expiresAt = expiresAt;
            this.lastActivity = now;
        }
    }

    private static final class TokenBucket {
        private double tokens = BURST_PACKETS;
        private long updatedNanos = System.nanoTime();

        private synchronized boolean allow() {
            long now = System.nanoTime();
            tokens = Math.min(BURST_PACKETS, tokens + (now - updatedNanos) / 1e9 * PACKETS_PER_SECOND);
            updatedNanos = now;
            if (tokens < 1) return false;
            tokens -= 1;
            return true;
        }
    }

    private static final class Window {
        private final long hour;
        private final AtomicLong count = new AtomicLong();

        private Window(long hour) {
            this.hour = hour;
        }
    }
}
