package com.turattext.v2;

import jakarta.annotation.PostConstruct;
import jakarta.annotation.PreDestroy;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.springframework.stereotype.Component;

import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.SocketAddress;
import java.net.StandardSocketOptions;
import java.nio.ByteBuffer;
import java.nio.channels.DatagramChannel;

/**
 * UDP-сторона ретранслятора звонков. Основной транспорт: голосу нужна низкая задержка,
 * а потеря отдельного пакета для него не страшна — её прикрывает кодек.
 */
@Component
public class CallUdpRelay {
    private static final Logger log = LoggerFactory.getLogger(CallUdpRelay.class);

    private final CallRelayService relay;
    private DatagramChannel channel;
    private Thread worker;

    public CallUdpRelay(CallRelayService relay) {
        this.relay = relay;
    }

    @PostConstruct
    void start() {
        int port = relay.udpPort();
        if (port <= 0) {
            log.info("Call relay UDP is disabled; calls will use WebSocket only");
            return;
        }
        try {
            channel = DatagramChannel.open();
            channel.setOption(StandardSocketOptions.SO_RCVBUF, 4 * 1024 * 1024);
            channel.setOption(StandardSocketOptions.SO_SNDBUF, 4 * 1024 * 1024);
            channel.bind(new InetSocketAddress(port));
        } catch (IOException exception) {
            log.error("Call relay cannot bind UDP port {}: {}", port, exception.getMessage());
            channel = null;
            return;
        }
        DatagramChannel bound = channel;
        relay.attachUdp((data, target) -> bound.send(data, target));
        worker = Thread.ofPlatform().daemon().name("call-relay-udp").start(this::loop);
        log.info("Call relay listens on UDP {}", port);
    }

    private void loop() {
        ByteBuffer buffer = ByteBuffer.allocateDirect(2048);
        while (channel != null && channel.isOpen()) {
            try {
                buffer.clear();
                SocketAddress from = channel.receive(buffer);
                if (!(from instanceof InetSocketAddress address)) continue;
                buffer.flip();
                relay.receive(buffer, new CallRelayService.UdpEndpoint(address));
            } catch (IOException exception) {
                if (channel != null && channel.isOpen()) {
                    log.debug("Call relay UDP receive failed: {}", exception.getMessage());
                }
            } catch (RuntimeException exception) {
                log.warn("Call relay dropped a malformed datagram: {}", exception.toString());
            }
        }
    }

    @PreDestroy
    void stop() throws IOException {
        DatagramChannel current = channel;
        channel = null;
        if (current != null) current.close();
        if (worker != null) worker.interrupt();
    }
}
